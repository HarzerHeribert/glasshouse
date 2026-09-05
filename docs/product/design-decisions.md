# Glasshouse design decisions

> This describes the product. Do not cite it as instruction for how to run a
> worker or a batch — that belongs in `docs/process/`.

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

### Refined 2026-08-26: the test is what the control does, not whether its consumer exists

The rule above — *build only the sections whose feature exists, and their map
boxes stay unchecked until then* — was applied to the Routing and Memory
sections and produced two different answers, which is how it earned a
refinement rather than an exception.

**Routing settings shipped, and its six boxes are checked**, while Phases
9I-9K and 34-38 remain entirely unbuilt. Nothing routes yet. But the section is
not a shell: `max_router_latency`, `max_marginal_cost`, `prefer_free`,
`premium_reserve` and the model pin are validated typed values that resolve
project -> user -> default, show their layer, persist through the same
consent-gated writer as every other setting, and survive a reload. Using a
control there changes a file on disk. That is a feature, and it is the one
Phase 2D names.

**Memory settings did not ship, and its box stays open.** The section exists
and says, truthfully, "Project memory is not available in this build. There are
no memory settings to save." That is honest and worth keeping — it is the
opposite of the failure the original decision feared, since it tells the user a
capability is *absent* rather than implying one is present. But a section with
no settings in it is not a settings section, and the box says to add one.

So the operative test is not *"does the consuming feature exist?"* but:

> **Does using a control in this section do something real and durable?**

If yes, the section may ship and its boxes may close, whatever consumes the
value later. If no, it is a shape for someone else to fill, which is what the
original decision was protecting against.

Why the original wording needed changing rather than an exception: it ties a
settings box to a *different phase's* completion, which makes Phase 2D
unclosable for reasons that have nothing to do with settings. Providers and
Launch Profiles already shipped this way as their features landed, so the rule
was never actually tested against a section whose controls were real but whose
consumer was not. Routing is that case.

What this does not license: a section whose controls write a value no design
has decided the meaning of. The routing values each have a defined range, a
default, and a stated meaning in the map. Persisting a number nobody has
defined is still a shell, and a durable one.

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

## Launch profiles — a declared value, resolved through the adapter into an overlay

### The conflict

Phase 9A asks for twenty-six things at once: an abstraction, a composition, a
default approval mode, an acknowledgement ledger, an environment overlay, an
argument overlay, a generated-configuration mechanism, a protocol constraint,
session recording, and diagnostics. Built as one type they collapse into a
struct that means everything and guarantees nothing.

They separate cleanly along one seam: **what the user declares** versus **what
resolution produces**. A profile is inert configuration. An overlay is the
concrete, per-launch result of asking a specific adapter whether that profile
can be honoured by that harness.

### The decision: `LaunchProfile` is data, `LaunchOverlay` is its resolution

```text
LaunchProfile  (declarative, stored in configuration)
    name, harness, backend resource, model, expected protocol,
    approval selection, class

        resolve(profile, adapter)  ->  Result<LaunchOverlay, Refusal>

LaunchOverlay  (ephemeral, applies to exactly one child process)
    args, env, generated configuration, mechanism notes
```

`HarnessLaunch` already *is* the child-process overlay mechanism — it takes
args and env and nothing else can reach the child — so Phase 9A's line about
representing the mechanisms "together as an ephemeral child-process launch
overlay" is satisfied by building the overlay and handing it to
`HarnessLaunch`, not by inventing a second launcher.

Resolution is the only place allowed to turn a declaration into arguments, and
it **refuses rather than invents**. A profile naming a mechanism the adapter
does not declare is an error, never a guess at an environment-variable name.

### The decision: the approval declaration carries argv, not prose

This one was forced by the binaries, and it is the third time in this project
that a declaration was derived from the wrong artifact.

`ApprovalModes` stored one human-readable string per mode. Three of the seven
values cannot be used as launch arguments at all:

- **Claude Code** declared `"auto-mode"`. That is a *subcommand* — "Inspect or
  reset auto mode classifier configuration". Appending it to a launch would run
  the subcommand instead of starting a session. The flag that selects the mode
  **for a session** is `--permission-mode auto`, one of six choices
  (`acceptEdits`, `auto`, `bypassPermissions`, `manual`, `dontAsk`, `plan`).
  Verified against Claude Code 2.1.245: an invalid value is rejected with the
  allowed list, and `auto` is accepted.
- **Codex** and **Cursor** declared sandbox as usage strings —
  `"-s/--sandbox <read-only|workspace-write|danger-full-access>"` and
  `"--sandbox <mode>"` — with placeholders no process can receive.

So a mode now carries `args` (the exact argv that selects it) beside
`description` (what the harness's own documentation says). Keeping both is the
point: the description is for a human reading `glasshouse doctor`, and
conflating them is precisely what produced an unlaunchable declaration.

The predecessors: Antigravity's executable name was read from documentation
rather than an install, and Codex's hook events were read from trust-record
keys rather than its hook review screen. The pattern is the same each time —
a real artifact, cited for a purpose it does not serve.

### The decision: a default that falls back is not a request that is refused

The map says the approval selection defaults "to its native automatic-review
mode where one exists and never to a blanket bypass". Four of the seven
harnesses have no automatic-review mode, so "where one exists" has to mean
something precise:

- A profile that **explicitly** asks for automatic review from a harness that
  declares none is **refused**. That is the invariant the approvals decision
  already records, and refusing is what keeps a user from believing a session
  is being classified when it is not.
- A profile that merely **took the default** and meets such a harness resolves
  to the harness's *own* default — no approval argument at all. Not a bypass,
  and not an error, because the user asked for nothing in particular and
  Glasshouse changing nothing is the honest outcome.

The asymmetry is deliberate. An explicit request is a claim about the session
that Glasshouse must not silently fail to honour; a default is an absence of a
claim.

### The decision: a bypass needs a recorded acknowledgement, once, per harness

Where a harness offers only a blanket bypass, the user may still choose it —
they settled that — but not silently. Resolution refuses a bypass until an
acknowledgement for that harness is recorded, and the prompt shows the mode in
**the harness's own words**, which is what `description` is now for:
OpenCode's `--auto` is "auto-approve permissions that are not explicitly denied
(dangerous!)"; Antigravity's is "Auto-approve all tool permission requests
without prompting".

The acknowledgement is stored per harness in user-level configuration, so it is
asked once. Asking every time is how a warning becomes noise, and noise is what
made per-command approval useless in the first place.

### The decision: a backend resource is a distinct type, and only `Native` resolves today

"A provider, direct API, router, or gateway is a backend resource for a
harness, not an interactive coding harness by itself" is enforced by there
being no way to start a session from a `BackendResource`: sessions are started
from a resolved harness executable, and that path takes an `IntegrationId`.

`BackendResource::Native` is the only variant resolution accepts today.
`DirectProvider` and `GlasshouseGateway` are representable and are **refused**
with a diagnostic naming the phase that supplies them, because provider
configuration, protocol metadata and secret storage are Phases 9C, 9D and 9E.
Representing them now and refusing them is what makes the protocol constraint
and the class marking real rather than decorative.

### The decision: the Native profile is implied, never stored

Every harness has a Native profile by construction rather than by a
configuration entry. Nothing can delete it, so "keep native-subscription
profiles available even when gateway providers are configured" is structural
instead of a policy someone has to remember — and `glasshouse launch` with no
profile is exactly the Native profile, so today's behaviour is the default
path rather than a special case beside it.

### The decision: profiles are configuration, and the project database has no room for them

"Store launch-profile configuration separately from project memory" is
satisfied by where the type lives: profiles are TOML in the user and project
configuration layers, with the same `Layer` provenance every other setting
carries. The project database gains a *reference* — which profile a session
ran under — and no definition. There is no profile table, and a test says so.

### Invariants a test must hold to

1. No approval argument is a usage string: no element contains a space, `<`,
   `>` or `|`.
2. Claude Code's automatic review is `--permission-mode auto`, and never the
   `auto-mode` subcommand.
3. A profile explicitly requesting automatic review from a harness declaring
   none is refused; a profile that took the default resolves to no approval
   argument, never to a bypass.
4. A bypass is refused until an acknowledgement for that harness is recorded,
   and the acknowledgement is per harness rather than global.
5. Resolution refuses a mechanism the adapter does not declare instead of
   inventing an environment-variable name.
6. A `BackendResource` cannot start a session; only a resolved harness can.
7. The project database contains no launch-profile definition, only a
   reference to one.

---

## An approval declaration has to carry argv, not prose about argv

### The conflict

The decision above says a launch profile *selects* a harness's approval mode.
Selecting one means putting arguments on a command line, and `ApprovalModes`
stored a single human-readable string per mode. Three of the seven values
could not be used that way at all:

- **Claude Code declared `auto-mode`.** That is a *subcommand* — "Inspect or
  reset auto mode classifier configuration". Appending it to a launch would
  have run the subcommand instead of starting a session. The thing that selects
  the mode **for a session** is `--permission-mode auto`, one of six choices
  (`acceptEdits`, `auto`, `bypassPermissions`, `manual`, `dontAsk`, `plan`).
- **Codex and Cursor declared their sandbox as usage strings** —
  `-s/--sandbox <read-only|workspace-write|danger-full-access>` and
  `--sandbox <mode>` — with placeholders no process can receive.

The declaration was not *false*. `auto-mode` is real, and Claude Code really
does have a classifier. It was cited for a purpose it does not serve, and that
only became visible when something tried to use it.

### The decision: two fields, and never one

```rust
pub struct ApprovalMode {
    pub args: &'static [&'static str],   // exactly what selects the mode
    pub description: &'static str,       // what the harness's docs call it
}

pub struct SandboxSelector {
    pub flag: &'static str,
    pub values: &'static [&'static str], // empty means a boolean switch
}
```

`description` is for a human reading `glasshouse doctor`; `args` is what
reaches the process. Keeping them apart is the whole decision — collapsing
them back into one field is how the defect happened, and a future tidy-up that
"simplifies" this is reintroducing it.

`values` being empty is load-bearing rather than a default: Antigravity's
`--sandbox` is a boolean switch, while Codex's and Cursor's take a value from a
fixed set. A caller that appends a value to the first, or omits one from the
others, produces an invocation the harness rejects.

`HarnessAdapter::approval_args` answers `None` for a mode a harness lacks.
`None` means "this harness cannot be launched that way", never "launch it some
other way" — a bypass standing in for automatic review is exactly the silent
downgrade the previous decision forbids, so the fail-closed answer lives in the
accessor rather than in every caller's discipline.

### Why the diagnostic shows both halves

`glasshouse doctor` renders the description *and* the argv:

    approvals: auto review `auto permission mode for the session`
               (--permission-mode auto); bypass `Bypass all permission checks`
               (--dangerously-skip-permissions)

A diagnostic that showed only prose would hide the half that actually reaches
the process — and this row previously named a subcommand that could never have
started a session. Showing the arguments is what lets a reader notice that.

### The third time, and the rule it earns

Antigravity's executable name was read from documentation rather than an
install. Codex's hook event names were read from trust-record keys rather than
its hook review screen. Now Claude Code's approval mode was read from a
subcommand listing rather than the session flag.

Each was a real artifact, cited for a purpose it did not serve. The rule:
**before a declaration is used, check that its evidence supports the use, not
merely the claim.** A declaration nobody consumes is never wrong in a way
anyone notices.

### Invariants a test must hold to

1. No approval argument is a usage string: no element contains a space, `<`,
   `>` or `|`.
2. Claude Code's automatic review is `--permission-mode auto`, never the
   `auto-mode` subcommand.
3. A harness without automatic review returns `None`, never its bypass argv.
4. No description contains a backtick, because the report wraps descriptions in
   backticks and one carrying its own renders doubled.

---

## The gateways this user already has, and what Glasshouse owes them

### Why this is written down

The user keeps a working multi-gateway setup in `~/projects/openrouter-clis`
(the `ox` gateway). It is the concrete answer to a question Phases 9C-9I would
otherwise have to guess at: *which* providers, *which* endpoints, and what the
credential situation actually looks like on a real machine. Rediscovering it
means reading someone else's 100 KB gateway script again, so it is recorded
here once.

**No key value appears in this repository, in any form, ever.** What follows is
names, endpoints and environment-variable *names* — the same class of
information `glasshouse doctor` already prints. The values live in that
project's gitignored `.env` and were never read.

### The inventory (2026-08-25)

| Gateway | Endpoint | Credential env name | Endpoint implemented in `ox`? |
|---|---|---|---|
| OpenRouter | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | yes |
| UnoRouter | `https://api.unorouter.com/v1` | `UNOROUTER_API_KEY` | yes |
| AnyRouter | `https://anyrouter.dev/api/v1` | `ANYROUTER_API_KEY` | yes |
| Z.ai | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | yes |
| OpenCode Zen | `https://opencode.ai/zen/v1` (and `/zen/go/v1`) | — | yes |
| Kilo | `https://kilo.ai/api/openrouter` | `KILO_API_KEY` | no |
| Nous | `https://inference-api.nousresearch.com/v1` | `NOUS_API_KEY` | no |

**A key held is not an endpoint verified**, and this table has moved on twice
since it was written.

**Kilo and Nous now have endpoints, read from the services themselves**
(2026-08-26). `GET https://kilo.ai/api/openrouter/models` answers 200 with 367
model ids; `GET https://inference-api.nousresearch.com/v1/models` answers 200
with 372. Note that Kilo **moved**: `https://kilocode.ai/api/openrouter/models`
answers `308` redirecting to `kilo.ai`, so the old host is only usable by a
client that follows redirects, which a POST should not be asked to do. Full
probe record in `.agent-runtime/notes-provider-probes.md`. Both are now
templatable with evidence; neither has a template yet.

Glasshouse must still not invent base URLs. That rule is unchanged; it is
simply no longer binding on Kilo and Nous, because nobody had to invent
anything — the endpoints were read from the live services.

Free capacity is real here: the reference gateway routes models such as
`ox-alpha:free` and treats Zen's free endpoint as a first-class route — while
recording that it "returns 503 under load", which is exactly the kind of
free-tier health signal Phase 9I's cooldown lines exist for.

### What the map already covered, and what it did not

Most of this was already specified, which is worth stating rather than
re-specifying: Phase 9D already named UnoRouter, AnyRouter, Kilo and Nous;
Phase 9E already allowed multiple credentials per provider; Phase 9I already
covered marking models free and applying cooldowns; Phase 39 already defined
disposable one-shot jobs over OpenAI-compatible gateways.

Four things were genuinely missing, and were added as an explicit
specification change:

1. **Z.ai and OpenCode Zen** were not named among the services the generic
   templates should cover. Now they are.
2. **A key pool is not the same as a second provider instance.** Phase 9E's
   existing line allows several credentials only by configuring the provider
   twice. The user asked for multiple keys *per router*, which is a pool.
3. **Quota is per credential, not per provider.** Two keys for the same router
   are two separate allowances, and a router whose first key is exhausted is
   not an exhausted router. Without this, a rate-limited key would take a
   healthy provider out of routing.
4. **Glasshouse's own tests may spend free capacity, and only free capacity.**
   The user offered these keys for testing on the condition that free models
   are used. That condition is now a rule rather than a memory: an automated
   evaluation run may use configured zero-cost models and must never reach a
   metered resource without an explicit opt-in.

### The boundary this does not move

These gateways are **backend resources**, never interactive harnesses — the
distinction Phase 9A already enforces in the type system. A key for OpenRouter
does not make OpenRouter a coding agent; it makes it something an installed
harness can be pointed at through a launch profile, or something a bounded
disposable job can call. Nothing here weakens that, and nothing here weakens
the secret boundary: credentials remain outside project memory, tracked
configuration, checkpoints, event payloads and diagnostics, and Glasshouse
stores references rather than values.

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


## A credential-shaped value minted inside the crate should not widen `Secret`

Decided 2026-08-26 by the Opus orchestrator, on a recommendation from the
Phase 9G team lead.

The gateway needs an authentication token with exactly `Secret`'s protections —
no `Display`, no `Deref`, no `AsRef<str>`, no serde, a `Debug` that prints
`REDACTED`. It cannot *be* a `Secret`: the inner field is private to
`crate::secret` and the only other constructor is `#[cfg(test)]`, so a sibling
module cannot mint one in production. `GatewayToken` therefore mirrors it item
for item and carries the same source-scan test.

The lead recommended a `pub(crate) Secret::from_generated(String)`. **Declined,
and the reason matters more than the decision:** that constructor would let any
module in the crate turn arbitrary text into a `Secret`, which is precisely
what the module documentation says must be impossible — "an outside crate or
module cannot construct one from arbitrary text and then claim the protections
of this type for it." A convenience that dissolves the guarantee is not worth
having.

If the duplication ever becomes a real cost, the right shape is
**`Secret::generate(len)` living inside `crate::secret`**, filling its own bytes
from `getrandom`. The value then never comes from a caller at all, so the
boundary is preserved rather than widened. Nothing needs it today: one mirrored
type with a shared `REDACTED` constant and a shared scan test is cheaper than a
new API on the most security-sensitive module in the codebase.


## Phase 9E's native secret stores: `keyring` 3.x, not 4.x, and the reason is the MSRV

Decided 2026-08-26 by the Opus orchestrator, from crates.io metadata rather than
recollection. Recorded so the packet that implements the three native-keychain
lines does not have to rediscover it — or worse, reach for the newest version.

Phase 9E has three unchecked lines asking Glasshouse to prefer the macOS
Keychain, Windows Credential Manager, and a Secret Service keyring. All three
want one cross-platform crate behind `SecretStore`, and `keyring` is the
obvious candidate.

**The obvious version does not fit.** `keyring 4.1.6` declares
`rust-version = "1.88.0"`. This workspace pins **1.85**, and CI gates on
`rustup run 1.85.0 cargo check --locked`. Adding 4.x breaks that gate.

> **Superseded 2026-08-26, and the correction matters more than the entry.**
> The premise above was false when it was written. `ratatui` and `time`
> *already* declared `rust-version = "1.88.0"`, so the workspace floor was
> 1.88 all along and the manifest's `1.85` was a claim no current cargo would
> honour. Nothing caught it because the gate could not: cargo 1.85.0 does not
> enforce `rust-version` at all. The decision to pin `keyring = "3.6"` still
> stands, but **not for this reason** — see the MSRV entry below.

**`keyring 3.6.3` declares `rust-version = "1.75"`**, which is comfortably
inside the workspace MSRV. So the line to pin is `keyring = "3.6"`.

Three consequences worth carrying into the packet:

- **Pin the minor line, not `"3"`.** A caret on `3` is fine today, but the 3→4
  jump is exactly the MSRV cliff above, and `--locked` in CI is the only thing
  standing between a routine `cargo update` and a red gate.
- **The platform backends are feature-gated in 3.x**, and which features are
  needed was *not* established here. The packet must read the crate's own
  feature list before enabling anything, and must not assume the 4.x `v1`
  feature name applies.
- **Raising the workspace MSRV to 1.88 is a product decision, not a
  convenience.** It decides who can build Glasshouse. If a future line genuinely
  needs `keyring` 4.x, that trade goes to the user, not into a dependency bump.

**Still unproven, and the packet must say so rather than claim it:** a
Secret Service keyring needs a session bus, and a Windows Credential Manager
needs a real user session. Neither is obviously present on a CI runner. macOS
can be proven on this machine; the other two need either a real host or an
honest `LOCALLY VERIFIED` with the platform gap recorded.

---

## The workspace MSRV is 1.88, and the gate that said 1.85 could not have known

_Decided 2026-08-26, by the orchestrator, on evidence from the first run of a
new CI job._

`Cargo.toml` declared `rust-version = "1.85"`. It was wrong, and had been for
as long as `ratatui 0.30` was a dependency.

**What the evidence says.** Scanning every one of the 282 locked packages for
its declared `rust-version` puts the true floor at **1.88.0**, and it is not a
leaf dependency doing it:

| package | declares |
|---|---|
| `ratatui` 0.30.2, and its `-core`, `-crossterm`, `-widgets`, `-termina`, `-termwiz` | 1.88.0 |
| `time` 0.3.55, `time-core` 0.1.9 | 1.88.0 |
| everything else | ≤ 1.85 |

`ratatui` is the TUI framework. There is no version of this product that drops
it, so the floor is not negotiable by dependency surgery.

**Why nobody noticed.** The project's MSRV gate was
`rustup run 1.85.0 cargo check --locked`, and it passed. It passed because
**cargo 1.85.0 does not enforce `rust-version`** — it compiles whatever
compiles. Cargo 1.96 refuses the same workspace outright. So the gate was
checking that the code *compiles* on an old rustc, never that the promise in
the manifest was true. It was the MSRV equivalent of a test that passes whether
or not the behaviour is there.

A second, sharper trap sat underneath it, specific to this machine but general
in shape: `rustup run <version> cargo` execs the toolchain's cargo, and cargo
then resolves **`rustc` from `PATH`**. With Homebrew's rust ahead of
`~/.cargo/bin`, the "1.85 check" could compile with rustc 1.96.1 and report
success. Both halves have to be pinned with `rustup which --toolchain`, which
is why the gate is now `scripts/msrv-check.sh` and not a command.

**The decision.** Declare `rust-version = "1.88"` — the floor that is already
true — rather than downgrade `ratatui` and `time` to hold a number nobody was
verifying. The alternative was to pin two central dependencies to older
releases purely to preserve a claim, which trades real security exposure for a
cosmetic one; the repository owner named that risk explicitly when asked.

**Note what this is not.** The *code* compiles fine on rustc 1.85.0 — that was
tested, in a clean target directory. 1.88 is required because our dependencies
*declare* it and cargo enforces declarations, not because any 1.86–1.88 language
feature is in use. The honest statement is "no current cargo will build this
below 1.88", and that is what a declared MSRV is for.

**Consequences.**

- `keyring 4.x` is no longer blocked by the MSRV. It stays pinned at `3.6` for
  the smaller reason that the macOS Keychain path is verified against 3.6.3 and
  moving is a re-verification exercise with its own evidence.
- The `msrv` CI job reads the version out of `Cargo.toml` rather than repeating
  it, so the manifest and the gate cannot drift. It found this defect on its
  first run, on all three platforms, which is the whole argument for having it.

---

## Subscription auth and key auth do not mix, and a gateway launch should be one atomic artifact

_Raised by the repository owner, 2026-08-26, while reviewing the Phase 9G
`/models` refusal. Recorded as direction; not yet implemented._

**The principle, in the owner's terms.** A harness authenticated by
subscription (OAuth, a `claude.ai` login) and a harness authenticated by an API
key are two different things, and Glasshouse should not turn one into the other
by mutating its environment. When a launch is backed by a gateway key rather
than the user's own subscription, the credential should arrive through a
**generated shim** — one atomic launch artifact — rather than by configuring a
harness on the fly.

### The hazard is real and this project already probed it

From the Phase 9F evidence, established against the real binary:

> `ANTHROPIC_AUTH_TOKEN` **wins over the user's claude.ai login for that child
> and leaves it untouched on disk** — the harness said so itself.

That is the whole problem in one line. Set the variable and the subscription is
still logged in, still valid, still on disk — and silently not the thing paying
for the session. Nothing on screen distinguishes the two states. A user who
believes they are spending their subscription may be spending gateway credits,
or the reverse, and the only way to find out is to look at a bill.

**One facet of this was already discovered independently.** Phase 9A's
`--permission-mode auto` composes a classifier call that a third-party gateway
cannot serve, so a gateway-backed Claude Code session would have come up with
its tools blocked. The fix made `resolve` backend-aware. That was treated as a
one-off; under this principle it is an instance — **a gateway-backed session is
a different kind of session, not a subscription session with extra environment
variables**, and features that assume the native backend do not survive the
substitution.

### What already exists

- `LaunchProfile` is already marked `native-subscription | direct-provider |
  glasshouse-gateway` (map line 370, checked), so the taxonomy is not new.
- `resolve` is already backend-aware for approval modes.
- **`glasshouse shim` already generates exactly the artifact this asks for**
  (Phase 9B). It `exec`s `glasshouse run <harness> --profile <name>`.

So the mechanism is built. What is missing is that **nothing makes it the
mandatory path** for a non-native launch, and nothing refuses the mixed state.

### One correction to the shape, which matters

**The shim must not carry the credential.** A key written into a file on disk is
strictly worse than one passed to a child process — it persists, it is readable,
it outlives the session, and it is exactly what `crate::secret` exists to
prevent. The generated shim therefore stays what it is today: a stable name that
`exec`s `glasshouse run`, with the credential resolved **in-process, per
launch**, and never written anywhere.

That is not a weakening of the idea. The value the owner is pointing at is
**atomicity of configuration**, not relocation of the secret: one named artifact
that fully determines harness, provider, protocol, approval mode and credential
source, instead of a set of environment mutations applied to a harness that also
has its own ambient auth. The shim already delivers that; it simply is not
required.

### What this would change

1. **Refuse the mixed state rather than silently winning.** A gateway-backed or
   direct-provider launch of a harness that also holds a live native session
   should say so — and say which one will serve the session — instead of letting
   precedence decide invisibly.
2. **Make the shim the sanctioned launch path for non-native profiles**, so a
   gateway session is started by name and not by an environment a user assembled.
3. **Treat "works on the native backend" as a property to be re-established per
   backend**, the way approval modes now are.

### It reframes the `/models` question, and better than the alternative

Phase 9G left Codex's `GET /models` refused, because `/models` is a catalogue
endpoint all three protocols define and placing it means choosing a protocol for
a request naming none. The orchestrator's instinct was a routing tie-break.

**That was the wrong layer.** Under this principle the answer is to settle it at
*configuration* time — in what the profile or shim writes into the harness's own
provider configuration — rather than at *gateway routing* time. Codex asks its
configured base URL for a catalogue regardless of how it was configured; a
gateway guessing which protocol an unqualified `/models` meant is a tie-break
invented to paper over a configuration that never said. Fixing it where the
configuration is authored needs no guess at all.

Recorded here rather than acted on: it is a direction that touches `profile`,
`launch` and `shim` together, and it deserves its own batch with its own
evidence rather than being folded into a review.

---

## A guard that outlives its reason blocks the capability it was protecting

**Refined 2026-08-26**, integrating Phase 9H.

Phase 9G's `gateway_upstream` refused any configuration in which more than one
provider served the gateway ingress. The refusal was correct when written:
choosing between providers is sticky routing, sticky routing was Phase 9H, and
9H did not exist. Refusing beat choosing silently.

Phase 9H now exists, and the same refusal would have made **every one of its
fourteen lines unreachable by construction** — failover cannot be exercised in a
configuration that is rejected for having something to fail over to. A user with
two configured routers could not start a gateway-backed session at all.

**The rule.** When a guard exists because *no phase owns a decision yet*, it is
a placeholder, and the phase that takes ownership retires it. Retiring it is not
overriding the earlier decision; leaving it in place is quietly converting a
placeholder into a permanent block. The test is what the guard was objecting to:
9G objected to a **silent** choice, not to a choice. A choice that is announced
in the launch's mechanism notes, pinnable, migratable and recorded on every
change is not the thing it refused.

**And delete the variant.** `GatewayUpstreamRefusal::SeveralProvidersServeTheIngress`
was removed rather than left unconstructed. An error variant nothing can produce
is decoration, and a reader cannot tell decoration from a live guarantee — §20's
"a gate that cannot fail is not a gate", applied to an enum.

**What this does not license.** A guard that encodes a real invariant — a
credential that must not reach a log, a payload that must not be read — is not a
placeholder and no later phase retires it by arriving. The distinguishing
question is whether the guard's own rationale names a *missing owner* or a
*standing harm*.

---

## Supervision is about processes, and it reports rather than reaps

**Added 2026-08-26** with Phase 10A, on the user's sign-off.

Phase 10 models a session as a **record**: harness, profile, backend, state,
presentation, last activity. Nothing in it describes a *process that is alive
and no longer owned*, and that turned out to be the condition that actually
hurts. Five runaway processes were found on the user's machine at 501% CPU,
three of them Glasshouse sessions that had outlived the pane that started them
by nineteen hours. Glasshouse could not see them and would have started more
beside them.

**Why a separate phase rather than more lines in Phase 10.** Adoption,
identity verification and quarantine are about operating-system processes;
Phase 10 is about what Glasshouse remembers. Filed together, the process
concerns become an afterthought inside a phase whose subject is metadata — and
the distinction is load-bearing, because a record can be wrong about a process
in ways a record cannot detect from itself.

**Identity is a pair, never a process id.** A process id alone is reused, so a
stale record eventually matches a stranger — and the stranger is then adopted,
signalled, or reported as the user's session. Recording the start time
alongside it makes the match falsifiable.

**The reset condition on restart is the whole design.** A consecutive-restart
counter reset when a session is *started* turns a crash loop into an infinite
one. It resets only on *verified healthy*.

**Three limits, deliberately.**

1. **Glasshouse never adopts a process it did not start.** Supervision is
   scoped to this project's own records. A control plane that reaches for
   unrelated processes is a different and much more dangerous product.
2. **Quarantine reports and refuses; it never reaps.** Ending a session is the
   user's decision. Something alive and unaccounted for is precisely the case
   where Glasshouse understands least, and killing what you do not understand
   is worse than saying so.
3. **No daemon.** V1 remains a local terminal control plane. This is
   supervision inside a running Glasshouse, not a background service — the same
   line Phase 55 already draws.

**Owed to prior art, and read for architecture only.** The lifecycle shape —
adopt-or-replace on discovery, quarantine on identity change, readiness bounds,
bounded respawn, one ordered path for state changes — comes from reading a
publicly posted reconstruction of another vendor's agent coordinator. That
repository carries no license; nothing was copied from it and nothing should be.

---

## A source-scanning guard reads by lines, so it cannot be blinded by line endings

Several invariants in this codebase are enforced by scanning Glasshouse's own
source: that `LifecycleEvent::TurnEnded` is minted in exactly one production
function, that a hook handler never reads its payload. A guard like that is only
as good as its ability to see the file.

**A multi-line literal search finds nothing on a checkout where Git converted
line endings, and finds nothing silently.** The guard passes, reports success,
and has scanned nothing — the worst failure available to a check, because it is
indistinguishable from the invariant holding.

So every source guard scans with `str::lines`, which strips a carriage return on
the way past. The scan is blind to line endings **by construction rather than by
anyone remembering**, and each guard is exercised against a CRLF copy of its own
input so the property is tested rather than asserted.

This cost a red Windows job to learn, at a time when the Windows runner was the
only thing that could have shown it.

**The related trap, in the same family.** `production_code` cuts a module at
`#[cfg(test)] mod tests`, not at the first `#[cfg(test)]` attribute.
`session/runtime.rs` carries a `#[cfg(test)] const` two hundred lines in, so
cutting at the first attribute scanned a fifth of the file and silently exempted
the rest — including the exit path, which is exactly where a forbidden inference
would be written. A planted violation survived the scan. Anchor on the attribute
that actually introduces the test module.

---

## A pseudo-terminal child's exit is observable before its output is

`waitpid` reports that a child has gone. Its last output still has to cross the
pseudo-terminal and be copied into the session's scrollback by a **different
thread**, which under load may not have run yet.

**So any observer that asks "has it exited?" and then "what did it say?" in the
same breath can see an empty buffer from a child that definitely spoke.** The
window is small — measured at 1.1ms to 2.2ms beside a full test suite on Linux —
and it is wide enough to be hit at random, which is how it presented: a gate
failing on two different tests for weeks, at 8 runs in 17.

`crash_report` therefore **waits to be woken** by the reader thread rather than
sleeping and looking again. The wait is bounded at 250ms, deliberately the same
bound `session::attach` allows its own output pump, because on Windows no
end-of-file ever arrives while the pseudo-terminal is open and nothing else would
end the wait.

**What this is not.** Glasshouse never lost a crashed harness's output: 600
trials on Linux, with the child reaped before the first read, lost zero bytes —
the reader is handed everything that was written and *then* told the far end has
gone. The defect was in reporting the output as absent when asked inside the
window, which is a smaller thing than it looked and worth stating so nobody
"fixes" the drain path again.

## Glasshouse has no async runtime, and the capability map has not noticed

Phase 0's dependency box grants "async execution" as a permitted category.
Nothing in the tree uses it: there is no `tokio`, `async-std` or `smol` at any
depth. Concurrency is threads and `mpsc` channels — the provider probe, the pty
reader, and the event source are all plain threads, and the local gateway is a
synchronous listener on an ephemeral port tied to one process's lifetime.

This is worth stating because it is invisible from the map, which reads as
though an async runtime were expected, and because it is load-bearing for
several other decisions already recorded here: a pseudo-terminal child's exit
being observable before its output is, and a terminal hangup being detectable
with a blocking `poll(2)`, are both facts about a threaded synchronous program.
An async rewrite would not inherit those observations.

Found while writing Phase 0's evidence entry, which is the first time anyone
enumerated the dependency tree against what the map permits.

## Harness-model pairing — `unknown` is an answer, and vendor alignment is only ever a prior

### The conflict

Phase 9J asks Glasshouse to say what the relationship between a harness and a
model *is*, and the map's first fixed architectural requirement immediately
takes the obvious use away from the answer: *"Vendor alignment is an
inspectable positive initial soft prior, never proof of quality or a hard
routing requirement."* So the taxonomy has to be precise enough for a router to
lean on and weak enough that leaning on it is never conclusive.

The failure mode is already documented one level up. `harness::Vendor`'s own
comment says that collapsing "who publishes the harness", "who developed the
model" and "who serves it" into one field "is how a router ends up believing a
harness and a model are first-party partners because their names rhyme." Phase
9J is the same failure one level down: a model called `claude-something` on a
provider called `anthropic` is two names, and neither is an attribution.

### The decision: the developer is a different type from the vendor, and they meet in one table

`ModelDeveloper` is not `Vendor` and there is no conversion between them, so a
value of one cannot be assigned to the other by accident. They are compared in
exactly one function, `pairing::vendor_organisation`, which is a declared table
with a citation per entry and returns `None` for four of the seven harness
publishers — Cursor, OpenCode, Pi and Hermes — because nothing their
installations expose names a model that vendor developed. Those four can
therefore never produce a vendor-native pairing, which is a missing capability
rather than an invented one.

`ModelDeveloper::Named` carries a slug rather than a variant per company. An
enum of organisations would have made "a user can correct pairing metadata
without changing router code" false for any developer Glasshouse had not
anticipated, and it would have meant this module inventing a list of companies
from recollection — the one thing the rest of `harness/` refuses to do.

### The decision: `unknown` is a first-class answer, reachable from the front of the ladder

`PairingClass::Unknown` is what a stealth or insufficiently attributed model
gets *even when its wire protocol is perfectly ordinary*. A model nothing
attributes, reached over Claude Code's own Anthropic Messages endpoint, reports
`class: unknown` and `protocol fit: native` — two lines, two answers, neither
lying. The attribution comes from an exact-id lookup and nothing else: a
`vendor/model` routing prefix is not stripped, a family is not inferred from a
common stem, and the serving provider is never consulted, because every one of
those is reading a developer out of a name.

The same rule catches the quietest case, which is a launch profile that names
no model at all. Glasshouse assigned none, so it knows none — and the fact that
the harness is Anthropic's program is not evidence about the model Anthropic's
program will pick.

### The decision: vendor-supported does not require a known developer

Vendor-native needs both halves — the vendor declares the family as one of its
own **and** the developer is that vendor's organisation. Vendor-supported needs
only the vendor's own list, because it is a claim by the harness vendor and
stands whether or not anyone can say who wrote the weights. Antigravity's own
`agy models` lists `gpt-oss-120b-medium`; Glasshouse reports that pairing as
`vendor-supported` with `developer: unknown`, and the two answers are
independent on purpose.

### The decision: three compatibility axes, three types

Protocol compatibility, model-behaviour compatibility and tool-semantic
compatibility are three separate fields of three types that cannot substitute
for one another — `ProtocolFit`, `ModelBehaviourFit`, and the existing
`routing::ToolSemantics`. They disagree in practice: every built-in provider
template declares tool calls `Unverified`, so a pairing on a harness's own wire
is `native` on one axis and unestablished on the other two. A single
"compatible" verdict could not say that, and a build in which a native protocol
quietly verified the other two is killed by
`the_three_compatibility_axes_are_answered_separately`.

`ModelBehaviourFit` is `Unverified` for every catalogued model, and that is not
an oversight: nothing in Glasshouse observes whether a model behaves the way a
harness needs. Phase 33A's evidence ledger is what would feed it; until then the
only thing that moves it is a person who has run the pairing and found out.

### The decision: the metadata is declarative on both sides

A harness adding official support for a model is one string in one array inside
the adapter that already owns every other fact about that harness, and every
such array cites the artifact it was read from — `agy models` for Antigravity,
`claude --help` for Claude Code, the `[tui.model_availability_nux]` table Codex
wrote into its own configuration. A *user* correcting metadata writes a
`[pairing.models."<id>"]` table in their own configuration file, layered
project-over-user like every other lookup, and `glasshouse pairing` names the
layer each correction came from so a surprising verdict can be traced to the
file that caused it. Neither path is code a router reads.

A correction may set the developer, the family and the observed behaviour. It
may **not** set the class. The class is always derived, so "why does this say
vendor-native" always has an answer made of things somebody declared — which is
the whole point of a taxonomy whose top rung is a claim about a first-party
relationship.

### The unreachable rung, and why it is there

`ProtocolTranslated` is representable and cannot be produced today, because
`provider::translation_available` answers `false` for every pair and V1 prefers
pass-through. The classifier still *asks* that seam rather than assuming the
answer, and `the_classifier_asks_the_one_function_that_owns_translation` fails
on a build that stops asking — so the first concrete translation adapter anyone
adds is reflected in the taxonomy without a second edit.

### Invariants a test must hold to

1. A model nothing attributes is `unknown`, whatever its name, its provider's
   name, or the wire it travels over.
2. The serving provider never answers "who developed this".
3. Vendor-native requires the declared family *and* the vendor's own
   organisation; a family name alone is not enough.
4. Vendor-supported stands without a known developer.
5. Protocol fit, model behaviour and tool semantics are three answers, and one
   never sets another.
6. A correction in a configuration file changes what the binary prints, and
   nothing in a router changes with it.


## Response profiles — five axes, and a floor no axis can lower

### The conflict

Phase 9K asks for communication policy that a user can dial down, and its
second fixed architectural requirement immediately forbids the obvious way to
implement it: profiles *"must not use concision to suppress diagnostics,
evidence, or verification"*. So the type has to be expressive enough to make an
answer genuinely terse and constrained enough that terse can never mean
under-reported. Written as a sentence in a prompt, that constraint is one the
terse setting itself can argue with.

### The decision: five axes, and independence that a compiler holds

Verbosity, audience, progress narration, evidence presentation and
final-answer format are five types with no conversion between them, five
private fields on `ResponseProfile`, and five directive functions each of which
takes only its own axis's type as its only argument. `narration_directive`
*cannot* consult verbosity, because it was never given one. Phase 9J's three
compatibility axes are the precedent and the mutation is the same shape: a
build where terse quietly implies silent, or concise implies minimal evidence,
is killed by `the_five_dimensions_are_independent`.

The independence is per-*layer* as well as per-type. Precedence resolves one
axis at a time down line 596's six layers, so a project that wants silent
narration and nothing else records one key and inherits four. A whole-profile
precedence would have made "set one thing" mean "restate five".

### The decision: the floor is a constant, not a sentence

`REQUIRED_REPORTS` — changed files, verification, risks, blockers — is a
`const`. `ResponseProfile::required_reports` returns it and takes `&self`
without reading it. `directives()` appends `floor_directive()` unconditionally,
and `floor_directive` also states the standing prohibition in the map's own
terms. There is no code path by which any of the 324 axis combinations reduces
it, and the test enumerates all 324 rather than sampling. The concise-technical
preset's third clause is therefore not a property of that preset at all: every
preset carries it, because no preset can do otherwise.

### The decision: the bottom of the chain is the harness untouched

Line 596 ends at "harness default", and the honest reading is that an
unconfigured Glasshouse applies *nothing*. `ResolvedProfile::is_harness_default`
is true when every axis reached the bottom, and `apply` then produces
`NotApplied` with a reason. This is load-bearing in a second way: the role
layer's *built-in* default applies only when a person named a role, because a
role layer that always answered all five axes would leave line 596's bottom
three layers structurally unreachable.

### The decision: prefer the harness's own mechanism, and let the adapter judge it

`apply` asks for a native mechanism, then an additive one, then refuses. Line
601's *"without weakening coding instructions"* is decided inside the adapter,
which is the only thing that knows its own styles. Claude Code 2.1.247 declares
four built-in output styles, all four keeping its coding instructions, and only
two of them are communication policy: `Learning` sets exercises and `Proactive`
prefers action over planning, and either would break the phase's first fixed
requirement. Both are recorded in the adapter's table with the harness's own
description beside them, so "Glasshouse never selects these two" is a fact a
reader can check rather than an absence.

### The decision: three applied categories, and no fourth

`AppliedMechanism` is `Native`, `Additive`, or `NotApplied`. **There is no
variant meaning "replaced".** Nothing downstream can record replacing a native
system prompt because nothing upstream can perform one — the only things
produced are a settings key the harness documents and arguments an adapter
declared as additive. Claude Code makes the hazard concrete: `claude --help`
documents `--system-prompt` (replaces) beside `--append-system-prompt`
(appends), and only the second is declared.

`NotApplied` always carries a reason, and two very different situations reach
it: nobody asked, or this harness declares nothing Glasshouse may use. Six of
seven harnesses are in the second state today, which is the honest state of the
declarations rather than a gap.

### The consequence: one harness reads one document

Probed on Claude Code 2.1.247: `claude --settings A --settings B` honours only
`B`. A second `--settings` does not merge and does not error — it discards the
first. A response profile that appended its own would have turned off every
lifecycle hook in the session, silently. So `install_session_document` writes at
most one document per file name and emits at most one flag pointing at it,
merging the profile's keys into the hook document when both name the same file.
That function returns `hooks_installed` beside its arguments because Codex
installs hooks successfully and contributes *no* arguments, and inferring
installation from a non-empty argument list reported that as a failure.

### The consequence: one representation of a resolved value

`ResolvedProfile` stores the profile once and derives its per-axis report from
it. An earlier shape stored the values a second time beside the sources, and a
surviving mutation showed the cost: the report printed the stored value while
the harness got the mutated one, and nothing could tell. Where a value has two
homes, a defect gets to live in the gap.

### Invariants a test must hold to

1. Every one of the 324 axis combinations reports changed files, verification,
   risks and blockers.
2. No axis's value can be read out of, or set by, another axis.
3. Each of line 596's six layers can win, and does when the ones above it are
   silent.
4. A project's response profile never reaches another project.
5. The recorded mechanism is the mechanism that reached the child.
6. No launch Glasshouse composes replaces a harness's own system prompt.
7. Whenever Glasshouse writes instruction text of its own, that text carries
   the floor.

## A stored vocabulary and a live one are two vocabularies

`session::store` records a session's pairing class, its wire protocol and the
mechanism that carried its response profile, and it records each as its own
type — `SessionPairingClass`, `SessionProtocol`, `ResponseMechanism` — rather
than as `harness::pairing::PairingClass`, `harness::WireProtocol` and
`harness::response::AppliedMechanism`. The immediate reason is Phase 6 line
294: the session model may not depend on a harness adapter, and a source scan
enforces it by forbidding `crate::harness` in that file.

The durable reason is better than the immediate one. A schema's `CHECK` fixes
the words a row may hold, and a row written by an older build has to stay
readable after the live enum grows. If the two were one type, adding a seventh
`PairingClass` would compile everywhere and then fail as a constraint violation
on whichever background write happened to carry it — the failure mode
`LIFECYCLE_EVENT_KINDS` already exists to prevent one table over. Keeping them
apart puts exactly one total, exhaustive function between them, in
`session/mod.rs`, so a new variant is a compile error at the single place
somebody has to decide what it means on disk. The cost is three small
conversion functions; the thing bought is that the decision cannot be skipped.

## NULL means "not recorded", so anything else needs its own word

Every column migration 8 adds is nullable, and NULL means one thing throughout
the `sessions` table: *the build that wrote this row recorded nothing here*.
That is the rule `launch_profile` established in migration 3, where NULL is a
session that ran before profiles existed and `'native'` is a session that ran
the Native profile.

Applying it consistently forced two designs. `pairing_class` and `protocol`
both have a real answer meaning "Glasshouse established nothing", so both
store the word `unknown` and NULL stays available for "not recorded". `model`
is the harder one: *"Glasshouse assigned no model, so the harness chose"* is a
recorded answer, and a column holding a bare model id would have had one empty
slot for it and for "never recorded". So the column stores `harness-default` or
`named:<id>`, and the prefix is what makes the two impossible to confuse
however a model is named — including a model whose id is literally
`harness-default`, which round-trips correctly and is tested.

## Renaming a session is not something the session did

`glasshouse sessions` is ordered by `last_activity_at`, and that column answers
one question: when did this session last run. Renaming, tagging and closing a
record are things the *user* did to Glasshouse's bookkeeping, so none of them
touches it. The alternative — treating any write as activity, which is what
`set_lifecycle` does and is right to do — would let naming a month-old session
push it to the top of a list of what ran most recently, and the list would stop
answering its question.

## Closing a record is not deleting a conversation

`glasshouse sessions close` writes one column and says out loud what it did not
do: the harness's own session files are not read, not moved and not deleted,
and the native session identifier stays recorded so the history remains
findable. Glasshouse has never parsed or owned those files and retiring its own
record is not an occasion to start. The map reserves the possibility of
deleting them *"unless explicitly requested"*; nothing in this batch is such a
request, and until something is, the reassurance is printed rather than
implied. A live session is refused rather than closed underneath itself — a
`closed` row that a running harness keeps updating is worse than an error
telling the user to stop it first.


## A provider's model list is a catalogue, not an entitlement list

Glasshouse asks a provider `GET /models` to learn what it offers, and every
surface that shows a model to a user is built on that answer. **The list says
what exists. It does not say what this credential may invoke**, and on some
hosts the two differ completely.

Measured on 2026-08-27 against fourteen providers with real credentials, and
re-run independently before being written here:

| host | catalogue | this account could invoke | how the refusal arrives |
|---|---|---|---|
| NVIDIA | 84 models | **none of them** | `404` — `Function '<uuid>': Not found for account '<id>'` |
| Nous | 372 models | none — balance too low | `404` — `requires available credits` |
| Cerebras | 2 models | none — unpaid | `402 payment_required` |
| DeepSeek | 3 models | none — unpaid | `402 payment_required` |

**The same condition arrives as three different status codes, and one status
code carries three different conditions.** Nous alone answers `404` both for a
model that genuinely does not exist — *"does not exist in our configuration or
OpenRouter catalog"* — and for one it lists and this account cannot afford —
*"requires available credits. Your account balance is too low"*. Both were
observed from this repository within a minute of each other.

### What this requires of Glasshouse

**Any component that decides whether a model is usable must classify on status
*and* body, never on status alone.** A router that reads status codes only will:

- treat NVIDIA's and Nous's *entitlement and billing* conditions as
  *"model does not exist"*, and may retire a model permanently that would work
  after a top-up;
- treat Cerebras's and DeepSeek's identical condition as retryable, because
  `402` is the honest code;
- and be unable to tell, on Nous, a real absence from a temporary one at all.

This binds the routing prior (Phase 35B) and the free-pool router (Phase 21B)
before either is written, and it is the reason a catalogue entry may not be
promoted to "available" by its presence in the list. **Reachability and
entitlement are different claims** — the same distinction an unauthenticated
probe already forces, one layer further in.

A model identifier is not evidence either: on one gateway an id containing
`free` was refused as a paid tier while a sibling served for nothing on the
same credential. Whether a model costs this account anything is a property of
the account, not of the name.

### And an error body is untrusted output that may name the account

NVIDIA's refusal quotes an account identifier back; two other hosts quote a
masked tail of the submitted credential. **A provider's error body must be
treated as sensitive by default**: classified against, and never copied whole
into a log, a diagnostic, a session record, or anything a user might share.
That is the same rule already applied to the request side, applied to the
response.

### The mirror: a `2xx` can mean "you did not authenticate"

The table above is about failure codes *understating* — a `404` that means "you
have no credit". **The same host family supplies the opposite, and it is the
worse direction because it fails silently.**

z.ai answers an unauthenticated `GET /api/paas/v9/models` with **HTTP `200`**
and a body of `{"code":1001,"msg":"Authentication parameter not received in
Header, unable to authenticate","success":false}`. The mechanism is two auth
gates with two error schemas: the recognised `/v4/` prefix meets the current
gate and answers `401` with a documented `{"error":{…}}` envelope, while an
unrecognised version prefix falls through to older middleware that reports the
identical failure as a success code. Measured on 2026-08-27 against both
`api.z.ai` and `open.bigmodel.cn`, unauthenticated and authenticated.

**A provider health check that asserts `status == 200` would mark this provider
reachable and healthy with no credential at all.** That is not a hypothetical
— it is the check anyone writes first.

So the rule the table states has to hold in both directions, and the 2xx
direction needs one addition: **compare the body's *schema* against the one that
host documents for success.** A `200` carrying `success: false`, or an envelope
that does not match the shape a successful call returns, is a failure regardless
of its status line.

### The rule that governs all of the above

**A status code is a claim by whichever layer answered, and an unrecognised path
may be answered by a different layer than the one whose contract you read.**

That one sentence explains every row in this section, which is why it belongs
above the others rather than beside them:

- **z.ai** — an unrecognised version prefix falls through to older middleware
  with its own error vocabulary, so two paths on one host disagree about how to
  spell the same failure;
- **NVIDIA** — the `404` comes from a per-account function router that resolved
  the model to an internal id this account cannot reach, not from a path that
  does not exist;
- **Nous** — the same `404` is emitted by two different checks, catalogue
  lookup and billing, which is why the status separates nothing.

The actionable form is the schema comparison above: **a response whose envelope
does not match the shape the host documents for success is the tell that you are
talking to a layer you did not mean to reach.**

### A prediction, labelled as one: this is gateway behaviour, not host behaviour

`open.bigmodel.cn` — Zhipu AI's China platform, a different host from the
`api.z.ai` international brand — answers the same three probes with the same
status pattern, the same error number `1001`, and the same pair of envelope
schemas, differing only in the language of the message. Measured, both hosts,
2026-08-27.

**Two independently addressed hosts behaving identically is evidence the
behaviour belongs to the shared gateway rather than to either deployment.** That
is an *inference and not a measurement*, and it is recorded as one because it is
useful and falsifiable: any other endpoint fronted by the same gateway — a
regional host, a partner deployment, a future base URL a user configures — should
be expected to answer `200` to an authentication failure on an unrecognised path
prefix.

This matters to Glasshouse specifically because providers are templated by base
URL. **A z.ai-templated provider pointed at a sibling host inherits the
prediction, not a fresh unknown**, so a probe that establishes reachability
against one of these hosts should not be assumed to have established it against
another. The falsifier is cheap and named: the `{code, msg, success}` envelope
where the documented success shape is `{object, data}`.

### And a `200` with empty content is not a dead provider either

A one-token liveness probe is the natural cheap health check and it does not
survive contact with a reasoning model. `glm-4.6` with `max_tokens: 8` returns
HTTP `200`, `finish_reason: "length"`, `content: ""`, and all eight completion
tokens accounted for as `reasoning_tokens` — the budget was spent thinking and
none of it reached the visible message.

**So empty content is not evidence of a broken provider**, and a check that
treats it as such will report healthy models as dead. A liveness probe against a
model that may reason must either allow enough budget for the model to finish
thinking and still speak, or treat `finish_reason: "length"` with non-zero
`reasoning_tokens` as a success — the request was served, which is what liveness
asks.

Taken together with the entitlement table: **`200` can be a failure, `404` can
be a billing state, and empty content can be a success.** No single field of an
HTTP response is load-bearing on its own, which is the whole design constraint
this section exists to record.

## Metered capacity for background jobs — permitted, and bounded by proportion

### The conflict

Glasshouse runs bounded internal jobs of its own — memory extraction, task
classification, reranking. Phase 9I line 533 says to prefer free models for
them. Phase 9I line 542 says Glasshouse's own automated evaluation and test runs
must *"never [use] a metered resource without an explicit opt-in."*

Neither line answers the question the router actually faces: **when no free
resource can serve an ordinary support job, may it spend paid quota?** Until it
is answered, `main.rs::disposable_candidates` builds only `Cost::Free`
candidates, `routing/disposable.rs`'s metered branch is unreachable, and two map
lines (1293, 1550) cannot close because the mechanism behind them can never run.

### The decision: yes, and the spend must be small relative to the task it serves

**A background job may use metered quota.** The user's decision, 2026-08-28,
recorded verbatim because the qualifier is the substance:

> *"yes a background job may use quota. But this has to be not much in contrast
> to the actual task."*

So the permission is real and the constraint is a **proportion, not a ceiling**.
The comparison is to the work the job supports, not to a monthly budget. A
classification that costs a fraction of the turn it routes is in scope; one that
costs a material share of it is not, however small its absolute price.

### What this does and does not authorise

- **Ordinary support work** may fall back to a metered resource. This is
  `MeteredUse::Permitted`, which already exists and whose own doc comment
  already describes it as *"a legitimate last resort"* — free capacity first, in
  line with line 533; metered only when no free resource can serve.
- **Glasshouse's own automated evaluation and test runs are unchanged.** They
  stay `MeteredUse::Withheld` unless `GLASSHOUSE_ALLOW_METERED_MODELS=1` is set.
  Line 542 is already COMPLETE and this decision must not regress it — the two
  are different callers with different answers, and collapsing them would make
  "explicit opt-in" indistinguishable from a default, which is the exact wording
  `MeteredUse`'s own doc warns against.
- **Nothing here authorises spending premium interactive capacity on bookkeeping.**
  Map line 1611 and the map's *Product Rules* list still stand — *"Free resources
  should be used aggressively for suitable work but never treated as equivalent
  merely because their monetary price is zero"* and *"A cheap routing model
  should protect premium capacity only when the routing overhead is materially
  smaller than the resources it saves"* (cited by name: the list's line number
  moves every time a phase is inserted above it, and this citation had rotted
  twice).

### The sub-question this leaves genuinely open, and it must not be faked

Enforcing *"not much in contrast to the actual task"* literally requires
comparing a disposable job's expected cost against the task's — and **Glasshouse
cannot estimate either today**: Phase 32G (provider-aware request-cost
estimation) is 0/10, and map line 1305 explicitly says to *"treat unknown
pricing as unknown instead of assigning a fake zero cost."*

So the ratio is not computable yet, and inventing one from unavailable data
would be precisely the failure this project keeps recording. The implementable
reading of the constraint, using only what exists:

1. free capacity is always preferred (line 533, already COMPLETE);
2. metered is a **last resort**, reached only when no free resource can serve;
3. the job stays bounded — a disposable job is bounded by definition (Phase 39);
4. the fallback is **inspectable**: the routing explanation says a metered
   resource was chosen and why, which is what map lines 1293 and 543 ask for.

When Phase 32G exists, the proportion becomes measurable and this decision should
be revisited against it rather than re-derived. Recorded as the validity
condition: **this reading holds while cost estimation does not exist.**

### Invariants a test must hold to

- With free capacity available, a metered candidate is never chosen.
- With no free capacity and metered permitted, a metered candidate is chosen and
  the explanation names the reserve decision and the reason (lines 1293, 1550).
- An automated Glasshouse run with `GLASSHOUSE_ALLOW_METERED_MODELS` unset
  chooses nothing metered and fails instead — line 542, unchanged.
- A stray value in that variable leaves metered use withheld: the fail-closed
  direction, already implemented in `MeteredUse::for_automated_run`.

## Negotiable by default, and built to be measured — the alpha's shape

### The decision

**Most of Glasshouse's behaviour should be optional, and every optional behaviour
should be built so its usefulness can be measured.** The user's words, 2026-08-28:

> *"not every feature needs to be optional, some are just what glasshouse is and
> can't be negotiated. But I don't think this only applies to some workings of
> it — most of it can and should be. Glasshouse is gonna be early alpha so a lot
> of A/B testing to measure usefulness of feature is needed, which will clarify
> while using it. So negotiable features should be built with this in mind."*

Two claims, and the second is the one that changes how packets are written.

### 1. There is a core, and it is small

A minority of behaviours **are** Glasshouse and are not negotiable. The product
invariants already name them — Glasshouse orchestrates real installed harnesses
and does not hide them; every interactive session is backed by a real native
harness; memory, session state and logs are project-scoped; cross-project access
is disabled structurally; secrets never enter logs, `Debug`, diagnostics,
snapshots, fixtures or commits. **Those are not settings and must never become
settings** — a switch that can turn off project isolation is a defect, not a
feature.

**Everything outside that core is presumed negotiable.** The default assumption
for a new behaviour is *optional*, and making something mandatory is the choice
that now needs an argument.

### 2. Negotiable means switchable **and observable**

*"Built with A/B testing in mind"* is the operative half, and it is a
construction requirement, not a documentation one. A negotiable behaviour must:

- **be switchable cleanly**, so the off state is a real alternative rather than a
  degraded one — turning it off must produce coherent behaviour a user could
  live with, because that is the control arm of the comparison;
- **default to the behaviour the product wants**, not to off. A default of off is
  not neutral; it is a decision that the feature is not worth having, and it will
  silently win every comparison nobody runs;
- **make its effect observable** — the decision it changed must be legible
  somewhere a person or a metric can see it. A behaviour whose only evidence is
  that it ran has nothing to A/B.

**This is why `Contribution` and `RoutingExplanation` matter beyond their own
boxes.** A routing signal that pushes a named contribution with an honest reason
is measurable by construction; one that quietly adjusts a number is not.

### What it does NOT authorise

**It does not authorise building A/B infrastructure ad hoc.** Measurement is
**Phase 51 (Evaluation hooks), 0 of 37**, and every one of its lines is a
*"measure how often…"* question this decision now makes strategically important:
how often retrieved memory is actually useful, how often stale memory is
retrieved, how often automatic routing is overridden by the user, how often
warm-session reuse is chosen over a fresh session. **A package makes its own
behaviour switchable and observable; it does not invent a second telemetry
system beside Phase 51's.**

It also does not authorise a setting for everything. A setting is a promise to
maintain both paths forever, and Phase 21H exists to discourage exactly the
speculative variation that produces. **Optional by default is a bias, not a
mandate** — if a behaviour has no plausible off state a user would want, say so
and make it mandatory.

### Consequence for how packets are written

A packet for a negotiable behaviour should state, explicitly:

1. whether the behaviour is core or negotiable, and why;
2. if negotiable, what its **default** is and why that matches the product's
   intent — not what is safest to ship;
3. how the behaviour's effect is **observable** to someone measuring it later.

### Where this came from, and the mistake it corrects

An orchestrator proposed gating metered quota behind a switch defaulting to
**off**, and justified it as satisfying two readings of an earlier decision at
once. The user rejected it: *"you can't just build something conflicting."* The
default was the defect — the recorded decision was that a background job **may**
use quota, and an off default ships the opposite while claiming to honour it.
**A hedge between a real instruction and an invented alternative is not neutral;
it silently picks the invented one.**


## The A/B directive needs a capability Glasshouse does not have: counting events over time

### The finding

A read-only recon gated **Phase 51 (Evaluation hooks) — all 37 lines — and found
every one of them BLOCKED.** Not one is closable today, and they fail for the
same reason.

> **Glasshouse cannot count occurrences of a decision or an effect over time.**

Its schema is built almost entirely for **current state** — a memory's status, a
session's mechanism, a routing row you can look up *if you already know its
identity*. Phase 51 asks exclusively for **event counts**: how often routing
picked the cheap route, how often a user overrode automation, how often a
response profile changed behaviour, how often memory actually helped or hurt.
**None of those is a lookup of current state.**

### Why this matters more than 37 boxes

The recorded alpha directive is that negotiable features be built so their
usefulness can be **measured**, because early alpha needs A/B comparison. Every
such comparison — feature on versus feature off — is an event count. So the
directive and the phase that serves it are blocked on the same missing
primitive, and no amount of per-feature switchability substitutes for it.

**What can be measured today, honestly:** task success rate and sample
count/freshness per `(provider, model, route, harness)` identity via
`EvidenceLedger::summarize`; quota and capacity bands with reset timing via
`GatewayQuotaCache`; per-session response-mechanism and pairing-class
categorisation. That is enough to ask *"did this identity's success rate move?"*
**by re-running `summarize()` manually before and after a change** — a real
comparison, but a hand-driven one, not an experiment the product runs.

### The cheapest way in, and it is not a migration

**Populate the fields the schema already defines and the writer never sets.**
`NewObservation` / `ObservedEvidence` define `purpose`, `cost`, and the
`first_byte_at` / `first_token_at` / `first_tool_call_at` timestamps; the single
production writer, `gateway/session.rs::record_routing_observation`, sets none of
them. Adding `with_purpose` / `with_cost` builders and threading real per-exchange
data through the one call site that already builds every other field is
**caller-side work against an already-designed schema**, and it unblocks or
materially advances six Phase 51 lines.

The memory cluster is the expensive one: it needs **a new event-log table**, which
is a migration and Red tier. It should be scoped and decided deliberately rather
than arrived at by a package that discovers it mid-flight.

### What this does not change

**It does not make the negotiable-by-default decision wrong or premature.** A
feature still has to be switchable before its effect can be compared, and the
switch is cheap. What this records is that switchability is **half** of that
decision's requirement, and the observability half currently bottoms out in a
primitive the product lacks — so a packet claiming a feature is "built with A/B
in mind" today can honestly promise a clean off state and legible per-decision
output, and cannot yet promise a count.

## Probing costs a request budget, so the cheapest probe is the one nobody runs

### The conflict

Map line 1323 — *"Avoid background probing at an aggressive rate that wastes
free-request pools"* — was proposed for closure on a source-scanning test and
declined twice. The reason it kept failing is that the property holds today for
the wrong reason: **nothing probes automatically at all.** Every probe path in
`src/` is user-invoked — the CLI `--probe <name>` flag (`main.rs:2236-2271`,
opt-in by its own doc) and the settings-screen `t` key
(`shell/state.rs:1100 begin_provider_test`). There is no periodic trigger, no
timer, and no rate guard, because there is nothing to guard.

So the line could be read two ways, and the two want different code: *"no
automatic probing exists"* (true, and closable by rewording, which is what line
1748 did) or *"probing is rate-limited when it happens"* (nothing enforces it).
That is a map decision, not an engineering one, so it went to the user.

### The decision: the line stays open, and the reason is a budget

The user's answer, 2026-08-29, recorded because the reasoning is the substance:

> *"If there ever happens any probing and it would hit a free endpoint like
> openrouter it would use a request budget — so best is to not probe at all if
> not necessary and if necessary only sporadically. And if we want model data
> maybe the providers have data endpoints and measured availability for us
> instead of doing it ourselves."*

**This is not the rewording answer, and it is deliberately not.** Line 1748 was
reworded because its property was genuinely structural — physical state
separation holds whoever deletes what. This one is not structural: it holds only
because a feature is absent, and the moment a background prober is written the
guarantee evaporates silently. Leaving the line open keeps the requirement
pointed at the code that will need it.

### What the decision actually says, in three parts

1. **A probe against a free endpoint spends a request from a scarce pool.** That
   is the cost, and it is the reason the line exists. `FreePool`'s `Allowance`
   already models a per-credential request pool, so the budget being spent is one
   Glasshouse can already name — it simply has no probe consuming it yet.
2. **Do not probe unless necessary; if necessary, only sporadically.** The
   ordering is a preference with a default: *not at all* is the default, *rarely*
   is the fallback, and *at a cadence* is not on offer. Any future prober owes a
   justification for existing before it owes a rate.
3. **Prefer the provider's own data over measuring it ourselves.** Where a
   provider publishes model metadata, usage, or availability, read that instead
   of inferring it from traffic Glasshouse generates. This is not a new
   capability — map line 1230 already says *"Read provider usage endpoints when
   they are documented and can be queried without excessive request cost"*, and
   it is ☑. **1323's answer is to route through 1230's path**, and a prober built
   without first checking whether the provider will simply tell us is the defect
   this decision names.

### How this composes with what is already checked

- **Line 537** (☑) — *"Avoid consuming scarce free requests on health probes when
  actual workload can provide health signals"* — is the same principle from the
  other side, and it is already implemented: `FreePool::observe` learns health
  *entirely from work that was going to happen anyway*, and recovery too
  (`WorkloadOutcome::Served` clears the cooldown). **Glasshouse already gets its
  health signal for free.** That is the strongest argument that a prober is
  unnecessary, and it is shipped code rather than an intention.
- **Line 1369** — *"Reduce or suppress active probes when probing would consume a
  material fraction of a scarce request pool"* — is 1323's enforcement half and
  stays open with it. They should close together, against the same prober.
- **Line 1853** — *"Measure how much scarce capacity is consumed by probes and
  whether passive observations can replace them"* — is Phase 51's evaluation of
  exactly this question, and is blocked on the counting primitive.

### What a future package owes

A background prober may not land until it carries, in the same change:

- a stated reason why passive observation from real workload (line 537's
  mechanism, already shipped) is insufficient for the thing being probed;
- a check for a provider-published endpoint first (line 1230);
- a rate or interval guard with a test that **fails when the guard is removed** —
  not a source scan, which proves the scan and not the behaviour;
- the request-pool accounting that shows what each probe costs.

Until then 1323 stays open, and its openness is the record that this is owed.

## Scoping the Phase 51 event log: a split verdict, and most of it is not a migration

### The decision to scope it at all

The previous section records that Phase 51's 37 lines are blocked on one missing
primitive — Glasshouse cannot count occurrences over time — and that the memory
cluster needs a new event-log table. Asked how to handle that, the user chose,
2026-08-29: **scope the migration deliberately**, as a written design before any
code, rather than let a package discover it mid-flight.

This section is that scoping. It is **the brief, not the design**: it fixes the
shape and the constraints so the design is written inside them. It rests on a
read-only survey of `database.rs` end to end, `lifecycle_events`' full schema and
every writer and reader of it, and all 37 lines checked for whether their event is
observable in the shipped binary today.

### The verdict is a split, and the cheap half is genuinely cheap

**Do not fold Phase 51 into `lifecycle_events`, except where it is already there.**

- **Extend `lifecycle_events` with an aggregate read method — no schema change at
  all — for the gateway-health cluster** (map lines 1836, 1837, 1851, 1852).
  `GatewayUnhealthy` and `GatewayBackendChanged` already exist as `kind` values
  and are already written by a real production caller in `gateway/session.rs`.
  What is missing is only a counting *read*: today the only readers are
  `EventLog::all()` and `for_session()`, both full scans. **Four lines, a Rust
  read helper, zero migration.**
- **A new table for the memory-evaluation cluster** (1820-1826, 1831) **and — an
  extension of the earlier finding — the routing-decision cluster** (1834, 1835,
  1845-1850). These are events about *Glasshouse's own decisions*, not about a
  harness's process.

### Why not simply widen `lifecycle_events`, in three parts

1. **Vocabulary mismatch.** All eleven existing `kind` values are things that
   happened to a session's process or its harness — a turn, a keystroke, a
   process exit, a backend going unhealthy. Nearly every Phase 51 line is a
   decision *Glasshouse* made or evaluated: a memory retrieved, a route preferred,
   a tier assigned. Folding them in means either one `CHECK` holding two unrelated
   vocabularies, or enough columns that one table carries two schemas.
2. **It is the argument migration 11 already made, one level up.** `database.rs:1042-1052`
   explains why `routing_observations` is its own table rather than columns on
   `sessions`: *"A dedicated table with its own `seq` is migration 4's own argument
   for `lifecycle_events` over a column on `sessions`, applied here for the same
   reason."* The same reasoning applies again — `lifecycle_events` is the wrong
   shape for a second, unrelated kind of counted fact.
3. **The rebuild is not free and the risk is specific.** SQLite cannot `ALTER` a
   `CHECK`, so widening `kind` means a third table rebuild after migrations 6 and
   7 — on the one table `memories.source_event_first`/`_last` reference by `seq`.
   That is precisely the hazard a worker declined to take on in an earlier batch,
   and its reasoning was right then and is right now.

`crate::events`' own module doc scopes the stream to process/harness lifecycle and
says *"nothing downstream ever learns which harness produced one"*, with two tests
(`no_harness_is_named_in_the_core_event_stream`,
`turn_completion_is_minted_in_exactly_one_place`) existing specifically to keep it
narrow. A memory-usefulness event does not belong in it.

### What the design must NOT duplicate

- **`routing_observations` already has the columns for six of these lines.** Cost,
  tool rounds, retries, repairs, failovers and purpose are declared in the schema
  and simply never set by the one producer. **Those are caller-side wiring, not a
  new schema**, and a Phase 51 table that invents its own "routing turn cost"
  column would be a second source of truth for a fact the ledger already models.
  (Note that `purpose` among them is now map line 1330's re-opening — see
  `docs/product/evidence/phase-33a.md`.)
- **`GatewayQuotaCache` is a snapshot, not a history.** It is one JSON file per
  provider holding only the most recent reading, overwritten each time. The
  quota-history lines must route through the new table rather than growing the
  cache a second, competing history mechanism.
- **`EvidenceLedger` covers gateway-forwarded turns only.** Its sole writer is
  inside `gateway/session.rs`, reached only from the accept loop, so a native
  subscription session produces zero rows. It structurally cannot record memory
  operations or session-lifecycle decisions — which is exactly why a new table
  does not overlap it for most of the phase.

### Two lines are blocked on an absent feature, not on counting

Worth knowing before anyone scopes work for them: **`Guardrail` has zero matches
anywhere in `crates/glasshouse/src`**, so map lines 1842 and 1843 have no feature
to instrument — Phase 21K is unbuilt. And `session::recovery` has no production
caller outside its own file, which blocks the cross-harness-resume lines
independently of any event log. **Counting is not their blocker and a Phase 51
table will not unblock them.**

### The order this implies

1. **The four gateway-health lines first** — a read helper over existing rows,
   no migration, and it proves the counting *shape* against real data before any
   schema is committed to.
2. **Then the caller-side wiring** for the fields `routing_observations` already
   declares, which is where map line 1330 also lands.
3. **Then the new table**, designed against what the first two taught, for the
   memory-evaluation and routing-decision clusters.

**Doing 3 first is the expensive mistake this scoping exists to prevent**, because
a table designed before anything counts anything is a table designed against
guesses.

## Convergent co-editing — a third answer to a file two agents need

### The user's proposal, 2026-08-29

> *"lets say 2 agents get 5 files each and one file needs to be touched by both.
> Instead of them actually doing it they could write into some pre-implementation
> buffer and it could be waited until both agents state their work done on that
> file. Agents could see changes made by other agents before they are finished
> with their work and can mutate their changes to fit. As soon as both are done,
> reconcile into the actual file with all features from both agents respected."*

### Why this is a third option, not a restatement of Maybe D/E/F

The experimental sections already hold three answers to a contended file, and
**none of them is this one**:

- **Maybe D — queue.** One agent waits. Correct, and it serializes.
- **Maybe E — reconcile later.** Both proceed blind; the conflict is marked and
  *"the orchestrator receives a reconciliation task after both conflicting
  workers finish."* Reconciliation is **post-hoc**, between two versions that
  never knew about each other.
- **Maybe F — turn-scoped claims.** A finer-grained lock. Still a lock.
- **Maybe G — read visibility.** Warns *readers* that a file is being edited. It
  says nothing about two *writers* adapting to one another.

**The proposal's distinctive claim is mutual visibility *during* the work.** Each
agent can see the other's in-progress change and *mutate its own to fit*, so by
the time anyone reconciles, the two versions already account for each other. That
is **convergent co-editing**, and it is strictly better than post-hoc merge for
one reason: the reconciler inherits two versions built with knowledge of each
other, instead of two versions that must be reconciled by someone who understands
neither as well as their authors did.

### Why Glasshouse specifically can do this, and a single harness cannot

**A harness sees its own edits. Glasshouse sees every session's.** That is not a
feature to be added — it is what the product already is: the session layer, the
lifecycle event bus, and project-scoped state all sit above the harnesses. The
same argument the map already makes for the unified event bus (*"one normalized
core lifecycle-event stream shared by the TUI, router, memory, API and MCP
surfaces"*) applies here. No individual Claude Code or Codex session can offer
this, because none of them can see the others.

### The three hard parts, stated before anyone builds it

**1. The buffer must be compilable, which means it is a worktree, not a patch
file.** An agent whose change lives in a staging buffer cannot compile, test, or
mutation-check it — and for this project that is the agent's entire value. **So
"pre-implementation buffer" must resolve to an isolated working tree**, which
Glasshouse can already create per session. The buffer is not a new storage
concept; it is the isolation the product already needs, plus a merge protocol.

**2. Visibility creates staleness.** If A reads B at T1 and adapts, and B changes
at T2, A's adaptation is stale — the classic convergence problem, and an
unbounded version of it will oscillate. **The cheap discipline that captures most
of the value: read the other's version once, at finalization, not continuously.**
One look, before declaring done. Continuous mutual adaptation is a research
problem; a read-before-finalize barrier is a protocol.

**3. Reconciliation judgement does not disappear — it moves and improves.** Two
agents both adding a parameter to one function still need someone to decide the
final signature. What changes is that the decider *reviews* a proposed merge
instead of *authoring* one from two reports. That is a real saving and it should
be measured as such, not claimed as automation.

### What must not be built

**Not automatic semantic merge.** *"Reconcile with all features from both agents
respected"* is the goal, not the mechanism — and a system that silently produces
a merged file both agents would disown is worse than a queue. The map's existing
rule holds: *"Never silently discard one worker's changes merely because another
worker claimed the file first"* — and its mirror belongs here, **never silently
invent a merge neither worker wrote.** A reconciliation either is confirmed by
both authors, or is escalated with both versions visible.

### Evaluation before promotion, per the experimental sections' own standard

The honest baseline is that a queue already works. So this must be measured
against it: how often two agents genuinely need one file; how often convergent
editing produces a merge both accept without escalation; how much wall-clock the
barrier costs versus queueing; and how often mutual visibility caused an agent to
adapt in a way that turned out wrong. **If it does not beat "queue and serialize"
on real work, it should not ship** — Maybe I's criteria for file coordination
already say this and apply unchanged.

## Phase 51's event log: `evaluation_observations`, migration 15

**Decided 2026-08-29. The full design is `docs/product/design-phase51-event-log.md`;
this is the summary and the corrections it forced to *this* file.**

A **new table**, not a widening of `lifecycle_events` and not a view: `CREATE
TABLE` plus one index plus migration 11's two project-scope triggers. No
`ALTER`, no rebuild, no existing `CHECK` touched — and **no new
`LIFECYCLE_EVENT_KINDS` value**, so map lines 310, 327 and 1316 stay refused on
exactly the ground the register gives them.

`kind` deliberately carries **no SQL `CHECK`**. Its vocabulary lives in Rust
with a pinning test, following `response_profile`'s precedent
(`database.rs:869-875`) rather than `lifecycle_events.kind`'s. Putting a `CHECK`
on the one column certain to grow would manufacture migration 7's problem on
purpose — which is what Cluster G is.

It counts *how often*; `routing_observations` measures *how much*. That is why
there is no `magnitude`/`unit` pair despite four lines wanting one.

**Retention is in the migration, not a follow-up**: 90 days / 100,000 rows,
trimmed in the writer's own transaction, and deliberately without migration 5's
append-only `DELETE` trigger.

**It unblocks 7 of Phase 51's 37 lines and closes 3** (1822, 1826, 1856). Twenty
have no producer and are not schema work. The design bucketed all 37 rather than
claiming the phase.

### Three corrections this design forced to the text above

1. **This document contradicted itself on 1836/1837** — `:2322-2325` put them in
   the zero-migration gateway-health cluster while `:2369-2372` routed the
   quota-history lines through the new table. **Ruling: they belong to the new
   table**, and the step-1 cluster shrinks to 1851 and 1852.
2. **"The only readers are `EventLog::all()` and `for_session()`, both full
   scans" is wrong.** `events/log.rs` has six readers, and `len()` is already a
   SQL `COUNT(*)`. The accurate claim is *"cannot count by kind within a
   window"* — one method signature, not a primitive gap.
3. **1824 does not need the table**; it is answerable from `memories` alone and
   is a read helper, not an event. **1831 does need a producer** the table does
   not supply — a home is not a producer.

### An unrelated defect this surfaced, and it is not Phase 51's

**Nothing in Glasshouse prunes anything.** A grep for `fn prune` / `retention` /
`VACUUM` / `DELETE FROM` across `crates/glasshouse/src` finds no production
retention path. `memories`, `lifecycle_events` and `routing_observations` grow
forever, and `lifecycle_events` **cannot be trimmed even deliberately** — its
`BEFORE DELETE` trigger `RAISE(ABORT)`s (`database.rs:500-512`). That is a
fourth reason not to fold Phase 51 into it, and it is its own piece of work.

---

## Workload tier comes from measured model capability, not from task guesswork
### Decided by the user, 2026-08-30

**The question.** Phase 34D's lines 1457/1459 need a task's workload tier and
confidence to reach a consumer. `classify_heuristically` computes both and
`task_requirements_from_text` discards them, and **nothing in `SessionRouter`
reads a tier today** — so closing those lines meant deciding what a tier should
*do*. Three options were put: leave it blocked, weight destinations by tier
inside `SessionRouter`, or build the schema for a routing model that does not
exist yet.

**The answer was none of the three as framed.** In the user's words:

> a tier should be assigned by model capability and answering quality. this has
> to me be measured long term but i would start by assigning from official
> benchmarks which test how good a model is by tasks and add a classifier
> [that] can determine task at hand and router routes intelligently to model by
> task at hand.

**What this settles.**

1. **A tier is a property of a model, not a guess about a task.** The current
   `WorkloadTier` is a *task requirement* (`classify.rs`'s own doc comment is
   careful about that, and refuses to merge the two scales). What the user is
   asking for is the other half: a **model capability rating**, per task kind.
2. **Seed it from published benchmarks, per task type.** Not one scalar
   "intelligence" — a model that is strong at code edit and weak at long-context
   retrieval must be representable as exactly that. This is Phase **34F**
   ("Model capability and tier calibration") rather than 34D.
3. **Measure long term and let measurement win.** The benchmark seed is a
   starting prior, not the truth. Glasshouse already has the machinery for the
   measured half: `routing_observations` records real outcomes per
   `(provider, model, route, harness)` and `EvidenceLedger::summarize`
   aggregates it — that is the same ledger the project overview now reads.
4. **The router matches task kind to model rating.** That is the
   "routes intelligently to model by task at hand" clause, and it is the
   consumer 1457/1459 were missing.

**What this does NOT authorise.** Building the tier weighting inside
`SessionRouter` as a bare heuristic to close two boxes. That was option 2 and
the user did not take it. The consumer has to be a real capability rating with a
real source, or the lines stay open.

**Where the work goes.** Phase 34F is the home; Phase 34D's 1457/1459 close
behind it. A first package is a model-capability table keyed by
`(model, task kind)` seeded from published benchmarks, with the schema shaped so
the measured half can replace the seed per entry rather than wholesale — because
the seed and the measurement will disagree, and the design has to say which wins
and when.

## A durable observation sink: the user picks the root, not the eighteen leaves

**Decided 2026-08-30**, on an explicit question, after two recons classified 35
open lines across five phases and found exactly **one** reachable.

### The finding that forced the question

> *"Every line that closed did so by reading something **already** durable on
> disk — session events already logged, gateway caches already written by a
> different process — rather than by adding a new producer. The producers that
> already exist were the ones that closed the map's easy lines. What is left is
> producers that do not exist yet."*

So the search heuristic this project has run on — find a mechanism built and
never installed, then wire it — is **exhausted**. A phase being mostly closed is
now evidence *against* its remainder being cheap, and `ORIENT.md`'s
fewest-open-first ranking actively misleads: three packages died at Phase −1 in
one batch, each of them recommended by that ranking.

### The decision

**Build a durable observation sink.** Offered the three tractable directions,
the user chose the root rather than the leaves.

It is the largest tractable option — roughly **18 lines** across three phases
depend on it, and every one is already diagnosed down to its seam:

- **Cluster H** (1757, 1759, 1760, 1763, 1766, 1767, 1769) — *"a view whose data
  is never made durable"*. `RoutingExplanation` (`routing/mod.rs:475`) has no
  durable sink; every production sink is a `tracing` line or an in-memory `Vec`.
  `ShellState::record_disposable_choice` (`shell/state.rs:1216`) has zero
  production callers while `shell/view.rs:1793` **already renders it**.
- **Phase 47**'s seven open lines — uniformly the same shape.
- **Phase 9K's 627–630** — four "Measure…" lines waiting on a measurement
  channel that does not exist anywhere in the build.

### The trap this decision exists to avoid

**Phase 47, as the map currently words it, is a debug VIEW.** Closing it as
scoped would deliver a view over data still not made durable, and **would not
unblock 627–630.** A recon checked this specifically. So the thing to build is a
**producer**, and no phase in the map currently owns one.

**That is why this was the user's call and not a packet's.** It is new product
surface. A package that quietly grew a producer while claiming to close a view
line would be inventing scope, and this project's rule is that the map is
authoritative and design decisions are recorded here first.

### SCOPED 2026-08-30 — and two things above are WRONG, corrected here

`GH-OBSERVATION-SINK-RECON` scoped it read-only, and its findings correct this
entry rather than confirming it. **Read this section, not the estimate above.**

**1. "~18 lines depend on it" overstates the payoff, and the correction is
material.** Counted conservatively, line by line: **0 `WOULD CLOSE`, 5 `NEEDS
MORE`, 6 `STILL BLOCKED`.** A minimal sink closes **nothing** outright and moves
**two** lines from `STILL BLOCKED` to `NEEDS MORE`. The five it is necessary for
(1757, 1759, 1763, 1766, 1769) each *also* need a reader, and often more. The six
it does not help (1760, 1767, 627–630) fail on a different missing link — no
cache-temperature signal exists at all, nothing computes a correlation, and
nothing counts output tokens.

**The user agreed to build a producer, and that is still what this is. But it
buys fewer boxes than the question implied, and the honest first package closes
zero.**

**2. No new table, and no migration.** `evaluation_observations` (migration 15,
`database.rs:1515`) **is** the sink. It was built to be extended by adding one
Rust enum variant and says so in its own header: *"One variant, because this
package lands one producer. Variants are added as producers land, never in
advance"* (`evaluation/mod.rs:89-90`). It already has retention, project-scope
triggers, a `kind` with no SQL `CHECK`, a free-text `detail`, `session_id`,
`routing_seq` provenance and a listing read.

**3. Constraint 2 below is aimed at the wrong table — do not follow it.**
`sessions` already carries **harness** (`database.rs:179`), **launch_profile**
(`:241`), **model** (`:905`), **protocol** (`:916`) and **response_profile**
(`:921`), the last two written by two real production launch callers. **A sink
row carrying `session_id` recovers every `EvidenceKey` axis by join.** Copying
`EvidenceKey` into a new table would *reintroduce* the conflation, not fix it.

**4. Line 630's recorded blocker is wrong.** `phase-9k.md:347-348` says it waits
on Phase 33A because *"separately for each harness-model pairing is that
ledger's key"*. **33A's ledger is the one missing `launch_profile`; `sessions`
keys correctly today.** 630's real and only blocker is that there is no
measurement to key.

**5. Line 1763 is closer than the register says** — production does emit three
distinct failure classes, not the one the register records.

### DELIVERED 2026-08-30 — `GH-DISPOSABLE-ROUTE-SINK`, both halves

`glasshouse hook` now records the disposable-routing decision it makes once per
completed turn into `evaluation_observations` under one new `EvaluationKind`,
and pressing `d` in the shell draws it. Both proved through the shipped binary:
a real `hook` process for the write, a real pty and a real keystroke for the
read. No migration, no new table, no `CHECK`, no constructor for
`DisposableChoice`. **Zero boxes claimed, as intended.**

**The Cluster-B guard fired for real, and this is the measurement worth
keeping.** Severing the reader was caught by **exactly one test in the whole
suite** — the pty one. Every in-crate test still passed. So a package that had
landed only the producer, or only an in-crate reader test, would have created a
sixth Cluster-B mechanism and nothing would have said so.

**The worker overrode the packet's write location and was right.** The packet
and the recon both said "after `rx.recv_timeout`". That point returns when the
extraction thread calls `tx.send(outcome)` — *before* that thread drops its
`ProjectMemory` — so a write there races a live second write-capable handle on
the platform where SQLite's locks are mandatory. It is §65's own hazard, and the
packet that warned about it proposed it. The write went one step earlier, in
`disposable_extraction_model`, where the process holds one idle connection and
nothing else. It also records on turns that time out, where the packet's point
would have silently dropped them and biased the ledger toward fast cases.

### The recommended first package

**`GH-DISPOSABLE-ROUTE-SINK`** — make the disposable-job routing rationale
durable and read it back in the shell. **It closes zero boxes**, and it is the
right first package anyway: it builds the producer plus the reader that stops it
being Cluster B, on the signal produced most often, needing no ruling, no
migration and no new table.

Chosen over the gateway failover path because that one fires only on provider
failure, holds no `session_id`, and its `routing_seq` does not exist at the
moment its explanation does. The disposable path fires **once per completed
turn**, and `report_hook` (`main.rs:2351`) already has a `&Runtime` and a
`session: &str` in scope.

`main.rs` is structurally contended — claim it with `scripts/coedit.sh` (§77)
rather than queueing behind it.

### Constraints the first package inherits

1. **A schema migration is likely, and this project refuses those casually** —
   Cluster G exists for exactly that, and says *design first*. Establish whether
   the sink needs new tables or fits an existing one before writing code.
2. **`EvidenceKey` (`harness/pairing.rs:502`) already keys correctly** —
   `(harness, launch_profile, model, route)`. `RoutingObservation`
   (`routing/evidence.rs:338`) does **not**: it has no `launch_profile`. A sink
   built on the coarser key silently conflates two profiles of the same
   harness+model+route. Reuse the key that is already right.
3. **A view is not a producer, and a producer with no reader is Cluster B.**
   The package must land both halves or state plainly which half it left, and
   `record_disposable_choice`'s existing renderer at `shell/view.rs:1793` is the
   cheapest reader to satisfy first.
4. **Do not fabricate a value to fill a column.** Map line 1294's refusal is the
   standing example: *"a fabricated value here does not degrade the policy, it
   inverts it."* A column production cannot honestly fill stays absent.

## Phase 43: the MCP surface is a transport over the existing API door

### Decided by the orchestrator, 2026-08-31

The register's Cluster R said Phase 43 was *"a design decision for the user"*
because an MCP server exposing worker spawning, messaging and interruption adds
an external control surface, and needs a dependency choice, a transport
decision, and a security model. All three are decided here, from what the tree
already contains, and none of them broadens product scope: every one of the
ten "expose X through MCP" lines names an operation `api::protocol::Request`
already performs for `glasshouse api serve`.

1. **Transport: JSON-RPC 2.0 over stdio, newline-delimited, hand-rolled on
   `serde_json`.** No new dependency. The MCP handshake is `initialize`,
   `notifications/initialized`, `tools/list`, `tools/call`, `ping` — a few
   hundred lines. A dependency that pulls an async runtime into a binary that
   has none is the thing this project has refused every time it came up.
2. **Every tool is a thin adapter onto an existing `Request` variant and goes
   through the same `dispatch` the Unix door uses.** That is how line 1702
   ("restrict MCP tools to the active project scope") is inherited rather than
   re-implemented: `SessionApi::resolve` refuses a foreign session, and
   migration 11/15's triggers refuse a foreign `project_id` at the database.
   The MCP layer opens no store of its own — a source-scan test enforces it.
3. **Dangerous operations are separate, explicitly named tools** (1703):
   `glasshouse_spawn_session`, `glasshouse_send_message`,
   `glasshouse_interrupt_session` — never one `control` tool with an `action`
   field — annotated non-read-only, so a harness that gates by tool name or
   annotation can gate exactly those three.
4. **Origin is always `RequestOrigin::Machine`.** An MCP caller is a program.
5. **One project per server process.** The server binds to the `Runtime` it
   was started in and offers no argument that names a project, a path or a
   database.

Package: `GH-MCP-SERVER`. What it must not do: reach a store directly, add a
dependency, or offer any operation the Unix door does not already offer.

## Phase 51: a routing decision's outcome is the harness's own verdict, and nothing else

### Decided by the orchestrator, 2026-08-31

The register's RC-B held twelve lines behind one question — *"how does
Glasshouse learn whether a routing decision was good?"* — and said no line may
be packaged until a person answers, because inventing a proxy would be a
fabricated denominator of the kind line 1294 refuses.

**The answer is the one signal that is not a proxy.** A harness reports
`TurnEnded { outcome: Completed | Failed }` through its own hook; Glasshouse
translates it at `session::lifecycle::event_for` (single construction site,
source-scan tested); `events::task_outcome` reads it — and has **zero
production callers** (Cluster B). That verdict is the harness's own statement
about its own turn. It is recorded against the routing decision that put the
work in that session, as a **second row** (`routing_outcome_observed`), never
an `UPDATE`.

What is deliberately **not** learned: process exit, output going quiet, the
person's next action, elapsed time. The capability map's standing rule — *do
not infer successful task completion solely because a child process became
quiet* — is the rule here too. A decision whose session never reports a turn
end has outcome *unknown*, and unknown is its own bucket in every ratio, with
its denominator printed.

This answers the *routing* half of RC-B (1834, 1835, 1854, and the success
quantity of 1845). It does **not** answer the *memory* half (1821, 1823, 1824,
1825, 1831 — "was the retrieved memory useful", "did an old decision cause
complexity") — a completed turn says nothing about whether a memory helped. Those
stay open with the same missing producer they had, and a future ruling must
find a signal that is the agent's own statement, not a correlation.

Package: `GH-ROUTING-OUTCOME`.

## Phase 33: framing is not content — the relay may count and timestamp what it never reads

### Decided by the orchestrator, 2026-08-31

Cluster L in the refusal register holds Phase 33A's 1331–1334 and Phase 33C's
1364 behind one sentence from `gateway/ingress.rs`: the body *"stays a byte
stream this function never parses."* Its own closing paragraph names the way
out — *"either `gateway::ingress` gains a bounded, streaming, non-buffering
observer that counts and timestamps without interpreting content, or these
lines stay open"* — and the 1331 ruling in `handoff.md` asked the narrower
question outright: *may the relay observe response FRAMING without reading
content?*

**Yes.** The relay already handles the status line, every header, the declared
`content_length`, and the byte stream it copies — `settle` already counts bytes
toward a cap. Metadata the relay must handle in order to forward is not the
body's content. A byte count, the fact that the count fell short of the
declared length, the fact that the peer closed before the terminating chunk,
and a timestamp are framing facts. **The boundary that stays**: no byte of the
body is inspected, decoded, matched, or buffered beyond what forwarding
already buffers. A source-scan test enforces it.

What this unblocks: a nine-way failure classification (1364) from status,
headers, transport detail and framing; rate-limit responses counted apart from
transport and model failures (1316); cadence throttling told apart from an
exhausted window by the rate-limit headers the gateway already parses (1365);
the `failovers` column written from the assignment change the exchange caused
(part of 1334).

What it does **not** unblock, so nobody stretches it: *time to first real
token*, *time to first tool call* (1331), padding-vs-token (1332), token counts
(1333), tool rounds and repairs (1334) — every one requires reading content.

**The migration.** `routing_observations.outcome` keeps its four-value `CHECK`;
it answers a different question. Migration 16 adds one nullable `failure_class
TEXT` with **no `CHECK`** — migration 15's reason: a vocabulary that will grow
must not cost a table rebuild — pinned by a Rust constant and a test, added
with `ALTER TABLE … ADD COLUMN` as `validity_conditions` was. Cluster G says a
migration needs a design first; this is the design.

Package: `GH-FAILURE-TAXONOMY`.

## Phase 21K: assumptions are stated by the agent through the door, never inferred

### Decided by the orchestrator, 2026-08-31

Phase 21K's forty-three lines describe a guardrail against the failure mode
where an uncertain inference silently becomes the premise of a large
implementation. Phase 51's RC-D says its measurements (1838–1844) have no
subject because the feature does not exist. This is the design that makes it
exist, on the same shape as Phase 43: **a build over the existing API door.**

1. **An assumption is something the agent says, through `api::protocol::Request`
   and its MCP twin.** Glasshouse never derives one from output, never reads a
   transcript for one, and never stores reasoning (998). The record is a concise
   claim, current evidence, evidence-source class, uncertainty, affected scope
   and the cheapest useful verification (1014, 1016). A claim body is untrusted
   text and is handled with `memory/inject.rs`'s discipline.
2. **Storage is an append-only ledger** — `task_assumptions` plus
   `assumption_transitions`, project-scoped by migration 15's two triggers, in
   one migration (19). The current state is the latest transition; nothing is
   `UPDATE`d. States: proposed, probing, supported, refuted, unresolved,
   waived_by_user (1018).
3. **The gate is deterministic and cheap.** `Preflight` takes the factors the
   agent states about its intended change and answers a risk class, the factor
   that decided it, a verdict from the configured mode, and at most three
   critical-assumption prompts. Trivial never gates. `guardrails.mode = off |
   advisory | risk_gated` (default advisory); only security, destructive and
   migration categories may block, and only when configured to; a per-task
   `--guardrail force|skip|lower` exists and every automatic pause names its
   origin and the override that lifts it (1008, 1052, 1053).
4. **A refuted premise may become a failed-approach memory; promotion to
   durable memory is explicit and only as a decision, constraint or finding**
   (1019, 1020, 1017).
5. **Notification rides the door** (`Events`, the watcher's completion line);
   no new `lifecycle_events` kind, whose `CHECK` costs a table rebuild.

What this does not build: a verifier framework (1031 uses the existing
`SpawnSession` if at all), and any rollback of code (1044) — Glasshouse records
the choice, it does not perform it.

Package: `GH-ASSUMPTION-GUARDRAILS`.

## Phase 21H–21J: the implementation policy is Glasshouse-authored text, delivered like a briefing

### Decided by the orchestrator, 2026-08-31

Lines 955–990 name what an agent should prefer, avoid, consider and check
before calling an implementation complete. Glasshouse's only honest mechanism
for a *policy* is to carry it to the agent and make it inspectable: a
structured, versioned, Glasshouse-authored document (`policy/`), one rule per
map line, rendered inside its own labelled block — a marker pair distinct from
`MEMORY_MARKER`, because this text is trusted and extracted memory is not —
delivered beside the memory briefing wherever Glasshouse briefs an agent, once
per session, switchable off, available on demand through the door and to a
person on the CLI, and bounded in length by a tested ceiling.

A line closes only when its content is carried **and** a test proves the text
reached the agent through the shipped binary. A line whose verb would require
Glasshouse to perform an analysis it cannot (an unindexed-scan detector, say)
is refused on that reading and the policy still carries the instruction.

Package: `GH-IMPLEMENTATION-POLICY`.

## Phase 17: cmux is driven through its documented CLI, and a pane is metadata

### Decided by the orchestrator, 2026-08-31

Phase 54 asks that cmux stay optional, that nothing depend on its undocumented
internals, and that embedded sessions work if cmux changes or disappears. The
design that satisfies all three: **`integrations/cmux.rs` wraps a small
allow-list of `cmux` subcommands behind a trait** (tests inject a fake), and
never the socket. Detection is what already exists — `IntegrationId::Cmux`'s
environment presence — corroborated by `cmux ping`. External presentation
**runs Glasshouse in the pane**: the outer process creates a workspace whose
command is an ordinary `glasshouse launch … --presentation-ref <ref>`, so the
session inside is a normal embedded one whose record says `External` and where.
That *where* is one nullable column, `sessions.presentation_ref` (migration
20). Focus goes through the integration; sending text prefers Glasshouse's own
door and falls back to cmux only when the session is unreachable, and says so.
`session/**` and `shell/**` never name cmux — a source-scan tripwire, which is
also how Phase 54's criteria are held rather than merely written down.

Package: `GH-CMUX-PRESENTATION`.

## Phase 29: a memory commit is the existing extraction, named and triggered from four places

### Decided by the orchestrator, 2026-08-31

Glasshouse has one extraction pipeline (`run_extraction`), one production
trigger (`TurnEnded { Completed }`), and an `ExtractionTrigger` vocabulary with
a `BeforeCompaction` variant whose production caller is unverified. A *memory
commit* is that pipeline with its trigger named on every memory it produces: a
person (`glasshouse memory commit`), a completed task (already), a Git commit
landing, and the harness's pre-compaction event. **Git-commit detection needs no
git hook** — the hook path runs on every harness event and the checkpoint
module already reads HEAD; a changed HEAD since the last one this session saw
is the code-change boundary, and the hash is recorded on the memories as
provenance (migration 21, `sessions.last_seen_commit`). Idempotency is the
store's existing dedupe, proven by running the same commit twice; no lock.

Package: `GH-MEMORY-COMMITS`.

## Phase 34A/35B: a declared capability that the scorer already reads is not wired twice

Ruled 2026-08-31 on GH-TIER-CEILING's refusal, verified by the orchestrator.
`ResourceCapabilities::describe(&harness_caps, facts)` — which `capability_fit`
(`routing/session.rs:786`) already calls — reads the harness adapter's own
declarations for `code_edit`, `shell_tool_use`, `browser_use` and `mcp`, and
prefers `facts` only when a fact is `Declared::Verified`. Copying the adapter's
declarations into `ResourceFacts` and attaching them to every destination would
therefore change no score anywhere: `prefer()` falls through to the same
values. It would also assert *verification* nobody performed, or, left
`Unverified`, be inert. **`Destination::with_resource_facts` keeps no
production caller, deliberately**, until something produces a fact the adapter
cannot declare — `large_context`, `fast_cheap_analysis`, `repository_review`
have no producer today and `axis_for` maps no hard capability to them. A wiring
that would survive its own mutation is the shape `cluster-b.py` exists to find;
building one on purpose is worse than leaving the gap named.

Package: `GH-TIER-CEILING`; lines 1401–1403 closed on the tier producer plus the
declarations `capability_fit` already reads.

## Phase 56: the harness is the user's choice; the subscription is Glasshouse's to route

**Instruction of record, 2026-08-31, from the user:** *"decoupling subscription /
harness — Claude Code and model, or Codex and model, or Gemini and model —
everything where possible, so Glasshouse gets a real bundled API gateway plus
subscription rules. Conceptually I want to be able to choose the harness, not
the provider, because some harnesses are more efficient in different tasks."*
Recorded as Phase 56 (map lines 1945–1956, twelve mandatory lines).

**What it changes.** Line 497 — *no broad cross-protocol request translation
until concrete harness/provider pairs require it* — was a standing rule with no
trigger. Phase 56 is the trigger, and it keeps 497's shape: translation lands
one **named** pair at a time, each behind an end-to-end test through the shipped
binary against a fixture upstream (line 1956), and each refused pair is
refused by name (line 1949). The refusal register's **P10** — *a model axis on
the candidate set*, the missing producer behind 566/569 and Phase 35A/35B — is
now a requirement rather than an observation (line 1953).

**What it does not change.** Phase 55's fixed requirements stand: no replacement
harness, no cloud service. A harness's native tooling is kept or the pairing is
refused (line 1950); nothing is silently degraded to make a pairing look
supported. The decoupling is opt-in per launch profile (line 1955); every
existing profile keeps its native pairing until the user changes it.

**Order of work, from the tree as it stands.** (1) A subscription as a routing
resource with rules — configuration first, no routing change (lines 1946,
1947, 1954). (2) The subscription/model axis on the candidate set (P10;
line 1953). (3) Gateway translation for the first named pair — the pair the
user actually wants first, with its end-to-end test (lines 1948, 1949,
1950, 1956). (4) Per-harness efficiency evidence and the preference
that reads it (lines 1951, 1952), which reuses Phase 51's evaluation
channel and today's `RoutingTierObserved` rows. (5) The announcement and the
never-charge rule (line 1954), which is what makes the whole thing
inspectable.

Not yet packaged; every package owes its Phase −1 from production code.

### 2026-08-31, later — the user's answer on pairs: all of them

Asked which harness/subscription pair the gateway should translate first, the
user answered: *"please as much of this as possible i want full
intercompatibility and translation."* Ruled from that:

- **Scope.** Every harness Glasshouse adapts is to be servable from every
  entitlement whose wire protocol the gateway can translate to the harness's
  own — the full matrix, not one pair. Line 497 (*no broad translation until
  concrete pairs require it*) stays ☑ and stays true: the pairs are now
  required, and each is still offered only behind its own end-to-end test
  through the shipped binary against a fixture upstream (1956), recorded by
  name in the pairing table (1949), and refused by name where the harness's
  native tooling cannot be kept (1950).
- **Architecture: codecs, not translators.** Translation is one canonical
  form — a request (system, messages of typed content blocks including tool
  use and tool results, tool definitions, generation parameters) and a
  response (content blocks, tool calls, stop reason, usage), with one
  streaming event vocabulary for the same — plus one codec per wire protocol
  that decodes that protocol's requests into the form and encodes responses
  and stream events out of it. A pair is a decoder and an encoder meeting in
  the middle: three protocols cost three codecs rather than six translators,
  and a fourth protocol (Gemini's) costs one codec and a harness adapter.
  Fidelity is a property of a codec and is tested per codec; per pair only the
  end-to-end test is owed.
- **The relay rule is narrowed, not repealed.** `gateway::ingress` forwards a
  request whose target belongs to a protocol the provider serves *byte for
  byte*, exactly as today. Only a target the provider does not serve — which
  today is answered `404` with nothing opened upstream — may enter a codec,
  and only when a supported pair exists for it; parsing is bounded by the
  existing body limits, streaming stays streaming, and nothing is guessed from
  a body's shape. Consequence for the refusal register's P1b: a translated
  exchange has a parsed response, so its usage is recorded as *exact* where the
  provider states it; relayed exchanges are unchanged.
- **Order, by leverage.** (T1) the canonical form, the Anthropic Messages and
  OpenAI Chat codecs both ways, the seam, and the first pair — Claude Code
  served by an OpenAI-Chat entitlement (OpenRouter and every OpenAI-compatible
  key) — end to end; (T2) the OpenAI Responses codec — Codex served by a
  Claude entitlement, Claude Code by a ChatGPT/Codex plan; (T3) a Gemini codec
  and the Gemini CLI adapter, which the tree does not have. One package each;
  none is offered before its test.

## Phase 56A: the entitlement is the unit of capacity, and a broker stands between every harness and the pool

**Instruction of record, 2026-08-31, from the user, refining Phase 56.** The
full text is quoted in the map at Phase 56A's intro; the essence: several
entitlements of the same vendor (two Claude Max 5x accounts) consumed evenly by
the scheduler, optimised around reset boundaries — *"A at 12% resetting in
1h20m and B at 61% resetting in 4d ⇒ burn A; A at 12% resetting in 4d ⇒
preserve A, route B"* — with independent workers distributed across a pool
(Claude A, Claude B, Codex, OpenRouter, API fallback) and long-running sessions
sticky to their account; scored by *available capacity × time-until-reset ×
recent throttle × session affinity × model availability*; layered as harness →
protocol adapter → authentication → entitlement → inference. Map lines
1962–1974, thirteen mandatory lines.

**Does this throw today's work overboard? No — it names what it was missing.**
Everything that landed on 2026-08-31 is per-resource machinery: capacity bands
and reset proximity (subscription-pressure), throttle scope and health
(rate-limit-scope, evaluation-producers), affinity (affinity), reserve rules
(support-work-economy). All of it keys on `ResourceKind::NativeSubscription
{ harness }` — **one account per harness**, which is exactly the inefficiency
the user names. Phase 56A changes the unit: the entitlement. Every one of those
producers becomes per-entitlement without changing what it measures. Phase 56's
first package (`subscription-rules`, 1946/1947/1954) is the *rules* half of an
entitlement and stays; its type is the seed of the pool's per-entitlement rule.

**Order of work, from the tree as it stands.**
1. **The entitlement as a configured resource, several per vendor** (lines
   1962, 1963, 1964, 1973) — config and data model: `[entitlements.<name>]`
   with `kind`, `vendor`, its own credential reference (Phase 9E's secret
   storage; never shared across entries), and `native_harness` optional.
   `ResourceKind::NativeSubscription { harness }` becomes one shape of an
   entitlement, not the shape. No routing change yet.
2. **Per-entitlement telemetry** (line 1965) — `CapacityFacts`/`CapacityState`,
   `ObservedHealth`, `ThrottleScope` keyed by entitlement. Only what the
   provider exposes (Cluster E discipline); an entitlement with no telemetry
   reads as *unknown*, never as full or empty.
3. **The broker's score and placement** (lines 1966, 1967, 1968, 1969) — the
   five-factor score as named contributions in the existing router
   (`RouterInputs` gains the pool), the reset-boundary rule as its own term
   with the user's two examples as its tests, stickiness reusing `session_affinity`.
4. **Fallback order and the announcement** (lines 1970, 1971, 1972) — one
   stated order, every fallback recorded with a purpose in the evidence
   ledger, one `glasshouse entitlements` view.
5. **The end-to-end test** (line 1974) — fixture entitlements, a reset
   boundary crossed, an exhaustion fallback, through the shipped binary.

Phase 56's translation lines (1948–1950) remain a separate track and still
wait for the user to name the first pair. **Pursue Phase 56A as the core of
Phase 56.**

### Step 4's fallback order — the user's ruling, 2026-08-31

`entitlement-fallback-view` closed line 1972 and **refused line 1970**, on the
ground that the order it names ("subscription to subscription to API credits")
is an order over *kinds*, and the only two ways to express it were both barred:
routing on `EntitlementKind` would falsify that field's own documented
invariant — *"No rule depends on it — so a wrong `kind` misdescribes an
entitlement and never misroutes one"* — and it is `Option` and absent by
default; the alternative needed a new `EntitlementConfig` field. The worker
stopped and asked rather than invent policy, and left no dead code behind.

**The user's answer, in their words:** *"switch to another subscription always
if same model or model of similar capability is available in another. You can't
put a fable 5 task and switch it to a nemotron v3 so we have to think in model
tiers. These are determined by public benchmarks if this possible to find out as
a baseline. A user can assign tiers to models I think that's a better plan for
UX. Determining model if they are api or subscription is just which entitlement
brings them. A api key or a subscription isn't that the distinction?"* — and,
completing the order: *"If subscription model of capability is not available
switch to api one - if available."*

**The order, therefore, and its missing constraint.** Line 1970's order stands
as written, but it is **tier-preserving**, which the line does not say and which
is the part that matters:

1. another **subscription** entitlement that can serve the **same model, or a
   model of the same capability tier**;
2. failing that, an **API-credit** entitlement that can serve that tier;
3. never a fallback that changes the capability tier. *"You can't put a fable 5
   task and switch it to a nemotron v3."* A fallback that silently downgrades
   the model is worse than a refusal, because the work continues and looks fine.

**Three consequences for the implementer, each checked against the tree.**

**(a) The subscription/API distinction needs no new field and no
routing-significant `kind`.** It is already structural:
`EntitlementBacking::NativeHarness(_)` authenticates *through the harness* —
that is a subscription — and `EntitlementBacking::Provider(_)` carries a
credential, which is an API key. The separation is not merely conventional, it
is **enforced**: an entry that is a native sign-in *and* names its own
credential is refused as `NativeSignInWithOwnCredential`, which is map line
1973's isolation rule. So `EntitlementKind`'s invariant survives intact, and the
user's hope — *"isn't that the distinction?"* — is already true in the data
model.

**The gap is one level up:** `ResolvedEntitlement::to_routing` renders the
backing as a **human string** ("no backing stated", and so on) rather than
carrying the discriminant, so the router cannot branch on it today. Making
`routing::Entitlement` carry the backing as data is the first piece of work.

**(b) "Model tier" is a new axis, and the existing `tier` vocabulary is not
it.** `WorkloadTier` (`routing/classify.rs`) ranks **how hard the task is** —
`Deterministic`, `Leaf`, `Standard`, and up — and `EntitlementConfig`'s
`allow_tiers`/`deny_tiers` are `ConfiguredWorkloadTier`, the same axis.
`routing/capability.rs` describes **hard capabilities** (`CapabilityAxis`,
`ResourceCapabilities`), which is *can it do this at all*, not *how capable is
it*. **Nothing in the tree ranks models by capability class.** That is the
second piece of work, and its home is **Phase 34F, "Model capability and tier
calibration"** — not Phase 56A, which should consume the axis rather than define
it.

**(c) User-assigned tiers are the source of truth; benchmarks are a default.**
The user's UX preference is explicit — *"A user can assign tiers to models I
think that's a better plan for UX"* — with public benchmarks as *"a baseline"*.
That ordering matters: a shipped benchmark table is a **seed** a user may
override, never an authority that overrides them, and a model the table does not
know must read **unknown** rather than be guessed into a tier (the same Cluster E
discipline the pool view already applies to capacity). A wrong tier here does
not misdescribe an entitlement, it **misroutes work** — which is exactly the
property `EntitlementKind` was kept free of.

**Order of work this implies:** (1) carry the backing discriminant into
`routing::Entitlement`; (2) define the model-capability tier under Phase 34F,
user-assignable with a benchmark-seeded default and an honest `unknown`;
(3) then line 1970's fallback becomes a post-ranking reselection over
`Routed::considered()`'s already-complete list — which preserves *additive,
never a filter* — recorded on the launch path with a new purpose constant beside
`TIER_ESCALATION_PURPOSE`, exactly as `record_tier_movement` does.



### Step 5's implementation rulings — accepted from GH-BROKER-FALLBACK-56A, 2026-09-01

The three-step order of work above landed (batch 70). Two decisions made in
the landing carry authority beyond it and are recorded here.

**A spend ceiling is stated in tokens, and the broker enforces tokens only.**
`[entitlements.<name>] spend_ceiling_tokens` refuses when the ceiling and an
observed reading are both established. It is tokens because
`routing_observations.cost_micro_usd` has no producer in this build and map
line 1465's closed reader already ruled that tokens — input plus output as the
provider reported them — are the only currency this ledger holds. A ceiling
stated in money could never be reached, which would make line 1971's *"never
let the broker exceed them"* vacuous. `[providers.<name>.quota] budget`
(`MonetaryBudget`, line 1203) remains the money ceiling and remains, by its own
documentation, uncounted. If money enforcement is ever wanted, it needs a
cost producer first — not a silent reinterpretation of this field.

**The tier axis plugs into the fallback through `Destination`, not through
configuration reads inside `routing`.** `same_capability_tier` is a free
function over two model names that deliberately reads no configuration
(`routing` may not), answering `Same` only for the identity case and `Unknown`
otherwise — so until Phase 34F's axis is wired, the fallback order collapses
to its two same-model steps and never widens on `Unknown` (mutation-pinned).
The accepted wiring shape, when 34F's consumer lands: attach the model's
capability tier to `Destination` the way `tier_ceiling` already is — one
field, one builder, populated where `destination_tier_ceiling` already calls
`resolved_ceiling` in `main.rs` — and `same_capability_tier` becomes a
comparison of two attached values. The alternative (threading a
`&dyn ModelTierAxis` through `choose`) was considered and declined: it needs
a `SessionRouter` builder change for no additional honesty.

**One packet error worth keeping visible:** the dispatching packet asked for
unknown-tier tasks to be *refused* against tier-restricted entitlements, which
contradicts the COMPLETE contract of lines 1947/1954 (a tier rule fires only
against an established tier). The worker implemented the packet's intent as
the *fallback's* narrowing — unknown never widens the candidate a fallback may
take — and left the closed gate contract intact. That reading stands.

## Phase 57 — Context firewall: the implementation arc, decided 2026-09-01

The user's spec (instruction of record, 2026-09-01) asked for an optional
tool-output compaction subsystem between harness and model — reduce, never
decide; preserve everything; fail open; measure from day one. These are the
architectural decisions that bind its packages, made so the feature embeds in
what Glasshouse already owns instead of growing a parallel organ.

**The semantic reducer is a disposable support job, not a new client.** The
spec's provider matrix (OpenRouter, Groq, Cerebras, OpenAI-compatible, local)
IS Glasshouse's existing provider registry; free-router aliases and pinned
free models are Phase 9I's free-pool machinery; per-entitlement
`deny_job_kinds` (56A) applies to reduction jobs unchanged. A new
`JobKind` variant carries it, and `disposable_interface.rs`'s variant-roster
tripwire firing on that variant is the designed signal to re-read Phase 39's
1625 refusal — not a regression. No firewall-private HTTP client exists at
any point.

**Telemetry is purpose-bucketed evidence-ledger rows.** Reducer calls are
real model calls: `NewObservation` with a `context-firewall` purpose family
(reduction, bypass), token counts as reported, provider/model as routed —
rendered by the same `consumption_by_purpose`/`RoutingOverhead` path the
tier-movement and pool-fallback rows ride (batch 70's pattern). Raw/
deterministic/forwarded sizes and the bypass reason live on the reduction
row; raw-expansion requests are counted as their own rows because they are
the recall signal the phase treats as primary.

**Raw preservation is a per-session content-addressed file store under the
data dir, not a migration.** MVP needs write-once blobs addressable by a
stable reference; SQLite adds a migration (Red tier, schema-pin ripple) for
no MVP query need. If listing/joining ever demands a table, that is its own
later ruling. References are `gh-tool://<id>` per the spec's shape.

**The Claude Code bridge's load-bearing premise is contested and gated.**
`harness-hook-protocol.md` (earlier, process-side experience): *"no hook
return field carries a substitute tool result — a hook is a gate, not a
proxy."* The spec asserts current `PostToolUse` supports
`hookSpecificOutput.updatedToolOutput`. Map line 1994 resolves the tension
structurally: verify at session start, fall back to shadow with a stated
reason. Recon against the installed Claude Code decides the bridge package's
shape; nothing else in the phase depends on the answer.

**Session-scoped hook registration, never the user's settings.** The bridge
registers through launch-time/session-specific configuration only for
Glasshouse-managed sessions the user enabled it for; a mechanism that
requires editing committed `.claude/settings.json` is refused as
premise-invalid for its package.

**Config is a top-level `[context_firewall]` section, mode `off` default,
semantic reduction requiring an explicitly named reducer in every mode.**
Thresholds (passthrough, semantic-minimum, target) are configuration with
defaults, never architectural constants.

**Harness abstraction from the first line of code:** the core consumes a
normalized tool result and emits a normalized reduced result; Claude Code
JSON lives only in an adapter beside the other integrations. Codex and
later harnesses reuse the core through their own interception mechanisms.

**Package sequencing** (each later package names its predecessor's evidence):
core + deterministic ladder + preservation + provenance (map 1980–1990,
reachable through a `context-firewall hook` CLI entry so the boxes have a
production caller) → modes + Claude Code bridge behind the recon verdict
(1991–1996) → the disposable-job reducer (1997–2003) → expansion, shadow
evaluation, and status surfaces (2004–2006). Boxes stay open until their
production reach exists — ten wrongly ticked boxes taught this project that
rule, and this phase starts under it.

### Phase 57 addendum — the hook-replacement premise VERIFIED, 2026-09-01

Recon against the installed Claude Code (v2.1.252, official hooks reference):
**`PostToolUse` supports `hookSpecificOutput.updatedToolOutput`**, replacing
the tool output the model sees; `systemMessage` and `terminalSequence` are
the only other documented PostToolUse output fields (`additionalContext`,
`updatedInput`, `permissionDecision` are PreToolUse-only). Stdin carries
`tool_name`, `tool_input`, `tool_response`, `tool_use_id`, `session_id`,
`prompt_id`, `transcript_path`, `cwd`, `permission_mode`, `hook_event_name`,
`effort`. Built-in tool responses are uniformly `{"type":"text","text":…}`;
MCP/WebFetch may differ and stay pass-through until adapted.

**Session-scoped registration lands on the `--settings '<json>'` launch
flag** — highest precedence short of managed settings, no file touched, dies
with the session — which is exactly Glasshouse's launch-profile injection
point. `.claude/settings.local.json` is the fallback mechanism, not
preferred: it persists beyond the session.

**Line 1994's session-start verification stays mandatory anyway**: the
introducing version could not be pinned from release notes, so older
installations may lack the field — the bridge probes (a version floor plus a
capability check) and falls back to shadow with a stated reason.
`harness-hook-protocol.md`'s "no hook return field carries a substitute tool
result" is corrected in place: true when written, false for ≥2.1.252, and
its worker-bridge guidance is unaffected.

### Phase 57 addendum 2 — the REAL Bash payload, and the one-settings-document rule (2026-09-01)

GH-FIREWALL-BRIDGE captured real payloads from the installed Claude Code
(2.1.252) with a tee-hook against a live `claude -p` run, correcting the
first addendum on one point:

**Bash is NOT the uniform text shape.** A real successful Bash
`tool_response` is
`{"stdout":…,"stderr":"","interrupted":false,"isImage":false,"noOutputExpected":false}`
— richer than `{"type":"text",…}`, and **with no `exit_code` key at all**.
And a FAILING Bash call never reaches `PostToolUse`: it fires
`PostToolUseFailure` instead, with `{"error":…,"is_interrupt":…}` and no
`tool_response`. Consequences, both landed: `exit_code: None` is the
ordinary success shape (an explicit non-zero refuses reduction; an absent
key does not — the previous `Some(0)`-required check made Bash permanently
un-reducible against the real harness), and since this build registers only
`PostToolUse`, a failing Bash result is a shape the firewall never sees.
The captured payloads are fixtures in `firewall::adapter::tests` and
`firewall_bridge.rs`. Grep/Glob/Read remain the verified uniform text shape.

**One settings document, never a second `--settings`.** The pre-existing,
verified fact in `session::HarnessSelection::install_session_document`:
`claude --settings A --settings B` validates only `B` — a second flag
silently discards the first. The firewall therefore merges its `PostToolUse`
key into the SAME session settings document lifecycle hooks and the response
profile already share; `HarnessLaunch::arg` is untouched in every mode,
which is also what makes `mode = "off"`'s byte-identical guarantee hold by
construction.

## Two quantities Glasshouse refuses to approximate, and what the source may cite

Decided 2026-09-01, promoting two standing refusals out of a process
document. Both were already correct and already recorded — the mistake was
where the shipped source pointed for them. `check-doc-boundary.sh` is right
to refuse a product source file that cites `docs/process/`: a reader of the
binary cannot act on how the binary was built, and a refusal a test enforces
is a **product** decision about behavior. The rule is enforced now, so the
two decisions live here.

### A task is never "nearly complete" — Glasshouse does not guess at progress

`ReserveDecisionInputs::task_nearly_complete` is `false` at both of its
production construction sites (`routing/pressure.rs::reserve_verdict` and
`routing/disposable/mod.rs`'s per-candidate loop — this section said "its one
site" until 2026-09-03, when re-deriving it for the design note below found
the second) and that is a decision, not an omission.

**Nothing in this build observes task progress.** The only completion fact
available where the reserve verdict is computed is that a turn is already
over, and a turn boundary is not a task boundary. The available proxies —
turn counts, elapsed time — all report "almost complete" for work that has
merely been running a while, which is precisely the long-running work the
protected reserve exists to keep serving. **A fabricated value here does not
degrade the policy, it inverts it**, and it inverts it exactly at the moment
the protection matters.

So the field is carried, spelled honestly, and passed `false`. Two
capability lines rest on this and are refused on the same ground rather than
approximated: *"avoid migrating a nearly completed task solely to preserve a
small amount of quota"* is the same guard seen from a different phase, and
gets the same answer. **If a real producer of task progress ever lands, both
lines re-open together** — that is why a test pins the constant rather than
letting it drift, and why relaxing the test is not a way to close either
line.

> **That happened on 2026-09-03, and this section's prediction is what made it
> safe.** A producer landed — a *declaration*, not an observation — and 1294
> and 1610 closed together exactly as written here. Nothing above is retracted:
> Glasshouse still does not guess at progress, the proxies named here are still
> refused, and the pins were **re-stated by the same argument** rather than
> relaxed. See *A task's progress is declared, never guessed* below, and
> `evidence/phase-32f.md` / `phase-38.md` for the closure.

### A file association is observed, never inferred

`FileAssociation` distinguishes a file Glasshouse *watched* a session touch
from a file a memory *refers to*. Only the first has a producer:
`MemoryStore::record_observed_files` writes `Observed`, and every row this
build can produce carries it.

`Referenced` is deliberately unreachable. Its qualifier — *the memory refers
to this file* — is a claim about a memory's meaning, and nothing on the
production extraction path reads a memory's body for file paths. Inferring
it from a path-shaped substring would produce a confident association from a
file name mentioned in passing, and file-aware retrieval is meant to be
advisory precisely because a stale association is worse than none.

**The variant stays** because the distinction is real and the day extraction
can identify a reference reliably it will be needed. Until then, a test
asserting a row says `observed` is also, structurally, asserting that no
production path invented `referenced`.

## SQLite write contention: short transactions, `BEGIN IMMEDIATE` where two processes meet, no WAL — decided 2026-09-02

**The question.** The Windows flake family (`phase-54a.md`, 2026-09-02) had one
member that was not a stub defect: `checkpoint_portability`'s two-writer race
starved one thread past the five-second busy timeout on the loaded ARM64 VM.
Every connection `database::open` hands out is rollback-journal with a
five-second `busy_timeout` and SQLite's busy handler has no fairness. The
test's workload was two threads writing a hundred rows each in a tight loop.
Whether any *product* path produces contention of that shape was recon'd
(`GH-SQLITE-CONTENTION-RECON`, read-only, 2026-09-02) before anything was
changed.

**What the census found.** Seven production constructors go through
`database::open`. Every write transaction on a production path is a single
autocommit `INSERT`/`UPDATE` or a small bounded batch: the checkpoint store's
`save` (`checkpoint/store.rs:229-244`), the routing ledger's one row per
forwarded exchange (`routing/evidence.rs:2680`), the event log's one row per
lifecycle event drained by a dedicated writer thread and never per PTY byte
(`events/log.rs:759-800`), the evaluation store's per-task batch
(`evaluation/mod.rs:865-895`), and the session store's `in_a_write_transaction`
(`session/store.rs:1901-1918`) — one `SELECT` plus one `UPDATE` under
`BEGIN IMMEDIATE`. Nothing loops rows from one connection. The events that
drive these writes — a completed turn, a forwarded exchange, a checkpoint —
are paced by human and model turn time, seconds to minutes apart.

**Two processes on one project database is real and already handled.** There
is no single-instance guard anywhere; `glasshouse hook` is a separate
short-lived OS process spawned by the harness beside the shell or `api serve`
that supervises the session, and both write `sessions.lifecycle`. That race
was found once (`session/store.rs:1745-1770` tells the story) and fixed by
`BEGIN IMMEDIATE`, not by the busy timeout: the ordering is what protects
correctness, the timeout only bounds the wait.

**Decision.** No WAL, no longer timeout, no retry in the stores. WAL's
sidecar files would touch `prepare_file`'s permission and symlink checks, the
read-only detection, the copied-database scenarios `checkpoint_portability`
proves, and Windows file locking — a Red package that buys nothing while no
production transaction approaches the timeout. The test that starved was
fixed as a test (retry on `DatabaseBusy` alone, within a deadline, panicking
on anything else).

**Validity condition, and what reopens this.** This holds while every
production write stays a single statement or a small bounded batch. A future
path that writes many rows in one transaction from one connection, or a
`database is locked` observed in the field with two Glasshouse processes on
one project, reopens it — and the first answer then is a bounded retry in the
store that contends, WAL only if the retry is not enough. The recon flagged
its own thin spot honestly: the "reachable overlap" claim rests on reading
the transaction shapes, not on a measured exchange or hook rate, and nothing
was run on Windows to confirm that the VM's slow rollback-journal fsync does
not stretch even single-statement writes. A tripwire test over transaction
shapes was considered and declined: a structural scan of `src/` for
multi-row loops cannot fail for a real reason, and a test that cannot fail
for a real reason is the shape §65 warns about.

## Phase 51, the memory half of RC-B: an explicit rating when given, a labelled proxy otherwise — user ruling 2026-09-02

**The question, as parked.** The 2026-08-31 ruling above answered the *routing*
half of the register's RC-B with the one signal that is not a proxy — the
harness's own `TurnEnded` verdict — and left the *memory* half (1821, 1823,
1824, 1825, 1831) open: *"a completed turn says nothing about whether a memory
helped … a future ruling must find a signal that is the agent's own statement,
not a correlation."* The orchestrator parked the question to the user with four
options (explicit signal only; observable proxy only; both; neither).

**The user's answer, verbatim:** *"Both: explicit rating when given, the
labelled proxy otherwise."*

**What that decides.**

1. **The explicit signal is a rating command, and it is the agent's or the
   user's own statement.** `glasshouse memory rate <memory-id> <verdict>
   [--session <id>] [--note <text>]`, beside `memory challenge` and `memory
   resolve`, recorded as its own evaluation observation (kind `memory_rated`,
   outcome the verdict) and never an edit of the retrieval row it judges. The
   verdict vocabulary is closed and each word is one map line's own quantity:
   `useful` / `not-useful` (1821), `prevented-repetition` (1831),
   `caused-complexity` (1823), `revalidation-correct` /
   `revalidation-wrong` (1824), `challenge-justified` /
   `challenge-unjustified` (1825). A harness can issue it as a tool call at the
   end of a task; a person can issue it afterwards. Unrated stays unrated.
2. **The proxy exists only where an observation actually bears on the
   question, and it is labelled `proxy` in every reader.** For *was the
   retrieved memory useful* (1821) and *did it prevent repeating a failed
   approach* (1831), the proxy is: the retrieving session's turn ended
   `Completed` by the harness's own verdict (the 08-31 mechanism) with no
   failover, retry, override or early abandonment recorded against it. That is
   a correlation, and the reader says so with the word `proxy` beside the count,
   never merged into the explicit count. For 1823, 1824 and 1825 — whether a
   decision *caused* complexity, whether a revalidation was *correct*, whether
   a challenge was *justified* — no observation in the build bears on the
   question, so there is no proxy: the reader prints explicit ratings and
   `unknown`, with the denominator, and nothing else.
3. **Nothing is inferred from silence.** A memory retrieved into a session that
   never reported a turn end is `unknown`; `unknown` is its own bucket in every
   ratio, printed with its denominator — the standing rule from the routing
   half, kept.
4. **No migration.** The evaluation table's `kind` and `outcome` columns are
   text; the new kind and verdicts are new strings, read back by the same
   `from_stored` that refuses what it does not know. The proxy is computed by
   readers from rows that already exist (`memory_retrieved` rows joined to the
   session's `routing_outcome_observed` rows) and is never stored, so a later
   change to what counts as a proxy changes every past reading consistently.

**Validity condition.** This holds while the harness's turn verdict is the only
outcome the build observes. The day an agent's own statement about a memory
arrives through a hook rather than a command, it becomes an explicit rating
with a different `source`, not a proxy, and the vocabulary above is what it
maps onto.

Package: `GH-MEMORY-RATING` (Amber): the command, the kind and verdicts, the
recorder, and the readers for the five lines with `explicit / proxy / unknown`
and denominators.

## Headroom, compared — what Glasshouse takes, what it refuses, and why — user instruction 2026-09-02

**The instruction.** The user asked for a side-by-side comparison with
[headroomlabs-ai/headroom](https://github.com/headroomlabs-ai/headroom)
(Apache-2.0, a local context-compression layer: library, proxy, agent wrap, MCP
server; 68k stars on 2026-09-02) and then ruled: *"take everything which would
benefit us in a meaningful way and ingest it — make sure this is documented
going forward so future orchestrators won't forget and it's not getting lost on
the sidelines."* The taken half is **Phase 58** (map lines 2014–2040, 15 lines);
this entry is the reasoning, so nobody re-derives the comparison or re-litigates
the refusals.

**What Headroom is.** One problem solved deeply: compress everything an agent
reads — tool outputs, logs, RAG chunks, files, history — before the model sees
it, locally, reversibly. A statistical JSON crusher, an AST code compressor and
a trained prose model behind a content router; originals kept in a hash-keyed
SQLite store with a `headroom_retrieve` tool and a 30-minute TTL (CCR); a Rust
proxy being rebuilt around byte-faithful passthrough (`RawValue` diffs), a
frozen cache hot zone, append-only history and live-zone-only compression;
auth mode (pay-as-you-go / OAuth / subscription) detected from header shapes
and used as a compression-policy axis; deterministic tool-array and JSON-Schema
key ordering, automatic `cache_control` placement and `prompt_cache_key`
injection for cache stability; an output shaper (a terse note appended at the
*end* of the system prompt to keep the cache, and a clamp-only per-turn effort
reduction when a turn only resumes after a tool result); `headroom learn`,
which mines failed sessions, correlates each failure with the fix that
followed, and writes a marker-delimited block into `CLAUDE.local.md`; seeded
offline proof tables and accuracy suites; a telemetry beacon on by default.

**Side by side, in one table.**

| concept | Headroom | Glasshouse | verdict |
|---|---|---|---|
| tool-output compression | three developed compressors, sub-millisecond, published ratios | Phase 57: deterministic ladder plus a reducer that is a routed disposable job | theirs deeper; ours honest about cost |
| reversibility | CCR store, retrieve tool, TTL | `gh-tool://` content-addressed raw store, expansion counted as the recall signal | equivalent; ours measures recall |
| proxy fidelity | byte-faithful passthrough by `RawValue` diff, hot zone frozen | the relay never parses a body; codecs only on translated pairs | ours stricter by construction |
| cache stability | tool sort, schema-key sort, `cache_control` placement, `prompt_cache_key` | codecs **refuse** `cache_control` by field name; a default Claude Code launch on a translated pair needs `DISABLE_PROMPT_CACHING=1` (`phase-56.md`, T1's recorded limit) | **our gap** |
| auth mode as a policy axis | sniffed from headers and user-agent prefixes; drives compression policy | entitlements: configured, ruled, pooled, brokered, with telemetry facets — but the firewall's thresholds are plain config | ours is the model; theirs uses the axis |
| output shaping | proxy-appended terse note; per-turn effort clamp | response profiles through native mechanisms with a verification floor; tiers pick the model per task | ours safer; the per-turn clamp is an idea we lack |
| learning from sessions | failure-to-fix mining into a marker block in the native instruction file | extraction, authority classes, revalidation, conflicts, ratings, injected at launch | ours stronger; their delivery shape is cheap and visible |
| harness coverage | fifteen agents by base-URL rewiring | four adapters with PTY, lifecycle, hooks, approval modes | broad and shallow vs narrow and deep |
| measurement | seeded proof tables, accuracy suites, a savings readout | evidence-ledger rows by purpose; Phase 9K's measurement lines open | they publish numbers we cannot yet print |

**Taken, and where each lives in Glasshouse (Phase 58).**

1. **Cache-stable translation.** Carry `cache_control` where the target has an
   equivalent, strip with a recorded reason where it does not, deterministic
   tool and schema ordering, hot-zone bytes unchanged across turns, a
   per-session `prompt_cache_key`, and cache read/creation tokens measured per
   exchange. This is the one place the comparison found Glasshouse behind on a
   thing the user needs: the harness we care most about is refused on every
   translated pair by default. Package first.
2. **Reduction policy keyed by entitlement kind.** We already know at launch
   whether the serving resource is a subscription, a metered key or local
   inference; the firewall's thresholds should follow.
3. **A local out-of-process reducer.** Headroom's compressors are better than
   ours will be for a long time and they run locally under a permissive
   licence; the firewall's reducer seat can take an installed tool beside the
   model-backed reducer, with the same provenance, preservation and expansion
   path, and absence or failure as a stated bypass. Use it; do not rewrite it.
4. **A savings readout that is a query over the ledger**, by purpose and
   profile, with denominators — which is what Phase 9K's open measurement
   lines have been waiting for — and a seeded offline proof fixture for the
   deterministic ladder.
5. **Per-turn effort clamp, evaluated first.** Cheap where a body is already
   parsed (translated pairs), impossible on the relay without breaking its
   promise; a recon decides whether to offer it.
6. **An opt-in export** of remembered constraints and failed approaches into a
   marker-delimited block of the harness's native local instruction file.

**Refused by name, so the next reader does not re-derive them.**

- *Header-sniffed auth mode.* We have the real thing (entitlements); a
  user-agent prefix is not a policy input.
- *A telemetry beacon on by default.* Against this project's own rule that
  telemetry measures outcomes locally, never phones home.
- *Base-URL wrapping instead of adapters.* Breadth we do not want at the
  price of lifecycle, approval and hook fidelity.
- *Steering text appended to the system prompt at a proxy.* Our native
  mechanism with the verification floor is proven; a proxy-side append would
  have to touch the relayed body.
- *Deleting or summarising history.* Headroom itself abandoned deletion for
  live-zone-only compression; the map's own rule keeps native compaction and
  project memory separate.

**Validity condition.** This holds while Headroom stays local, permissively
licensed and reversible. If its reducer moves behind a hosted API, item 3
becomes a provider entry like any other and loses its "local" seat.

**Order of work:** `GH-TRANSLATE-CACHE-STABILITY` (Amber, the codecs) →
`GH-FIREWALL-ENTITLEMENT-POLICY` (Amber) → `GH-SAVINGS-READOUT` (Green/Amber,
also serves 627–630) → `GH-LOCAL-REDUCER` (Amber, with a design note on the
subprocess boundary and the tool's version pin) → `GH-RECON-EFFORT-CLAMP`
(read-only, names its successor) → `GH-MEMORY-EXPORT` (Green).

## The local reducer seat — Phase 58 lines 2028–2030, designed 2026-09-02 before its packet

**Why a note first.** *Headroom, compared* (above) takes Headroom's compressors as a local reducer *"beside the model-backed reducer, with the same provenance header, raw preservation, and expansion path"*, and asks for a design note on the subprocess boundary, the version pin, and what is sent. Reading Headroom's tree settles one fact the map's wording did not: **its compressors return compressed text, not verdicts.** Glasshouse's semantic stage is a selector by contract — map line 1985 (*reduction selects, ranks, and annotates candidates; it never generates replacement evidence text*) and 1999 (*structured candidate-selection output … rebuild the final result from trusted original candidates by id*) — and `firewall::reducer::Reducer::select` returns `Verdict { id, relevance, reason }` per candidate. So a local tool cannot be dropped into the seat as a text filter; its output has to become verdicts, and the compressed text it produces is never forwarded. That is the design's one real decision, and the rest follows from it.

**The seat.** `[context_firewall.local_reducers.<name>]` declares a tool: `command = ["headroom-select"]` (argv, never a shell string), `version = "0.9"` (optional pin, prefix-matched against what the tool reports), `timeout_ms` (default 4000; refused if it does not leave two seconds inside the hook's own ten-second timeout, `CONTEXT_FIREWALL_HOOK_TIMEOUT_SECONDS`). `[context_firewall].reducer = "local:<name>"` selects it exactly where a provider or entitlement name selects the model-backed reducer today; `reducer_local_only` is unaffected (a local tool is local by construction). Nothing else in the firewall changes: the deterministic ladder runs first, the raw store keeps the original, the provenance header and the expansion path are the ones every reduction already has.

**The boundary.** One subprocess per reduction. JSON on stdin, JSON on stdout, exit status read, stderr captured to the debug log and never forwarded. The child runs with the launch's own credential-variable filter applied (`foreign_entitlement_credential_vars` — the same scrub the harness child gets), in a per-session scratch directory rather than the project root, with no expectation of network but no attempt to forbid it: a tool the user installed is the user's, and the note's promise is about what Glasshouse *sends*, not what the tool may do.

**What crosses, and what does not.** The request is Glasshouse's own contract, not Headroom's: `{"version": 1, "tool": <harness tool name>, "query": <the tool's query string or null>, "candidates": [{"id": n, "text": s}, …]}`. The candidate texts are the tool result the firewall is reducing — the thing the session was about to read anyway. **Never sent:** the task text (the model-backed reducer receives it under line 1998's *stated task*; a subprocess Glasshouse did not write does not need the user's own words to filter a log), the transcript, memory, file contents beyond the candidates, or any credential. The reply is `{"version": 1, "tool_version": "<string>", "verdicts": [{"id": n, "relevance": "relevant"|"uncertain"|"discard", "reason": s}, …]}`; `decide_keep_set` then applies exactly the inclusion bias it applies to a model's verdicts (line 2000), so a local tool's silence about a candidate keeps it.

**Absence, timeout and failure are bypasses, never errors (2029).** An executable that cannot be started, a reply after the timeout, a non-zero exit, a reply that is not the contract, or a `tool_version` the pin does not match each become a `SemanticBypassReason` of its own (`local-reducer-absent`, `local-reducer-timeout`, `local-reducer-failed`, `local-reducer-version`), forwarded like every bypass today: the deterministic result goes through, the header says why, the ledger row carries the reason in `route`. The version reason is the pin doing its job — a tool upgraded underneath a pinned configuration changes what gets dropped, and the user asked not to have that happen silently.

**Recording which reducer produced each reduction (2030).** `ReducerCallInfo { provider: "local:<name>", model: <tool_version>, route: None, tokens: None }` — the same row a model-backed reduction writes, with the tool's identity where the provider and model go, so `GH-SAVINGS-READOUT`'s facet groups by reducer with no new column. The provenance header names it too: `[glasshouse context firewall: semantic reduction by local:headroom 0.9.3 kept k/n candidates]`. No token counts: a local tool spends no tokens, and writing an estimate there is what the 1987 ruling forbids.

**Headroom, specifically.** `headroom-select` is a shim of about twenty lines of Python over `from headroom import compress`: it joins the candidates in id order, runs the transform Headroom's content router picks for the tool's shape (its log, search and diff compressors; never the prose model, which rewrites), and answers `relevant` for a candidate whose text appears verbatim in the transform's output, `discard` for one that is absent, `uncertain` for one the transform rewrote. Checked in under `contrib/headroom-select.py` with its `tool_version` taken from `headroom.__version__`; a user installs Headroom (`pip install "headroom-ai[all]"`) and points `command` at the shim. Glasshouse ships no dependency on Headroom and never imports it; the shim is an example of the contract, and any tool that speaks the contract sits in the same seat.

**Refused, with reasons.** *Glasshouse as an MCP client of `headroom_compress`* — a second long-lived process to supervise inside a ten-second hook, and a protocol far larger than a stdin/stdout contract needs. *A resident daemon for the tool* — a per-call Python start is tens of milliseconds against a four-second budget; measure first, and the savings readout will say whether a `serve` mode is worth its lifecycle. *Forwarding the tool's compressed text* — it is the one thing lines 1985 and 1999 exist to forbid, however good the compressor.

**Validity condition.** The one *Headroom, compared* states: local, permissively licensed, reversible. The contract outlives Headroom either way.

**Successor, named:** `GH-LOCAL-REDUCER` (Amber, Sonnet high): `firewall/reducer.rs` (`LocalToolReducer` implementing `Reducer`), `config/firewall.rs` (the `local_reducers` table and `local:` selection), `firewall/mod.rs` (the four bypass reasons), `firewall/provenance.rs` (the reducer's name in the header), `main.rs::disposable_reducer` (the `local:` branch), `contrib/headroom-select.py`, and shipped-binary tests against fake tools: one that answers the contract, one that sleeps past the timeout, one that prints garbage, one absent, one reporting a version outside the pin — five bypass-or-apply outcomes, each visible in the header and the ledger row.

## Carrying effort across a translated pairing — the prerequisite for Phase 58 line 2039, designed 2026-09-02 after `GH-RECON-EFFORT-CLAMP`

**Why a note.** Line 2039 asks for an *evaluation* of a clamp-only per-turn effort reduction on translated pairings. The recon found there is nothing to clamp: `canonical::Request` has no effort field, Claude Code's `thinking` object is refused at the shared decode seam for every target, and no encoder emits `reasoning_effort`, `reasoning.effort` or `thinkingConfig`. A shadow of a mapping nobody has written is an earlier box, not a smaller one. So the order is: carry effort first, measure second, clamp third — and only the first is a design question.

**What is carried.** One field on the canonical form, in the pattern `cache_requested` set: `effort: Option<EffortRequest>`, where `EffortRequest { budget_tokens: Option<u64>, level: Option<EffortLevel> }` and `EffortLevel` is a four-word ladder `Minimal | Low | Medium | High` — the vocabulary the harness-side and provider-side wires can all be mapped onto without inventing precision. The Anthropic decoder carries `thinking: {type: "enabled", budget_tokens: n}` as `Some(EffortRequest { budget_tokens: Some(n), level: None })` instead of refusing; `thinking: {type: "disabled"}` as `None`. A `thinking` *block* in message content stays refused (replaying reasoning to another vendor's model is a different question the recon did not open). **This changes today's all-or-nothing answer** for every Claude Code request that sets `thinking` on a translated pair — from a 400 to a served request — and that is the one decision here: a request the user's harness made with thinking on is better served at the target's nearest effort than refused, and the pair table's field rows say what happened to it (`FieldRows.effort`, the `CacheDisposition` shape: carried-as / stripped-with-reason).

**How each target takes it — to be pinned by the packet against the providers' current documentation, never from memory.** OpenAI Chat and Responses both accept a discrete effort word (`reasoning_effort` on Chat; `reasoning: { effort }` on Responses) on reasoning-capable models; the packet reads the vocabulary from the docs the day it is written, maps `budget_tokens` onto it by fixed thresholds stated once in `canonical.rs` with their reason, and emits nothing on a model the provider does not document as accepting the field (the field rows then say *stripped: model does not reason*). Gemini takes a token budget (`generationConfig.thinkingConfig.thinkingBudget`) on thinking-capable models; `budget_tokens` maps directly, clamped to the model's documented range, and a level maps to a documented fraction of that range. Where the harness sent a budget and the target wants a word, the mapping is the codec's; where the reverse, likewise — and both directions are pure functions pinned by unit tests, so 2016/2017's determinism holds.

**What is never done.** No effort is invented: a request that set no `thinking` emits no effort field and the target's default applies, exactly as today. No effort is raised by translation: a mapping rounds *down* to the nearest documented level. The relay path is untouched by construction (the recon confirmed `ingress::forward` never enters a codec).

**Then the measurement, then the clamp.** With effort carried, `GH-EFFORT-CLAMP-SHADOW` becomes dispatchable: per translated exchange, record the turn's shape (a pure tool-resume turn is a last `Role::User` message whose every block is a `Block::ToolResult` — none of today's fixtures has one, so the package writes the first), the effort carried, and the provider's reported `output_tokens`, joined to the session's next harness-reported `TurnEnded` verdict — the ledger's `outcome` column is a transport-level 2xx proxy and cannot judge quality. The clamp itself (`GH-EFFORT-CLAMP`, behind a launch-profile switch off by default) is offered only if the shadow's rows show the reduction saves output tokens on tool-resume turns without moving the verdict distribution. That is what *evaluate before offering* means in rows.

**Successor, named:** `GH-EFFORT-CARRY` (Amber, Sonnet high): `canonical.rs` (the field and the ladder), `anthropic.rs` (carry `thinking`; the refusal row and its stale *"no OpenAI Chat equivalent"* text go), `openai_chat.rs`, `openai_responses.rs`, `gemini.rs` (the mappings, each with its documentation citation in the code comment), `mod.rs` (`FieldRows.effort`), `tests/gateway_translate_effort.rs` (a `thinking` request served on each pair with the target's field present and correct; a request with no `thinking` unchanged byte for byte; a non-reasoning model stripped with the reason). Then `GH-EFFORT-CLAMP-SHADOW` (Amber), then — evidence permitting — `GH-EFFORT-CLAMP`.

## Memory is the project's, not the launch path's — user ruling 2026-09-02

**The instruction.** Told that a plain `glasshouse launch` gets no memory briefing while a session spawned through the machine door does, the user ruled: *"memory should be project specific, not by how a user starts it."* The briefing is a property of the project, and every session Glasshouse starts in that project receives it the same way.

**What was true.** `api/unix.rs::select_memory`/`deliver_memory` brief a door-spawned session with `memory::inject::briefing` and record the delivery as a `MemoryRetrieved` row carrying the session id; `main.rs::launch_session` never calls either. The split was never designed — the door was built for Phase 27's *"before Glasshouse automatically sends a routed task to a session"* and the CLI launch predates it. It is the structural gap `GH-RETRIEVAL-ATTRIBUTION` found under 1821/1831 from the other side (a briefed session is never routed, a routed session is never briefed), and it is a product defect by this ruling, not an inconsistency.

**How the CLI launch briefs.** Through the harness's own mechanism first, as the response profile already does: `harness::response::apply`'s ladder — a native mechanism, an additive instruction (`--append-system-prompt` on Claude Code, whatever each adapter declares), nothing with a stated reason — takes the briefing block beside the profile's additive text, labelled as Glasshouse project memory (line 1130), bounded (1127), never replacing a system prompt. Where an adapter declares no additive mechanism, the door's shape — a separately labelled machine message into the session — is the fallback where a session runtime holds the PTY, and a stated *not briefed: <reason>* line otherwise. The selection is the door's: `briefing(store, query, already)` with the query being the checkpoint text when the launch resumes from one, and the **standing set** otherwise — the current binding memories and recent failed attempts, exactly what `memory export-local` writes — because a launch with no task has no relevance query and line 1134's *small number of current high-authority memories* is the honest answer. Every delivery records `MemoryRetrieved` with the session id, so the memory-quality proxy covers manual sessions too. Opt-out, not opt-in: `[memory] inject_at_launch = false` (project or user), and `--no-memory` on the launch, for the user who wants a bare harness.

**Successor, named:** `GH-LAUNCH-BRIEFING` (Amber, Sonnet high) — it outranks the queued packages by this ruling.

## A session identity on the routing evidence rows — Cluster G's first column, designed 2026-09-02

**Why a note.** Phase 58's last two lines both need a gateway-written row to name the session it served. 2019's *per-session cache ratio* has no producer because `routing_observations` (migration 11) carries no session column; 2039's shadow measurement must join a translated exchange's effort and output tokens to the session's next harness-reported `TurnEnded` verdict — a row `evaluation_observations` already keys by `session_id` — and cannot without the same column. The refusal register filed both under Cluster G, *needs a schema migration this project refuses casually; design first*. This is the design.

**Which identity.** Glasshouse's own session id — the `sessions.id` value — and nothing else. Not the harness's `metadata.user_id`: carrying it would mean the relay reading a body it never reads, by a rule `an_exchange_has_nowhere_to_put_a_body` enforces, and it names an account the ledger has no business holding. Not the native session id either: `sessions.native_session_id` already resolves that mapping, and Glasshouse assigns the native id itself at launch. The Glasshouse id is the value `record_turn_outcome` already writes on `TurnOutcomeObserved` and `deliver_memory` on `MemoryRetrieved`, so every join this column exists for is on one value with no translation step.

**Where it comes from — the launch, not the wire.** A gateway is started once per launched session (`launch_session` and `resolve_resume_overlay` are the two doors, and a source-scanning test counts them). After the session record exists and before the harness is spawned, the launch tells the gateway which session it serves — `SessionRouting::serve_session(SessionId)`, a setter in the shape of `set_pairing_preference`, held in `State` beside `assignment` — and `record_routing_observation` stamps it on every row the gateway writes, translated or relayed. A gateway nothing has told is a gateway serving no session, and its rows say so with `NULL`. The launch's own router row (`record_routing_latency`, written before the record exists) stays `NULL` and says why in its doc comment; it is a row about the routing decision, not about a served exchange.

**Two more columns, filled at the same seam, so 2039's shadow needs no second migration.** `translate::serve` holds the decoded canonical request before the exchange is opened, and two facts of it are pure functions of the request alone: the effort carried — `Request::effort` reduced to its four-word `EffortLevel` through `EffortRequest::level`/`level_for_budget` (one vocabulary whatever the target's wire spelled) — and the turn's shape — *tool-resume* when the last `Role::User` message's blocks are all `Block::ToolResult`, *prompt* otherwise. Both ride on `Exchange` as enums (names, which its scanning test permits; never a `String`) and land in `routing_observations.effort_level` and `routing_observations.turn_shape`. A relayed exchange, whose body is never read, records `NULL` for both: unread, not absent.

**The migration — 24, migration 23's shape and its reasons.** `ALTER TABLE routing_observations ADD COLUMN session_id TEXT`, `ADD COLUMN effort_level TEXT`, `ADD COLUMN turn_shape TEXT`. Nullable, `NULL` backfilling every existing row ("this build recorded none", never "none"). No `CHECK`: the vocabularies are Rust enums pinned by tests, as `task_class` is. No `REFERENCES`: migration 12's rule, and a row must outlive its session's deletion as the evaluation rows do. No index: the readers are bounded window passes `routing_observations_by_route_time` already serves — migration 15's *measure before indexing*. An unrecognised stored word reads back as `None`, `task_class`'s rule, because both are bucketing inputs and not facts a reader may not lose.

**What reads it.** 2019's per-session clause: the `SAVINGS` section of `routing-cost` groups the translation facet by `session_id` beside the per-credential grouping it already prints — the ratio per session with its denominators, words where nothing was recorded, and the standing sample floor. 2039's shadow (`GH-EFFORT-CLAMP-SHADOW`, the successor): rows with `turn_shape = 'tool-resume'` and an `effort_level`, their `output_tokens`, joined by `session_id` to the session's next `TurnOutcomeObserved` after `observed_at` — the ledger's own `outcome` column being a transport 2xx and never a verdict.

**What is not done.** No harness identifier on the row. No body read on the relay. No rewrite of existing rows. No clamp: the shadow measures and the clamp is offered only if its rows say the reduction saves output tokens on tool-resume turns without moving the verdict distribution.

**Successors, named:** `GH-OBSERVATION-SESSION-COLUMN` (Red — a migration and a session identity; Opus specialist, high) closes 2019; then `GH-EFFORT-CLAMP-SHADOW` (Amber, Sonnet) closes 2039 and Phase 58.

## Preferring a cheap metered classifier over an unreliable free one — the ruling line 1439 needed, 2026-09-02

**The line.** *"Prefer a cheap metered model over an unreliable free model when failed routing attempts would cost more time than the price difference."* Its refusal was always the same: no price reached the classifier's candidates. `GH-CLASSIFIER-PRICE-CEILING` put one there, and `phase-34c.md` recorded the shape of the comparison — *(1 − parsed_fraction) × median latency* against the price difference — which mixes milliseconds and micro-dollars and so needs an exchange rate nobody should invent in a packet.

**The ruling: no exchange rate.** Glasshouse does not price a person's time. Both quantities are compared against thresholds the user has already stated in their own units, and the preference fires only when both statements are exceeded in the direction the line names:

- A free candidate is **unreliable enough** when its expected wasted time — `(1 − parsed_fraction) × median_ms` over its classification record, both already read for lines 1432 and 1435 — exceeds `[routing] max_router_latency` (`RouterLatencyMs`), the user's own statement of how much routing latency a turn may bear. Below the reliability sample floor the record is *unmeasured* and the preference is inert, as every measurement term here is.
- A metered candidate is **cheap enough** when `estimated_classification_cost_micro_usd(price)` is at or below `[routing] max_marginal_cost` (`RouterCostMicroUsd`), the user's own statement of what one decision may cost — the same ceiling 1436 excludes on. An unpriced candidate is never *cheap enough*; unpriced is not cheap.

When a free candidate that would otherwise be chosen is unreliable enough and an admitted metered candidate is cheap enough, the metered candidate is preferred, and the explanation says both things with their figures: *"free `<model>` expects `<n>` ms of wasted retries per call, over the `<limit>` ms routing-latency limit; metered `<model>` at ~$`<x>` per call is under the $`<y>` ceiling (map line 1439)"*. Otherwise nothing moves. This is a preference among the metered fallback and the free-tier choice at `choose_for_automatic_classification`'s seam; it never reorders the free selection among free candidates (`scoring_never_reorders…`'s invariant), and it never fires when no metered candidate is priced.

**Why these two knobs and not a third.** A `time_is_worth_usd_per_hour` setting would be a number the user has no way to know and Glasshouse no way to verify. The two existing settings already encode the user's tolerance on each axis, and the line's word *cheap* is relative to the user's ceiling, not to the free model's zero.

**Successor, named:** `GH-CLASSIFIER-TIME-PRICE` (Amber, Sonnet high) — `routing/disposable.rs` only, once the price lands; one preference, two figures in the explanation, tests through the shipped binary with a planted classification record and `pricing.toml`. 1419 is *not* unlocked by this: *the premium capacity it protects* still has no producer at the classification site.

## Counting money spent against the user's budget — the ruling lines 1263 and 1519 waited on, 2026-09-02

**The gap, verified.** `[providers.<name>.quota] budget` (`MonetaryBudget`: an amount in micro-USD and a period, calendar month or rolling thirty days) has been a ceiling with no counter since Phase 32A: `apply_user_configuration` sets the pool's *limit* and leaves *remaining* unmeasured, `glasshouse resources` prints *"Glasshouse does not count spend against this"*, and the register files 1263 (*lower the score when the budget is close to exhaustion*) and 1519 (*exclude candidates whose budget has been exhausted*) under one missing thing: a count of what has been spent. The token counter that does exist — `recent_credential_spend`, feeding the entitlement's token ceiling — is tokens, not money, and its own doc says so.

**What can be counted now, and what cannot.** Since Phase 56 a translated exchange writes `input_tokens` and `output_tokens` on its row, and since Phase 32G `pricing.toml` prices a `(provider, model)` per million tokens. Money is therefore a **read-time product**: for every served row in the budget's period with a token count and a known price, `input × input_rate + output × output_rate`, summed in micro-USD. A relayed exchange has no token count (the relay reads no body — the standing wall) and a model absent from `pricing.toml` has no rate; neither is *zero spend*. So the reading carries what it could not price — *N exchanges uncounted: M unread, K unpriced* — and the readout says so beside the figure, the way the cache ratio says *not counted*. **Never a `Some(0)` for an exchange nobody could price.** The sum is per provider, narrowed to the credential when every counted row names one — `recent_credential_spend`'s rule, verbatim, because under-reporting is the direction that lets a ceiling be exceeded.

**Where it goes.** A reader beside `recent_credential_spend` in `routing/evidence.rs` — `recent_credential_cost(observations, provider, label, prices, since_unix) -> CredentialCost { micro_usd: Option<u64>, priced_rows, unread_rows, unpriced_rows, account_narrowed }` — over the period's rows (`BudgetPeriod::CalendarMonth` is since the first of the month in local time; `RollingThirtyDays` is `now − 30 × 86 400`). `provider/resources.rs::observed_capacity` hands it into `apply_user_configuration`, which now sets the budget pool's *remaining* to `limit − spent` (saturating at zero) with `ReadingSource::Ledger` and the reading's own stamp, leaving *remaining* unmeasured exactly as today when no row could be priced. Nothing else in the capacity model changes: `remaining_capacity_score` already reads `user_budget` through `pools()` and already lowers as a measured pool empties — that is 1263, with no new term. `glasshouse resources` prints the counted spend, the uncounted breakdown, and drops the *does not count* sentence.

**The exclusion (1519).** A destination or disposable candidate whose provider's budget pool reads *remaining = 0* is excluded by name — `HardConstraint::Entitlement`'s shape, a new `EntitlementRefusal::BudgetExhausted { budget_micro_usd, spent_micro_usd, period }`, raised where `spend_constraint` (the token ceiling) is raised on the session path and mirrored at `disposable_candidates` on the support-work path — never on an unmeasured pool: a budget nobody could count against excludes nothing, which is the same fail-open the token ceiling keeps and the same honesty 1436 keeps for an unpriced candidate. A budget the user set to zero with rows priced against it is exhausted at the first priced exchange; a free-tier candidate is never excluded by a money budget (its rows price to zero by `Cost::Free`'s own meaning — say so in the reason).

**What is not done.** No new column, no migration: the price is applied at read time so a corrected `pricing.toml` corrects history. No estimate for unread relay exchanges. No spend shown per session (the per-session readout is 2019's and stays where it is).

**Successor, named:** `GH-BUDGET-SPEND-COUNTER` (Amber, Sonnet high) closes 1263 and 1519 and completes Phase 35A; `routing/evidence.rs` (the reader), `provider/telemetry.rs` (`apply_user_configuration` takes the spend), `provider/resources.rs` (the gather and the wording), `routing/mod.rs` (the refusal), `routing/session.rs` and `main.rs::disposable_candidates` (the two exclusion sites), and shipped-binary tests planting priced rows against a budget. Dispatch after `effort-clamp-shadow` and `task-class-cost-join` land (they hold `routing/evidence.rs` and `routing/session.rs`).

**Amended 2026-09-02, later the same evening — the first ruling was vacuous, and the worker proved it.** `GH-CLASSIFIER-TIME-PRICE` showed that expected wasted time, `(1 − parsed) × median`, is never more than the median, and line 1435 already excludes any candidate whose median exceeds `max_router_latency`; with line 1432's 80 % parse floor on top, an *admitted* free candidate wastes at most a fifth of the limit by construction. So the "unreliable enough" test above could fire only on a candidate the verdict had already removed — an account of an exclusion, never a preference. The comparison the line actually names is between two **times**: the free candidate's expected wasted retry time against the time the metered call itself takes. **Amended rule, still with no exchange rate:** a free candidate is *unreliable enough* when its expected wasted time — `(1 − parsed/outcomes) × median_ms` over its own record, above the reliability sample floor — **exceeds the metered candidate's own median classification latency** over the same floor (both from the same `ClassificationRecord` readings line 1435 already reads); a metered candidate is *cheap enough* exactly as before (its estimated call cost at or below `max_marginal_cost`). Either candidate unmeasured → inert, with the note saying which. The preference is asked about the free candidate the router would pick **from the admitted list** and the cheapest admitted priced metered candidate, so what it prefers is always something every other gate accepted, and when it fires it changes the choice. `max_router_latency` stays 1435's and plays no part here. The successor packet is re-issued to the same worker with this rule; the first implementation's seam over the pre-admission list is withdrawn.

**Landed 2026-09-02 (late evening), with three departures the code taught.** The remaining reading's source is `ReadingSource::LocalObservation("priced spend against the configured budget")` — there is no `ReadingSource::Ledger`, and the local-observation variant is the one for a number Glasshouse counted itself. The free-tier rule is enforced at the two sites by the *destination's* cost, not by pricing its rows to zero: `hard_constraint` asks the budget only for a non-free backend, and `disposable_candidates` skips only non-free models — and because a provider's `CapacityState` is one value shared by its free and metered models, a free model's capacity is computed with that provider's budget spend stripped, or the zeroed budget dimension would have excluded it through the router's own zero-headroom gate. The calendar-month start is the OS's own local time (`localtime_r`/`mktime`; on Windows `localtime_s` and the UCRT's `_mktime64`, which `libc` does not bind — the worker's arm called a `libc::mktime` that does not exist there, and was fixed at integration). The gather is provider-wide (`[providers.<name>.quota]` has no per-credential concept), so the narrowing rule never fires. Not yet wired: the context-firewall reducer's chooser and, once it lands, the reranking seat's — `GH-BUDGET-SPEND-REMAINING-CALLERS`.

## A reranking seat in the disposable router — Phase 24's five lines, designed 2026-09-03

**Why now.** Phase 24 was refused whole because *no cheap model is wired up in this build*. Since batch 87 the disposable router calls what it chooses (`GH-ROUTED-EXTRACTION-CLIENT`): `RoutedModel` resolves a credential, makes the request, observes health, releases its reservation. `JobKind::Reranking` has existed on the router's job axis since Phase 56. The seat is the same one extraction uses; only the prompt and the consumer differ.

**Where it sits.** `memory::inject::briefing` asks `search_grouped_for_injection` for up to `CANDIDATE_LIMIT` (40) lexical matches, grouped into *invariants and constraints* (always first) and *other*, then filters current, unreaffirmed-idea and already-injected records and takes `MAX_INJECTED_MEMORIES` (3). The reranker slots between the grouping and the take, on the **other** group only: invariants and constraints stay first by authority, never by a model's opinion.

**Optional, offline first (1090).** A reranker exists only when `[memory] rerank_model = "<provider>/<model>"` names one — the exact shape of `extraction_model` and the same consent rule: no knob, no call, lexical order stands. Every failure — no candidate resource, no credential, a refusal, a timeout, an unparseable reply, an id not in the candidate set — is a **bypass with a stated reason**, never an error the caller sees: the lexical order is returned and the reason travels with the outcome. Memory search and briefing therefore work exactly as today with no model anywhere.

**Bounded (1091).** Only the first `RERANK_CANDIDATES = 8` of the *other* group, in lexical order, are sent — a stated constant with its reason (a handful of short subjects plus one-line bodies stays in a few hundred tokens; the reply is a list of ids). Bodies are truncated per candidate at a stated byte ceiling in the prompt. One call per briefing, never one per candidate, and never when the group has fewer than two candidates (nothing to reorder).

**The prompt (1092).** A fixed contract in the style of `schema::PROMPT_CONTRACT`: the task text (already capped at `MAX_QUERY_CHARS`), then the candidates as `id · kind · updated <days> ago · <subject> — <body excerpt>`, and the instruction to return a JSON array of ids ordered by usefulness **for this task**, preferring recent over stale when equally relevant, dropping any candidate that duplicates a higher one, and never returning an id that was not given. *Active status* is not delegated: `is_current` filtering stays in Rust after the reranker, so a superseded record can never be resurrected by a model. The reply is parsed strictly; unknown ids are a bypass reason; omitted ids are appended in lexical order after the returned ones (a reranker may demote, never hide).

**Diagnostics on request (1094).** `[memory] retrieval_diagnostics = true` (project over user, default off) makes every briefing append one JSON line to `<project state dir>/memory-retrieval.jsonl`: the query, the lexical candidates with their group and score order, whether a reranker ran and which resource, the returned order or the bypass reason, and the final selection. Nothing is written when the flag is off; the file is project-scoped like every runtime artifact; no memory body longer than the prompt excerpt is written. `glasshouse memory search --explain` prints the same record for one query without writing it.

**The seat.** `main.rs::disposable_rerank_model(runtime, session)` beside `disposable_extraction_model`, the same four steps (consent, local bypass, choice, client) with `JobKind::Reranking`, handing `briefing` an `Option<&dyn ExtractionModel>` — the existing trait, whose `Prompt` gains a `from_text` constructor for a contract that is not an extraction chunk. Both doors (`api/unix.rs::select_memory` and `main.rs::brief_launch_session`) pass the seat's answer; `select_briefing`'s signature grows by that one parameter.

**What is not done.** No reranking of the standing set (a plain launch has no task to rank for); no embedding, no vector store (Phase 52's criteria stand); no learning from ratings. The reranker's verdict never overrides authority ordering, currency filtering, the injection cap or the byte budget.

**Successor, named:** `GH-MEMORY-RERANKER` (Amber, Sonnet high) closes 1089, 1090, 1091, 1092 and 1094 — Phase 24 complete.

## First real token and first tool call on the translated path — the 1331/1332 ruling, 2026-09-02 (late evening)

**The wall, re-read.** Line 1331 stayed open on the register's ruling that *"when the protocol exposes them"* is about the protocol and not about what Glasshouse chooses to look at, and that the relay's refusal to parse a body (`gateway::ingress`, Cluster L) is deliberate and stands. Both halves are still true. But since Phase 56 a **translated** exchange is not relayed: the seam decodes every provider event into the canonical vocabulary in order to re-encode it for the harness, and the row already carries three facts derived from exactly that decoding — `tokens` (Phase 56), `effort` and `turn_shape` (migration 24). The first real token and the first tool call are the same kind of fact on the same path: a clock reading taken when a canonical event the seam had to decode anyway goes past. Nothing new is parsed, and the relayed exchange keeps writing `NULL` for both, as it does for `tokens`.

**The rule (1332), stated in the canonical vocabulary so it cannot drift per provider.** The *first real token* is the instant of the first `StreamEvent::BlockDelta { delta: Delta::Text(s) }` whose `s` contains a non-whitespace character. The *first tool call* is the instant of the first `StreamEvent::BlockStart { block: BlockStart::ToolUse { .. } }`. The three exclusions the line names fall out of the vocabulary rather than being checked one by one: an SSE comment line (`: keep-alive`) is dropped by `SseReader` and never becomes an event; a `ping` event decodes to no canonical event at all (`anthropic.rs`, `"ping" => Ok(Vec::new())`); a thinking or signature delta is refused at decode (`reason("thinking block")`), so a reasoning-only delta is never a canonical event — and a decoder for another protocol that ignores a field it does not carry produces no `Delta::Text` for it either. A whitespace-only text delta *is* a canonical event and is the one case the rule must check by content, which is why the rule is written on `Delta::Text` and not on `BlockDelta`. On a document (the provider answered without streaming, or the harness asked for a document and the stream was gathered), both instants are the document's own `first_byte_at` when the response contains at least one qualifying text block or tool-use block, and `None` otherwise — the protocol exposed no finer boundary, and the row says so by equality rather than by an invented offset.

**Where it goes.** `translate::serve` already takes `first_byte_at` the instant the upstream's headers are in hand (`translate/mod.rs`, before `is_event_stream` is decided) and builds the `Exchange` through its `finish` closure. The two loops that feed the decoders — the streamed one that writes each canonical event to the harness as it arrives, and the gathered one that holds them for a document — note the two instants as the events pass, and `finish` carries them onto `Exchange` beside `first_byte_at`; `SessionRouting::record_routing_observation` writes them through two new builders beside `with_first_byte_at`, into the two columns migration 11 already created and nothing has ever written (`first_token_at_unix`, `first_tool_call_at_unix`). `glasshouse routing-cost` prints a mean time-to-first-token and time-to-first-tool-call per group beside its time-to-first-byte, each *not recorded* for a group with no timed row — the readout is the consumer, as it was for the first byte.

**Resolution, and the successor.** Every timestamp on the row is a unix second and these two are no different; at that resolution *time to first token* is nearly always zero or one, which is honest and nearly useless for comparison. The millisecond offsets Phase 33B's TTFC family needs (1347–1352, 1355) are a schema decision — Cluster G, designed with the same care as migration 24 — and the named successor: `GH-STREAM-TIMING-MS` (Red), one migration adding millisecond offsets from dispatch for first byte, first token, first tool call and completion, written from the `Instant` the gateway already takes. This package writes the seconds and records the limit; it does not pre-empt the migration.

**Successor, named:** `GH-STREAM-FIRST-EVENTS` (Amber, Sonnet high) closes 1331 and 1332: `gateway/translate/mod.rs` (the two loops and `finish`), `gateway/ingress.rs` (`Exchange`'s two fields), `gateway/session.rs` (the record), `routing/evidence.rs` (two builders, the aggregate's two means), `main.rs` (`routing-cost`'s two lines), and a shipped-binary test through the gateway fixture whose stream sends a keep-alive comment, a `ping`, a whitespace-only text delta, then — after a pause of more than a second — real text, then a tool-use block after another pause, asserting the three instants' order and gaps on the recorded row.

## The session router's rationale row — Phase 47's 1757 and 1766 on the durable sink, 2026-09-02 (late evening)

**Where this stands.** *A durable observation sink* (above) chose the root over the leaves and its first package, `GH-DISPOSABLE-ROUTE-SINK`, made the **disposable** router's rationale durable (`EvaluationKind::DisposableRouteDecided`, the rationale as `detail`, the shell's `d` key as the reader) and claimed no box, as its scoping said. The **session** router's rationale — why `glasshouse launch` chose the session or destination it did — still ends at a `tracing` line, though it is in hand at both moments the launch records its decision: `Routed::explanation()` is on the value `record_routed_session` is called with, on the continued-session branch and the fresh one.

**The row.** One new variant, `EvaluationKind::SessionRouteDecided`, written beside `record_routed_session`'s two rows at the same instant with the same `session_id`: `subject` is the chosen destination id, `detail` is the explanation as a compact JSON array of `{name, magnitude, evidence}` built from `Contribution`'s accessors — structured, because 1766 needs to rank by magnitude and a rendered string cannot be ranked; no task text, no memory body, nothing the explanation's own evidence strings do not already say. No migration, no new table, no `CHECK`: `evaluation_observations` is the sink and was built to take a variant per producer.

**The views.** 1757's *debug view showing why the router chose a session or resource* is `glasshouse sessions show <id>`: a `routing rationale` block, one line per contribution — `name`, signed magnitude, evidence — read from the session's newest `SessionRouteDecided` row, and `-` for a session with none (started before the row existed, or spawned through the machine door, which is not routed). 1766's *strongest measured factors behind the most recent routing decision in concise text* is one line in `glasshouse status`: the newest row's destination and its three largest contributions by absolute magnitude, `name ±m` each, or *none recorded*. A CLI readout is the view, the precedent 1762/1764 set (*the missing link was one query*); the shell's session view may draw the same row later and closes nothing further.

**What it is not.** It records the decision the launch actually made — never a recomputed explanation, which the batch-50 refusal rightly called *the factors of a decision that was never made*. It does not touch the disposable sink or its reader.

**Successor, named:** `GH-ROUTE-RATIONALE-SINK` (Amber, Sonnet high) closes 1757 and 1766: `evaluation/mod.rs` (the variant, `record_session_route` beside `record_disposable_route`, two readers beside `recent_of_kind`), `main.rs` (the two launch sites, `session_detail`, `status_report`), a shipped-binary test through the launch fixture `tests/evaluation_observations.rs` already drives.

## Tool rounds and repairs on the translated path — 1334's last two quantities and 1350, 2026-09-02 (night)

**The same reading as the first token.** `GH-FAILURE-TAXONOMY` left 1334 open on `tool_rounds` and `repairs`, *"a turn structure and a body this layer cannot see"*. The relay still cannot. The translated seam already decodes both halves of every exchange: the **request** as `canonical::Request`, whose `Block::ToolResult { is_error, .. }` blocks are what `turn_shape` is derived from today, and the **response** as canonical events, whose `BlockStart::ToolUse` are what `FirstEvents` stamps. Counting them is no new parsing.

**The two quantities, per exchange.** `tool_rounds` is the number of tool-use blocks the response requested — the rounds this exchange *began*; `repairs` is the number of tool-result blocks in the request that carried `is_error: true` — the harness's own report that a previous call failed and this exchange is the model repairing. Neither is a judgement: both are counts of blocks the protocol names as such. A relayed exchange writes `NULL` for both, as for every other decoded fact. A response with no tool-use block and a request with no error result write `0`, not `NULL` — the seam looked and found none, which is a different fact from not looking.

**What *successful* means (1334, 1350).** A round is successful when the harness came back with its result and no error — so over a session's consecutive translated rows, *successful tool rounds* is rounds begun minus repairs, derived by the reader and never stored as a fourth column. 1350's *tool rounds per minute of serving time* is `routing-cost`'s per-group line: rounds begun, repairs, and rounds per minute over the group's summed exchange durations (`duration_ms`, the seconds-resolution figure the row has), printed beside the first-byte, first-token and first-tool-call figures — *not recorded* for a group with no counted row. It is an outcome-adjacent measure and is printed as one, never folded into a quality score (1355's own rule, kept).

**Successor, named:** `GH-TOOL-ROUNDS-ON-TRANSLATED` (Amber, Sonnet high) closes 1334 and 1350: `gateway/translate/mod.rs` (the two counts beside `FirstEvents`, the request count at `serve` where the request is decoded), `gateway/ingress.rs` (`Exchange` carries both), `gateway/session.rs` (the record), `routing/evidence.rs` (`with_tool_rounds`/`with_repairs` beside `with_retries`; the aggregate's rounds, repairs and serving seconds), `main.rs` (`routing-cost`'s line); shipped-binary tests through the same fixture `gateway_first_events.rs` uses.

## Millisecond offsets on the routing row — Cluster G's second column set, designed 2026-09-02 (night)

**Why a schema change, and why now.** Every timestamp on `routing_observations` is a unix second: `dispatched_at`, `first_byte_at`, `completed_at`, and since wave 102 `first_token_at` and `first_tool_call_at`. `duration_ms` is that difference times a thousand. At one-second resolution *time to first byte* and *time to first token* are zero or one on nearly every exchange, which is honest and useless for the comparisons Phase 33B asks for — TTFC as the responsiveness measure for tool-using work (1347), TTFT kept apart from it (1348), decode tokens per second (1349), effective TTFC (1351), reliability-adjusted latency in route comparison (1352), the five figures shown separately (1355). Their producer is no longer the parsing wall (the translated seam decodes what it needs — *First real token and first tool call on the translated path*); it is resolution, and resolution is a column decision. Cluster G's rule holds: designed here first, one migration, in migration 24's shape.

**The columns.** Migration 25 adds four nullable `INTEGER` columns to `routing_observations`, each a number of milliseconds **since dispatch**, never an absolute instant: `first_byte_ms`, `first_token_ms`, `first_tool_call_ms`, `completed_ms`, with `CHECK (col IS NULL OR col >= 0)` as the token columns carry. Offsets rather than instants because a monotonic clock (`Instant`) is what the gateway can read at millisecond precision and a wall clock is not; the existing second-resolution `*_at` columns stay, are written exactly as today, and remain the row's only absolute timestamps. `NULL` keeps its meaning: this producer did not measure.

**Dispatch, honestly.** The gateway's seconds `dispatched_at` is, by its own comment, the instant the connection was handed to `ingress::serve`, not the instant the request left for the provider. The offsets' zero is the latter: the `Instant` taken immediately before the upstream request is sent (`ingress::forward` for a relayed exchange, `translate::serve` before `agent.run` for a translated one), threaded to the seam and the relay as a value, not a global. `first_byte_ms` is that instant's `elapsed()` when the upstream's headers are in hand (both paths — the relay already takes `first_byte_at` there and reads no byte of the body); `first_token_ms` and `first_tool_call_ms` are `FirstEvents::note`'s clock reading, which becomes an `elapsed()` beside the seconds it already stamps (translated only); `completed_ms` is `elapsed()` when the exchange ends, both paths.

**Readers.** `RoutingObservation::duration_ms` returns `completed_ms` when present and falls back to the seconds difference otherwise, so every existing consumer improves silently; `consumption_by_purpose` and `classification_record`'s medians compute over the ms columns when a row carries them and over the seconds fallback when not, saying which in the aggregate (`ms_sample_count`); `routing-cost` prints `time to first byte`, `time to first token` and `time to first tool call` from the offsets and marks a group whose rows predate migration 25 *seconds only*. Nothing scores on these yet: 1352's route-comparison term is its own package after the columns exist, and 1349's tokens-per-second is `output_tokens / (completed_ms − first_token_ms)` — a reader, once both halves are on the row.

**What it is not.** No wall-clock millisecond timestamps (a clock step would make an offset negative — the `CHECK` refuses it and the producer never computes one from two wall readings); no ms on the support-work rows written by `record_extraction_observation` until that producer takes an `Instant` of its own (a Green successor); no change to the relay's parsing wall.

**Successor, named:** `GH-STREAM-TIMING-MS` (Red — a migration; Opus 5 high): `database.rs` (migration 25, `SUPPORTED_SCHEMA_VERSION = 25`, the bootstrap test's expected column list, and every literal `version, 24` pin in the test tree — grep before dispatch, they are not traced by the blast radius), `routing/evidence.rs` (fields, builders, `duration_ms`, the aggregates), `gateway/mod.rs`, `gateway/ingress.rs`, `gateway/translate/mod.rs` (the `Instant` threaded, the three `elapsed()` readings), `gateway/session.rs` (the record), `main.rs` (`routing-cost`). Dispatch after `GH-TOOL-ROUNDS-ON-TRANSLATED` lands — it holds the same four gateway files. Then, on the columns: `GH-TTFC-READOUT` (1347, 1348, 1349, 1355 — readers) and `GH-EFFECTIVE-TTFC` (1351, 1352 — a term).

## Re-calibrating the headroom estimator when the quota regime changes — 1247's reachable half, designed 2026-09-02 (night)

**The line and its two halves.** *Reset or re-calibrate an estimator when Glasshouse detects a plan change **or** materially different quota behavior.* The register filed it under Cluster H: *detecting a change needs two readings, and nothing keeps the earlier one.* Half of that is still true and stays: no harness reports a plan in production (`CapacityState::plan(...)` has no production caller), and a configured plan changing is a configuration edit, not a detection. The other half has both readings today and simply never compares them: `GatewayQuotaCache::try_store` overwrites the provider's persisted reading on every exchange that carried rate-limit headers, and `load` hands the previous one back — the store is the one place the earlier and the later reading meet, and it looks at neither.

**The detector.** Before it writes, `try_store` loads the provider's previous persisted reading. A **regime change** is a difference in a *stated ceiling* — `limit`, `window_seconds` or `token_limit` — between the previous and the new headers, both present; a change in `remaining`, `reset` or `retry_after` is the pool being used and never counts. On a regime change the new reading persists `regime_changed_at_unix = observed_at_unix`; otherwise it carries the previous value forward (or `None` when there has never been one). A reading with no previous file records no change: the first reading is not a change from nothing. `PersistedGatewayReadingFields` gains the one optional field with a serde default, so a file written before this reads as *no change recorded* and the format version stands.

**The reset.** The estimator is derived on read from the rows in its window (Phase 32C's own architecture: no persisted estimator state). *Re-calibrating* it is therefore one floor: `populate_provider_facets` (the estimator's single caller) drops every observation older than the provider's `regime_changed_at` before calling `estimate_subscription_headroom`, and passes the instant along so `SubscriptionHeadroomEstimate` carries `since_unix: Option<i64>` — the regime the estimate describes. The render says so: `headroom estimate: ~<band> (<scope>, <n> …, <basis>; limits changed <age> ago)` in `resources`, `status` and `entitlements`. Rows before the change are not deleted and not relabelled; they are simply not evidence about the regime the provider is in now.

**What it is not.** No plan-change detection (no producer); no persisted estimator; no new column on the ledger (the instant lives in the quota cache file that already belongs to this reading); no learning of a *new* ceiling — the headers state it.

**Successor, named:** `GH-ESTIMATOR-RESET` (Amber, Sonnet high) closes 1247: `provider/telemetry.rs` (the comparison in `try_store`, the field, `load` surfacing it), `config/mod.rs` (the floor at the caller, the instant into the estimate), `routing/evidence.rs` (`since_unix` on the estimate; `estimate_subscription_headroom` takes it as a parameter or the caller pre-filters — say which), `main.rs` (the render), tests through the gateway's header fixtures (`tests/gateway_retry_after.rs`'s shape) and `tests/subscription_estimator.rs`'s.

## Provider cadence learned when no header states it — Phase 33C's last line, designed 2026-09-02 (late night)

**What already exists, and what 1366 actually lacks.** *Learn or parse provider cadence separately from Retry-After remainder values when evidence permits.* The *parse* half landed with the gateway quota cache: `RateLimitHeaders` carries `limit`, `remaining`, `reset` and `window_seconds` read from a provider's rate-limit headers, and `GatheredTelemetry::gather_gateway_quota` turns them into the free pool's `Allowance::RequestPool { limit, remaining, resets_at }` — a cadence, kept apart from `ResourceHealth::declared_wait_remaining`, which is the Retry-After remainder and nothing else (`CooldownCause::Declared`). The two are already consulted by two different router terms (`request_pool_cost`, `cadence_availability`). What no build has is the *learn* half: a provider whose headers state neither a window nor a reset leaves the pool with `resets_at: None`, and `Allowance::is_exhausted` then has nothing to reason from once `remaining` reaches zero.

**The ruling.** When a provider's newest quota reading states no `window_seconds` and no `reset`, and the routing evidence ledger holds at least `MIN_SAMPLE_FOR_SUMMARY` `CadenceThrottled` rows for that provider in the window, the *median interval between consecutive throttles* is the learned window. It is attached to the pool entry with its provenance — `Window::Stated(seconds)` from headers, `Window::Learned { seconds, sample }` from the ledger, or none — and a learned window is used for exactly one thing: `resets_at = last_throttle_at + window` where headers gave no reset, so `is_exhausted` can say *until about <time>* instead of nothing. A stated window always wins over a learned one; a learned window is never written back to the quota cache (it is derived, and the cache holds what providers said); and every readout that prints the pool says which it is holding (`resources`, `entitlements`, and the `cadence availability` term's evidence sentence). Fewer throttles than the floor: no window, no claim — the term says *no cadence is known for `<resource>`*, as it does today.

**Successor, named:** `GH-LAST-LINES-33C-34B` (Amber, Sonnet high) — one package with the 1419 ruling below, because both are the last open line of their phase and both are small: `routing/free.rs` (`Window` on `Allowance::RequestPool`, `is_exhausted` reads it), `provider/resources.rs` (the learner over ledger rows, in `gather_gateway_quota`'s neighbourhood), `routing/session.rs` (`cadence_availability`'s evidence names the provenance), `main.rs` (the two readouts), tests seeding throttle rows through the shipped binary. Mutations: the median replaced by the last interval; a learned window overriding a stated one; the learner running below the floor.

## The premium capacity a classifier protects — Phase 34B's last line, designed 2026-09-02 (late night)

**Why 1419 is not 1436 relabelled.** *Prefer a routing model whose marginal decision cost is materially lower than the premium capacity it protects.* 1436 (closed) excludes a classification candidate whose estimated call cost exceeds the user's own `max_marginal_cost` ceiling — a comparison against a number the person typed. 1419 compares against the *thing being protected*: the capacity the classified task will actually run on. Ticking 1419 on the ceiling would close a line about a comparison nobody makes.

**The protected capacity, ruled.** At `main.rs::automatic_classification_choice` the launch's own destination is not yet chosen — classification runs to inform routing — but the launch *profile's backend* is known to `classification_model`'s caller on the launch path (the profile being launched, its provider and model), and that backend is what a task lands on when classification does nothing. Its per-token price from `pricing.toml` (`PriceTable::price_for(provider, model)`) is the protected capacity's marginal cost. A profile whose backend is a harness's own sign-in or is otherwise unpriced protects capacity this build cannot price, and the comparison is then *inert and says so* — never guessed from a plan name.

**Materially lower, ruled.** A candidate's estimated classification cost (`estimated_classification_cost_micro_usd`, the same figure 1436 gates on) is *materially lower* when it is at most one tenth of the protected backend's cost for the same token count — a ratio, not a threshold in dollars, so it holds across price scales. The comparison is a `Contribution` in `classification_verdict`'s notes named `protected capacity`: `+1.0` when the ratio is at or under one tenth, `0.0` with the reason when either price is unknown or the candidate is free (a free candidate protects everything by construction and 1436 already says so), and a *negative* magnitude, never an exclusion, when the classifier would cost more than a tenth of what it protects — the user's ceiling excludes, this line only orders. The ratio and both prices are printed in the evidence sentence.

**Successor:** the same `GH-LAST-LINES-33C-34B` package: `main.rs` (`classification_model`'s caller passes the profile backend's price as `ClassificationPolicy::with_protected_capacity_price`), `routing/disposable.rs` (`ClassificationPolicy`'s field and the term in `classification_verdict`), tests through `glasshouse classify --explain` or the routing fixture that reads a verdict's notes. Mutations: the ratio inverted; the term excluding instead of ordering; an unpriced backend read as free.

## File paths a memory explicitly references — Cluster G's third migration and Phase 28's last three lines, designed 2026-09-02 (late night)

**Why the register's refusal no longer holds, and what still does.** The register's *Phase 28 scoped* section ruled that map line 1139's qualifier — *track file paths explicitly referenced by durable memories **when extraction can identify them reliably*** — was unsatisfiable, because the extraction model's input is the session's lifecycle events rendered by `describe` and none of them carries a path; the honest producer it found was observation (`WorkingTreeStatus::changed_files`), written to migration 17's `memory_files` as provenance `observed` and deliberately *not* called *referenced*. Both halves of that ruling stand: observed-dirty is still not explicitly referenced, and a path guessed from a dirty list would still be a fabricated producer. What changed is Phase 57. The context firewall registers a `PostToolUse` hook with matcher `"*"` (`harness/claude_code.rs::context_firewall_hook_entry`, merged into the session's settings document at `main.rs::register_context_firewall_hook`), so **every tool call's `tool_input` — including the `file_path` of every `Edit`, `Write`, `MultiEdit` and `NotebookEdit` — already reaches a Glasshouse subprocess** (`main.rs::context_firewall_hook`, which parses it with `firewall::adapter::tool_input_paths`; `firewall::eligibility::HARD_BLOCKED_TOOLS` already names the writing tools as the ones a reduction must never touch). The signal arrives on the production path today; nothing keeps it. This note designs the keeping.

**The record: a lifecycle event, and migration 26.** A new `LifecycleEvent::FileTouched { path }` (kind `file_touched`) with one payload column, `path TEXT` — repo-relative, `/`-separated, never absolute (`memory::store::normalize_observed_path`'s contract, applied by the writer), `CHECK (path IS NULL OR path <> '')` and `CHECK ((kind = 'file_touched') = (path IS NOT NULL))`. SQLite cannot alter a `CHECK`, so migration 26 rebuilds `lifecycle_events` exactly as migration 7 did to admit `gateway_backend_changed`: rename, recreate with the twelfth kind and the new column, copy **with `seq` named explicitly** so `memories.source_event_first`/`source_event_last` keep pointing at the same events, drop, recreate the index and the three triggers. `SUPPORTED_SCHEMA_VERSION` becomes 26. Why an event and not a table of its own: `memory::extract::lifecycle::chunk_for_session` already reads the session's events in order, renders each with `describe`, and derives every memory's provenance range from the kept tail — a second source would need a second ordering and a second range; an event slots in with no new reader. Why this is not the noise `REPORTED_EVENTS` refuses: that list keeps `PostToolUse` out of the *lifecycle state machine*; `file_touched` is appended by the firewall subprocess that already runs on every tool call, `session::lifecycle` treats it as no transition, and every `match` on `LifecycleEvent` in the crate is exhaustive, so the compiler names each consumer that must say so.

**The producer.** `context-firewall hook` gains `--session <id>`, baked into its command line at launch exactly as the lifecycle `report` hook's `--session` is (`harness/mod.rs`, the hook command); `register_context_firewall_hook` has the session's directory and its caller the id. On an event whose `tool_name` is a writing tool, the hook appends one `file_touched` per distinct path from `tool_input_paths` that normalises under the project root (absolute paths inside the root made relative, Windows separators folded, anything outside the root dropped and never stored), through `EventLog::append`, **before** the firewall's own processing and **never** affecting the hook's response — a failed append is a `tracing::warn!` and the reduction proceeds. Read-shaped tools (`Read`, `Grep`, `Glob`) carry paths too and are deliberately not recorded: *touched* means the session changed the file, which is the only association a memory can honestly reference.

**Rendering and the reliability guard.** `describe` renders the event as `edited <path>`; `credentials::scrub` runs over every chunk entry, so a path is scrubbed like anything else. `ExtractedMemory` gains `paths: Vec<String>` (`#[serde(default)]`), and the extraction contract asks the model for the repo-relative paths that appear verbatim in the activity and that the memory is specifically about, empty otherwise. **Reliability is mechanical, not the model's word:** a returned path is kept only when it is byte-equal to a path in the chunk's own `file_touched` set (the entries that survived the budget); every other path is dropped and counted as `paths_dropped` in Phase 47's opt-in extraction diagnostics (ids and counts only). That is what *when extraction can identify them reliably* means here — the model chooses among the paths the session demonstrably edited, and cannot introduce one.

**The rows, and the two consumers that stop pretending.** `FileAssociation::Referenced` (`"referenced"`; `MEMORY_FILE_PROVENANCE` grows to two values — no migration, the column is free text by migration 17's design) and `MemoryStore::record_referenced_files` beside `record_observed_files`. The automatic path writes both: observed from the dirty tree as today, referenced from the model's guarded choice; one memory may carry both for one path. `for_path` reports the strongest association per memory (referenced over observed) through `RetrievalResult::association(&id)`, an accessor in `relevance()`'s shape, and the two consumers that print `observed` unconditionally today — `memory::inject::render_entry` and `api::unix::file_observed_memory_json`, the limit the 1140/1143 entries recorded — read the per-row answer instead.

**1141: a kind preference for an intended edit, inside the ladder.** `for_path` takes a `RetrievalIntent` — `Lookup` (today's order) or `CodeEdit`. Under `CodeEdit`, *within* a ladder rung — the rung stays primary, Phase 21E's rule that an idea never outranks an invariant is untouched — the kinds `Constraint`, `Decision` and `FailedAttempt` order ahead of `Feature`, `Finding` and `Todo`, and `retrieval_weight` decides after that as it does now. A fixed kind class, never a number. The briefing's file section is built for the files the task names — the intended edit — and asks with `CodeEdit`; the socket door stays `Lookup` (its caller asked what a file is associated with, not what to read before editing it); the new `glasshouse memory search --path <p> [--for-edit]` exposes both and closes the CLI limit the 1143 entry recorded.

**1142: advisory by label, stale by commit order, never by judgement.** Every file-aware result says it is advisory: the briefing's file section heading carries *advisory — the source at <path> is the evidence*, the door's rows carry `advisory: true` and a `freshness`, the CLI prints both. Freshness is a fact about commits, not a claim about conflict — the register refused reading source and comparing it to a memory's claim at 828, 829, 862 and 932, and this reads no source. `checkpoint::git` gains `last_change_commit(root, path)` (`git log -1 --format=%H -- <path>`) and `is_ancestor(root, a, b)` (`git merge-base --is-ancestor`). A memory is **stale** when its `source_commit` is a strict ancestor of the file's last-change commit (the file changed after the memory was extracted), **current** when the last change is that commit or one of its ancestors, **unknown** when either side is missing or git cannot answer (no repository, unrelated histories). Stale is a label: it never withholds, reorders or rescores — the comparator never sees git. At most one `git log` per path and one `merge-base` per memory, and the briefing caps at three memories per path. This is the *never treat stale memory as stronger evidence than the current source code* clause done the one way the register's refusals allow: the reader is told which is older, and told the source decides.

**What it is not.** No path inferred from a dirty list (observed stays observed); no `PreToolUse`; no recording of reads; no semantic staleness; no change to the order under `Lookup` or to `MemoryStore::search`; no MCP tool argument; no new table.

**Successor, named:** `GH-FILE-AWARE-MEMORY` (Red — a migration and a hook-path change; Opus 5 high) closes 1139, 1141 and 1142 in one package: `database.rs` (migration 26 in migration 7's shape, `SUPPORTED_SCHEMA_VERSION = 26`, `LIFECYCLE_EVENT_KINDS`, `MEMORY_FILE_PROVENANCE`, the bootstrap test's expected columns, every rollback list and every literal `version, 25` pin in the test tree — the migration ripple batch 93 recorded), `events/mod.rs` and `events/log.rs` (the variant and its column), `session/lifecycle.rs` (no transition), `memory/extract/{lifecycle,chunk,schema,mod,diagnostics}.rs` (render, the touched set, `paths`, the guard, the count), `memory/store.rs` (the association and the writer), `memory/search.rs` (`RetrievalIntent`, the per-row association, the `CodeEdit` order), `memory/inject.rs` and `api/unix.rs` (the labels), `checkpoint/git.rs` (the two git questions), `firewall/eligibility.rs` (the writing-tool predicate), `harness/claude_code.rs` and `main.rs`/`cli.rs` (`--session` on the hook, the recording, `memory search --path`/`--for-edit`), one new integration test file.

## Creating the project database atomically — the bootstrap race's airtight successor, designed 2026-09-02 (late night)

**What wave 108 fixed, and the residual it named.** `GH-BOOTSTRAP-RACE` replaced `check_existing`'s 500 ms timer with SQLite's own write lock: a straggler that finds the database file present and zero bytes long takes `BEGIN IMMEDIATE` on a probe connection and decides afterwards. Its worker and its verifier both named the same residual: the state *"a zero-byte file exists at the final path"* still has **two** meanings — a creator between `create_new` and its own `BEGIN IMMEDIATE`, or a truncated database — and the code tells them apart with a 40 × 10 ms grace that is a probability argument, not a proof (a creator preempted for longer than the grace in that window is refused as truncated). The verifier's F1 added a second edge: every `create_new` loser in a burst goes straight to `open`, where `configure`'s fixed 5 s busy timeout is the bet a first migration must win — the same shape as the defect, one door down.

**The rule.** *A file at the final path is always a complete, migrated, project-bound database or a truncated one — never one in the making.* `prepare_file` creates `<db>.tmp-<pid>-<nonce>` with `create_new` and mode `0600`, opens **that** file, runs `configure`, `migrate` and `bind_project` inside one `BEGIN IMMEDIATE`, commits, closes the connection, and then publishes with `std::fs::hard_link(tmp, final)` — the one primitive that creates the final directory entry atomically with its full content on every platform this project runs (`link(2)` on unix, `CreateHardLinkW` on NTFS; a hard link shares the inode, so removing `tmp` afterwards leaves `final` intact). `AlreadyExists` from the link means a sibling published first: remove the tmp file and its sidecars, open the sibling's — complete by construction. `open` on the final path is then what it is today (`verify_identity`, `BEGIN IMMEDIATE`, `migrate` as a no-op at the current version, `bind_project` as a no-op), and the only contention left is the ordinary one between finished databases. In a burst of *n* first bootstraps, *n* migrations run on *n* private files and one wins the link. **Measured at implementation (the verifier's F1): that is not cheaper — at n = 64 the 64-caller test takes 4.7 s where the lock wait took 0.4 s, because 64 threads each build the whole 25-migration schema instead of one building it and 63 waiting.** The trade is taken anyway: it has no window, production n is one process or a handful of hook subprocesses, and the 64-caller test stays in the default `--lib` run as the field reproduction with its cost recorded rather than hidden behind `#[ignore]`.

**What this removes.** With the rule in force, `check_existing`'s zero-byte case has exactly one meaning again, so the lock wait — `wait_out_a_concurrent_creation`, `lock_probe_connection`, `grew_while_locked`, `CREATION_LOCK_WAIT`, `CREATOR_LOCK_GRACE_ATTEMPTS`, `DatabaseError::CreationWaitTimedOut` — goes, and a zero-byte final file is refused as `EmptyExisting` immediately, with no probe connection (which also retires the verifier's F3: the probe's hot-journal recovery on the refusal path). The 5 s busy timeout in `configure` stays what it is, because no first migration ever runs behind it any more. The 16-caller and 64-caller tests stay byte-identical as the field reproduction; the 2 s deterministic stress test is rewritten to the new mechanism: a creator that holds its private file mid-migration for 2 s, a straggler that publishes first, the creator's link failing, the creator discarding its own work and opening the straggler's — one database, one binding, both callers `Ok`.

**Crash leftovers.** A creator killed before its link leaves `<db>.tmp-<pid>-<nonce>` (and, in the default rollback journal, its `-journal`). `prepare_file` sweeps siblings of the final path matching the tmp pattern whose recorded pid is not alive — `session::supervision::observe`, the production probe with a macOS, Linux and Windows arm, whose `ObservedProcess` also carries the start time that tells a reused pid from the creator — and never touches one whose pid is alive: that is a live sibling mid-creation, and its file is its own. A leftover whose pid was reused by an unrelated process is left in place and named in a `tracing::warn!`; leaking a few kilobytes is the honest failure, deleting a live sibling's work is not.

**Ruling at implementation (2026-09-03, `GH-BOOTSTRAP-ATOMIC`'s decision 2).** The private name carries the creator's *start time* beside its pid (`<db>.tmp-<pid>-<start ms hex>-<nonce hex>`, `ObservedProcess::started_at_ms`), and the sweep treats a live pid whose start time is not the recorded one as **recycled — provably not the creator — and removes the leftover** with the warning the note above reserved for leaving it. The leak-rather-than-delete rule survives exactly where the proof is missing: a creator whose start time the probe could not read records `0`, and a `0` against a live pid is *working*, never *recycled*. The probe cannot tell a Glasshouse process from any other and does not need to; *not the creator* is the whole question. Accepted by the orchestrator: the deletion is now backed by a proof, and the earlier rule was written for a name that carried only the pid.

**What it is not.** No change to `open`'s sequence on an existing database; no change to migration semantics or `SUPPORTED_SCHEMA_VERSION`; no `rename` (a rename would *replace* a truncated file silently, and the refusal of a truncated database is a promise this project keeps); no WAL; no busy-timeout widening.

**Successor, named:** `GH-BOOTSTRAP-ATOMIC` (Red — process lifecycle and the database's creation path; Opus 5 high): `database.rs` only — `prepare_file`, `check_existing` and the removals above, the tmp naming and sweep, the rewritten stress test, and a standing test for a leftover from a dead pid and for a leftover from a live one; the verifier's F4 (`CreationWaitTimedOut` unreached by any test) is answered by the variant's removal.

## Decompression — the user's ruling of 2026-09-03, and what Phase 59 measures

**The ruling, in the user's words.** *Glasshouse is not sloppily agentically assembled. It is extraordinarily conscientious, defensive and seriously implemented — in places too conscientious. The biggest risk is no longer missing quality but complexity through over-assurance: files too large, too much historical documentation, and a process whose evidence system itself has to be maintained and checked. I would already dogfood it seriously. Before a broader release I would not prioritise further large features but a hardening and simplification phase: split modules, shorten redundant explanations, run real long sessions, and close the open items by risk rather than by checkbox count.* And: *we spend a lot of time on conscientious small things and flakes that hold up going forward; a decompression phase to polish Glasshouse and break up the large monolithic structures; keep what was good; changing the process is explicitly allowed.*

**What the numbers say.** 348,661 Rust lines — 218,089 under `src`, 130,541 under `tests`. `main.rs` is 20,563 lines, 16,247 of them production; `config/mod.rs` 7,048 production plus 3,516 of inline tests; `routing/evidence.rs` 5,728 + 2,432; `routing/session.rs` 7,213 with its tests elsewhere; `shell/state.rs` 4,967 + 3,081; `shell/mod.rs` 2,941 + 3,685; `database.rs` 3,251 + 3,068. Twelve files carry more than 2,500 production lines. Comment lines are 31–38 % of production in the six largest. The map stood at 1264/1347 checkboxes, which the user weights at roughly 85 % of product readiness because the open lines include context quality, cache affinity, low-confidence memory, preserving user changes and behaviour near a finished task.

**What stays, because it was the good part.** The structural boundaries (canonical paths, project-bound databases, triggers), the measured/estimated/unknown distinction, the defensive refusals, Phase −1 before every dispatch, the targeted gate with the trailing sweep, a mutation on every decision an Amber or Red package makes, the refusal register, visible workers in worktrees, the co-edit protocol, and an independent verifier for Red. None of that produced the bloat; the bloat is physical layout, narrative comments, and ceremony applied to work that carried no decision.

**Why a ratchet rather than a ceiling.** Twelve files are over the line today and the point is to move them down one package at a time while other work continues. `scripts/check-file-sizes.py` counts production lines (everything before a file's inline `mod tests`, so a file cannot "shrink" by deleting tests), fails any file over 2,500 that is not in `scripts/file-size-baseline.txt` at a size it has not exceeded, and refuses the baseline growing. A decomposition package ends with `--update`, and the reviewer diffs the baseline. It runs at the end of `blast-radius.sh` — the gate every worker runs — and in `ci-local.sh`'s lint lane.

**Why moves before trims, and why 2,500.** A pure move is reviewable mechanically: the full targeted gate (for `config` and `main.rs` that is most of the crate), `git diff --color-moved=zebra` with a count of the lines that are not moves, and the ratchet. A comment trim is a content change reviewed by reading, and mixing the two hides the second inside the first. So each monolith gets a move package and then a trim package. 2,500 production lines is the size at which a reader can still hold a module's invariants in one sitting; it is a working number, not a law, and the ratchet makes lowering it later a one-line change.

**Why `main.rs` goes last among the splits.** It is the file every command lives in and the file every live package touches; splitting it while a package is mid-flight is a conflict for both. The three splits that touch no live worker — `config`, `routing/evidence`, `shell` — go first, `routing/session` next, and `main.rs → commands/` once the memory package in flight has integrated.

**Flakes.** The load-sensitive PTY families (`terminal_loss`, `session_supervision`, the pty fixtures — the Gatekeeper-scan mechanism the measurements record) have cost a two-run attribution ritual on every red. The gate now owes that rerun itself: a failing target in a known family is re-run alone once and reported as a flaky-pass, which is not red and gets no write-up; three flaky-passes in a week buy a determinism packet. `GH-GATE-RERUN-ALONE` is the package.

**Dogfooding.** Mutations prove a test watches a line; they do not prove the Claude/Codex/provider chain works end to end, and every real defect this project found was found by running the shipped binary in a real terminal. One real session per working day — the shipped binary driving a real harness on a real project for at least an hour, the orchestrator watching memory extraction, routing, the firewall and the shell — with findings in `docs/process/dogfooding.md` and packets by risk.

**By risk, not by count.** The register's refusals of 1534, 1535, 1545, 1129, 1044, 1294 and 1610 are superseded by the user's ruling that they matter: each gets a design note and a package sized by its mechanism, or a design note that says honestly why it cannot be built yet — but not a refusal by checkbox economics. Everything else open stays refused unless a producer lands.

**Successors, named:** `GH-DECOMP-CONFIG` (Green, Sonnet high: `config/mod.rs` by concern, tests out), `GH-DECOMP-ROUTING-EVIDENCE` (Green, Sonnet high: ledger / readers / joins, tests out), `GH-DECOMP-SHELL` (Green: state / view / screens, tests out), `GH-DECOMP-ROUTING-SESSION`, `GH-DECOMP-MAIN` (after the memory package integrates: `commands/<name>.rs` per subcommand), `GH-TRIM-<module>` per split module, `GH-GATE-RERUN-ALONE` (Green, scripts), and the first dogfooding session.

## Trims: `shell/mod.rs` — history moved out of comments by `GH-TRIM-SHELL`, 2026-09-03

Rule 3's "move history out, behind a one-line pointer" landed here for the handful of blocks whose reasoning was worth keeping but not worth the lines in production code. Each subsection is what the in-code comment now points to.

**The headless-viewport filter, in the `Event::Tick` arm.** `view::render_viewport` already refuses to draw a headless session's screen, so the filter on `state.viewport_grid()`'s producer looks redundant — a mutation deleting it was run rather than assumed, and it survived on the visible-drawing property but was caught by two others: `state.viewport_grid()` stops being an honest description of what is actually on screen the moment the filter is gone (a headless session's grid sits there stale-but-wrong instead of empty), and a headless session's own output would then make the grid differ every tick, repainting continuously for a session nobody is looking at. Both are real behaviour a screenshot cannot see.

**`resource_capacity_line`, line 1659 and line 1663, precisely.** Line 1659 collapses `TelemetryClass::Authoritative` and `Observed` to `"measured"` — the map line names four words, not the five `crate::provider::quota` itself tracks, and both source classes are real readings nobody inferred. `Estimated` and `Manual` keep their own words, and no reading at all is `"unknown"`, never a number. Separately, `RemainingCapacityScore::percent`'s `Exact`/`Estimated` split (used once a score exists) is deliberately not the same test as `state.telemetry_class()`, which answers the whole resource's *best* source across every pool and would print "measured" even when the one number actually shown here is an estimate — two different questions that happen to share vocabulary. Line 1663's reserve note is gated at `band <= CapacityBand::Reserve`, the exact boundary `evaluate_reserve_spend` itself gates real spend on; above it the reserve has influenced nothing this round, so nothing about it is shown.

**`forecast_note`'s wording, in full.** Map line 1283 is *"surface exhaustion forecasts as estimates rather than promises"*, and the function computes one division of a measured remaining count by a median of bucket counts — a figure with real error bars a reader will act on — so every sentence it can produce is hedged in the text itself rather than by a disclaimer somewhere else. "estimated to last about …", never "will last": `about` because the rate is a median over five-minute buckets, `estimated` because the inputs are the ledger's own history, not anything a provider promised. "may not reach its reset at the current rate", never "will run out" and never "guaranteed": `may` because the forecast holds only while the rate does, and `at the current rate` says exactly which assumption it rests on. Hours render to one decimal rather than a timestamp because a clock time reads as a commitment about a moment, and this is not one.

**`spawn_event_tail`, why the interface cannot simply subscribe to its own bus.** A lifecycle hook runs as its own short-lived process — the reason `glasshouse hook` exists at all — and its events are minted on *that* process's bus, then it exits; nothing on the shell's own bus ever sees them, so an interface that only subscribed to itself would show a session's own keystrokes and never once show it finishing a turn. The project's event log is the seam both processes write into, which is why this reads the log on its own thread instead.

## Coupling: the physical split is done, the interfaces are deliberately not — user ruling, 2026-09-03

*Recorded from the user's ruling at the close of Phase 59's decomposition
waves, in the user's own terms.*

**The state after the splits is good enough.** The acute damage is gone: no
10,000–16,000-line production modules, responsibilities are findable, a change
has a smaller review and test surface, and the size ratchet prevents relapse.

**What is left is not a defect.** Several of the new files are physically
separated but still talk to each other through many re-exports and
`pub(super)` accesses. The physical separation exists; the domain interfaces
are in places still broad. **This is an improvement, not a repair**, and it
does not get a second refactoring wave.

**How the decoupling happens instead — organically, from evidence:**

1. When two modules repeatedly have to be changed together, re-cut the shared
   responsibility.
2. When one module needs many internal details of another, introduce a small
   domain API — small, and for that need.
3. Remove a re-export only once real new code paths have shown which interface
   is durably needed.
4. **No abstract traits and no extra crates invented for the sake of "clean
   architecture."**
5. Every new function stays in the module that owns it. `main.rs` and each
   `mod.rs` are a dispatch and composition layer, nothing else.

**The distinction that matters:** not *decouple everything preventively now*,
but *accrue no new coupling debt from here on*. Rule 5 is the enforcing half
and applies to every package from today.

**Order of work, per the same ruling:** close Phase 59 cleanly — its remaining
open lines, `shell/mod.rs` included — then return to product work and
dogfooding. After a run of real changes, the shared diffs show which module
boundaries are actually wrong, and that is a far better basis for further
decoupling than a theoretical architecture exercise.

**How this gets revisited, and why not yet.** The signal the ruling names is
**co-change**: which files keep appearing in the same commit. It is
computable from `git log` in about forty lines (`scripts/co-change.py`, named
here as the successor, together with a `pub(super)`/re-export census per
module). It is deliberately **not written yet**: the last three sessions'
history is dominated by the decomposition itself, so a co-change measure taken
now would mostly report which files a *split* touched together — the exact
artefact the ruling says not to reason from. It becomes worth writing after a
stretch of ordinary product changes, and the trigger to write it is that
stretch existing, not a date.

## Steering decisions of record — the user, 2026-09-03, at `2d1dc4d`

Four decisions, given directly by the user and superseding any interpretation that
conflicts with them. They are decisions, not proposals: do not reopen them because
implementation has tradeoffs. Bring back only evidence of a material technical,
security or performance problem.

**Attribution first.** The *proportional assurance* guidance recorded earlier that day
came from a monitoring agent the user runs, not from the user. It is retained as an
**advisory recommendation**, not as an immutable product requirement, and it does not
outrank this ruling.

### 1. P1b — relay-path usage reading: **approved**

The gateway may inspect supported relayed response bodies far enough to extract
structured usage and timing. Accurate usage and evaluation data is preferred over the
previous byte-for-byte opacity. The constraints are part of the decision:

- Forwarded response bytes and protocol semantics are preserved.
- Bounded streaming or incremental parsing; never buffer a whole response merely for
  telemetry.
- Extract protocol metadata and usage fields only — not general response content for
  storage or analysis, and **no relayed response content is persisted** by this producer.
- CPU, memory, latency and throughput overhead on the hot path are benchmarked.
- An unsupported provider or response format records usage as **unknown**, never
  estimated.
- A material performance regression is reported with its evidence and proposed
  alternatives, never silently accepted.

**Provisional regression trigger (user, 2026-09-03):** 2% is an *engineering trigger*,
not a universal SLA. Measure **proxy-only overhead with a controlled local fixture**,
separately from provider and network latency, and report CPU, throughput, p50/p95
latency and bounded per-stream memory. Escalate only if a **repeatable** regression
above roughly 2%, or another material resource cost, survives reasonable optimisation.
Benchmark noise is not a product escalation.

Built as **one producer package followed by its consumers** — not as twenty
independent projects for the ~20 lines it releases (1333, 1263, 1158, Phase 32G, much
of Phase 51).

### 2. Linux keyring dependency: **approved**

Use a maintained Secret Service-compatible Linux keyring integration, preferably behind
a platform-neutral secret-store boundary. Do not implement several desktop keyring
protocols independently unless the ecosystem actually requires it. Where no supported
keyring service is available: give actionable installation or configuration
instructions; permit an unencrypted local fallback **only by explicit opt-in**, after
clearly explaining the security risk; restrict file permissions as far as the OS
permits; never make plaintext storage the automatic default; and never write secrets to
logs, tracked project files or diagnostic exports. Releases the remaining Phase 9E line.

### 3. Phases 52 and 53 — vector and graph retrieval: **deferred gates, not blockers**

Neither becomes a core or MVP dependency. FTS5 and SQLite remain the production
baseline.

*Semantic/vector retrieval* — a **bounded optional experiment is allowed**, because
usefulness cannot be established without an implementation to compare against. Define
the concrete lexical-retrieval failures and the evaluation metrics first; implement
behind an optional feature or addon boundary; A/B against the existing
lexical-plus-reranking path on real Glasshouse queries; do not make it the default path
or introduce a core schema dependency before results justify promotion; if retained it
augments lexical retrieval rather than replacing it.

*Graph storage* — no dedicated graph database yet. Use explicit typed SQLite
relationships first, and require concrete multi-hop queries SQLite cannot serve
adequately before authorising an experiment, isolated the same way.

Both phases' open lines are **deliberate experiment gates**: deferred, not release
blockers, and not part of the active execution queue. They are not to be ticked as
completed work.

### 4. A bounded file-coordination capability is promoted to MVP scope

Maybe A, B, C, F and H are promoted from *Maybe* into committed product scope as **one
vertical capability**, not five platforms — recorded as **Phase 60**. A and F give
soft, project-scoped, file-granular, turn-scoped claims with automatic release; B gives
the structured edit-intent observation the coordination layer needs; C consumes claims
and intents to identify likely conflicts; H reports them so the orchestrator can re-plan
only the conflicting part of otherwise parallel work.

The MVP proves **one end-to-end behaviour**: two active sessions express edit intent for
the same file; Glasshouse detects the direct overlap, explains it, notifies the
orchestrator, and lets the orchestrator re-plan or serialise only that conflicting work;
the claim is released when the relevant turn finishes.

Scoping rules, which are the decision as much as the capability is: soft coordination
first — no OS locks and no permission changes; file-level and turn-level granularity by
default; no repository-wide semantic analysis on every operation; direct same-file
overlap is the first high-confidence case and inferred adjacent-interface predictions
stay advisory; use structured hooks where available and be honest when a harness cannot
provide them, never treating terminal-output inference as equivalent to a structured
pre-edit hook; preserve an explicit user bypass; keep warnings and re-planning
inspectable. **F is the scoping rule that keeps A surgical, not a separate locking
subsystem. H is a bounded consumer of the conflict signal, not a general autonomous
planning platform.**

**Delivery path for H, settled by the user (2026-09-03) — do not design another
transport.** Glasshouse already has an orchestrator delivery path: the Phase 15 wake-up
flow, `SessionApi::send_text`, and `api/unix/events.rs`. Reuse it. The MVP targets the
active project session designated as orchestrator; where there is no unambiguous active
orchestrator, **surface that the conflict could not be delivered** rather than inventing
a worker-ownership or push subsystem. Come back only if repository evidence proves that
seam cannot carry the event safely.

Implementation order: **A+F → B → C → H.** Maybe D, E, G, I, J, K and L stay parked and
are **not** rejected; they are not promoted merely because these five were. Promotion
does not make every speculative sub-checkbox an MVP release blocker — Phase 60 carries
the smallest coherent slice and the remaining lines in those groups stay experimental
refinements.

### 5. The policy contradictions, resolved

The broad feature-freeze reading is replaced by: **do not add further large speculative
capabilities before a broader release; missing producers required by already committed
behaviour, and the approved A/B/C/F/H coordination slice, are permitted.** *"Do not add
capability-map lines"* applies to process machinery — validators, ledger fields,
checklist expansion — and does **not** prohibit recording product capabilities the user
has explicitly approved. Risk-based ordering still applies to executable work.
Deliberately deferred experiment gates are not in the active execution queue and are not
to be presented as blockers. **Open, deferred, experimental, refused and
awaiting-user-decision are distinct statuses and must stay distinct.**


## A task's progress is declared, never guessed — designing lines 1294 and 1610, 2026-09-03

The user named **1294** (*"avoid moving an almost-complete high-value task to
another session solely because a reserve threshold was crossed"*) and **1610**
(*"avoid migrating a nearly completed task solely to preserve a small amount of
quota"*) among the seven lines whose refusal by checkbox economics does not
hold, and said *design it*. This is that design note. It supersedes nothing in
*A task is never "nearly complete"* above — it **answers that section's own
closing sentence**, which says both lines re-open together the moment a real
producer of task progress lands.

**What the ruling did not lift.** The inference ban stands, and it is the
strongest argument in either section: turn counts and elapsed time report
"almost complete" for work that has merely been running a while, which is
precisely the long-running work a protected reserve exists to keep serving.
`task_nearly_complete` is the **first branch** `evaluate_reserve_spend` takes,
outranking every other signal including the user override, so a fabricated
value does not degrade the policy — it inverts it, at the one moment the
protection matters. Nothing about a user ruling makes a proxy honest.

**The state in current source** (re-derived 2026-09-03; the refusal register's
citations had gone stale on Phase 59's splits, which is what that file's own
"this file drifts" section warns about):

- the field is `ReserveDecisionInputs::task_nearly_complete`, in
  `provider/quota/mod.rs`, with the refusal at its own doc comment;
- there are **two** production construction sites, not one:
  `routing/pressure.rs::reserve_verdict` and
  `routing/disposable/mod.rs`'s per-candidate loop. Both pass `false`, each
  with a comment naming the line;
- the two consumers are `provider/quota/mod.rs::evaluate_reserve_spend` (1294)
  and `routing/pressure.rs::reserve_verdict` (1610) — **one mechanism seen from
  two phases**, which is why the two lines are one package and not two.

**The producer that is honest: a declaration.** The pattern is already at the
richer of the two construction sites, one field away.
`routing/disposable/mod.rs` passes `user_override: self.reserve_override.applies()`
— a real user declaration, scoped so it is true only for a session the user
actually named. That is not an inference about the work; it is a statement
somebody made on purpose. Task progress can arrive the same way, and only that
way: **the person or orchestrator doing the work says the task is nearly
complete**, exactly as Phase 60's file claims are declared by a CLI verb rather
than guessed from terminal output (see *Claims, turn-scoped* in
`evidence/phase-60.md`, and line 2404's insistence that intent detection stay
best-effort and say so rather than infer).

This satisfies both lines as written. Their operative word is **solely**: the
guard exists to stop a *threshold* from being the whole reason a task moves. A
declaration is a second reason, contributed by the only party that knows.

**Successor package** (named here so this note is not another investigation
that ends in a document — practice §83): *a scoped task-progress declaration*,
carrying a CLI verb and a session-scoped, expiring store row, wired into both
construction sites, closing **1294 and 1610 together**. Its Phase −1 is the
paragraph above: the field, both callers, the propagation path and both
consumers all exist in production today, and the declaration is the one
missing link. `GH-TASK-PROGRESS` is the packet.

Two corrections to this note, from probing the source one step further after
first writing it:

1. **It is Red, not Amber.** A session-scoped expiring row needs migration 28,
   and a migration is Red in the tier table regardless of how small the
   decision is.
2. **`ReserveOverride`'s channel is configuration, not a CLI verb** —
   `EffectiveConfig::reserve_override_sessions` → `config/routing_policy.rs`.
   Its *shape* is still the thing to copy (session-scoped, and a no-match by
   default so its arrival is a no-op for every existing caller), but its
   *source* is deliberately not: a settings value is sticky, and a sticky
   task-progress declaration re-creates the inversion by the slower route this
   note's last paragraph warns about. The right precedent for the source is the
   claims machinery that landed the same day — a CLI verb writing an expiring,
   session-scoped row (`session/store/claims.rs`).

**And the pin that guards this is narrower than the refusal it protects.**
`tests/subscription_pressure.rs::the_policy_does_not_invent_task_completion`
asserts `task_nearly_complete: false` appears **exactly once** and `true`
nowhere — but its `production_source()` is `routing/pressure.rs` alone, up to
that file's first `#[cfg(test)]`. The second construction site,
`routing/disposable/mod.rs`'s per-candidate loop, is **not scanned**, so it
could gain a `true` without the pin noticing — and it is the site that decides
per candidate. Whoever wires the producer extends the scan to both files in the
same change; that is not optional cleanup, it is the reason the pin exists.

**What this deliberately does not do.** It does not infer progress from any
signal Glasshouse already receives; `LifecycleEvent` stays binary and
retrospective, and two of its variants keep the doc comments saying they are
not statements about the session's work. It does not relax the test that pins
the literal — that test is what stops the field drifting open before a producer
exists, and relaxing it was never a way to close either line. And it does not
make the declaration sticky: a declaration that outlives the task it described
would re-create the inversion by a slower route, so it is scoped and expiring,
like a claim.

## pane, the first-party harness — the user, 2026-09-05, recorded as Phase 61

Approved as a first-party harness built **in its own lane**, beside the final
Glasshouse stretch rather than after it. Design of record: the session artifact
*The Glasshouse Native Harness*. The map block is Phase 61, appended at the end of
`capability-map.md` for the usual line-number reason; the hand-off that produced it
is `.agent-runtime/pane/phase-61-draft.md`.

**The one design decision that makes it a different harness, and the only one worth
defending.** A tool result never becomes text in the conversation: it becomes a named
object in a runtime the model addresses from code. The model receives a bounded
preview and the handle; the object stays where it is. A 48k-token grep costs the
model a preview line to know about and nothing to compute over, so tokens per turn
stay roughly flat as a task grows. Everything else about the harness may be ordinary.

**This is not "a better harness", and the claim must not drift into one.** Claude Code
and Codex are better resourced and tuned against their own models; a head-on
comparison is a bet lost slowly. `pane` arrives as *one row in the capability
registry* — a destination with an unusual cost profile the router picks when the
workload suits it and ignores when it does not. A router with two destinations that
behave identically has learned nothing; two that fail differently is evidence. The
success criterion is correspondingly narrow: **on at least one workload tier, measured
over completed tasks rather than turns, native beats the adapter path on tokens or
wall-clock without losing on outcome.** One tier is enough, and 61A exists to be able
to say so honestly.

**Protocol, not linkage — and that is what makes standalone free.** Neither side
depends on the other at compile time. Glasshouse reaches `pane` exactly as it reaches
Claude Code: a declared executable, declared args, a PTY. `pane` reaches Glasshouse as
any harness does: `ANTHROPIC_BASE_URL` at the gateway, an MCP endpoint, a hook command
— each optional, each degrading to a local default when nothing answers. "Standalone"
is just the mode where nobody answers on the socket. Every Glasshouse-provided
capability degrades to a **local** one, never to an error, and the harness never
reimplements a Glasshouse subsystem: a local memory that is a table of notes is fine,
a local memory with authority classes and decay is a second Phase 21.

**Why it is a second crate, and why that is one line of manifest.** The workspace has
no async runtime on purpose — `ureq` blocking with rustls, gzip dropped, so the
gateway can stream a response through byte-identically. Codex, whose sandbox and
code-mode crates are the borrow, is tokio top to bottom with an embedded V8. Inside
`crates/glasshouse` the second constraint would eat the first. As its own member both
hold: `cargo build -p glasshouse` stays async-free, and V8 compiles in its own unit.
**The `--exclude pane` on every `--workspace` invocation, and `default-members`, are
part of the decision, not housekeeping** — without them all twelve GitHub cells and
the local gate would compile V8 on every run.

**Build order, each phase shipping something usable on its own:** 61A the ruler (the
evaluation hooks, because the interesting claim is exactly the kind that feels true
and often isn't) · 61B the crate and the adapter · 61C the loop and the three seams ·
**61D the sandbox, before any model-authored code executes — this ordering is not
negotiable** · 61E code over live objects · 61F the supervisor.

**Attribution is part of the borrow.** Codex is Apache-2.0 into MIT-OR-Apache-2.0,
which works only in the Apache direction: keep the license headers, keep NOTICE, state
the provenance, and **vendor** the crates taken rather than depending on
`openai/codex` as a git dependency.

**Four questions the user still owes an answer to, and 61B must not be taken as
having settled them:** whether the README's single-binary promise scopes to
`glasshouse` alone or the harness embeds its host; TypeScript-or-nothing, since taking
Codex's V8 runtime is a permanent interface decision; whether
`translate/canonical.rs` round-trips reasoning blocks byte-identically, which must be
tested before 61C and not after; and the name, with the `Vendor` doc comment owing a
sentence on the case where the publisher is us.

**Its own lane in the SDLC.** A team lead owns `crates/pane/**` on `pane/integration`
and pays review out of its own context; the primary owns `main`, the records, and
everything outside `crates/pane/`. Contention lasts exactly one commit — the four
Glasshouse-side files (workspace manifest, `harness/mod.rs`, `ci-local.sh`, the GitHub
matrix) all change once, in the kickoff, and never again. Merges are per sub-phase,
seven across the whole build. Tiers: 61A Amber · 61B Green · 61C Amber · **61D Red**
(Opus plus an independent verifier) · **61E Red** · 61F Amber · 61G Amber (the batch
window and the interrupt class are decisions; the bus already exists). 61G — events
in batches, background work, messages — was recorded the same evening from the
ended session `glasshouse-9c`'s hand-off: the mechanism that makes pane usable in
the orchestrator role the two-modes table already lists, and the cure for the storm
of one-event-one-turn that the orchestrator running this build pays for today.

## Line 442's crate, ruled 2026-09-05: the one that can refuse a prompt before raising it

The 2026-09-03 answer named *the secret-service/zbus route*; the backend that landed uses `dbus-secret-service` 4.1.0 directly, and the ruling is that the measured property decides, not the name. The hazard this line was refused over for a month is a locked collection hanging a launch — `keyring` 3.6.3 leaves the prompt timeout unset and the calling thread waits up to a year. `dbus-secret-service` exposes `connect_with_max_prompt_timeout(_, 0)`, under which a prompt is refused before it is raised, and reads a collection's `Locked` without unlocking it; the pure-Rust `secret-service` crate, by the worker's reading of its source, has no timeout anywhere and unlocks unconditionally inside `delete` — an unbounded wait, worse than the year. The cost is `libdbus-sys` and therefore `libdbus-1-dev` + `pkg-config` on every Linux build host, which the user accepted the same day, and no executor enters the crate. The box ticks when a CI fixture proves the round trip against a live Secret Service; until then 442 is LOCALLY VERIFIED in `phase-9e.md`.

## Context size is read off the gateway's own exchange, never guessed — designing lines 1158 and 1534, 2026-09-05

Both lines were refused for one reason, stated in `phase-30.md` and repeated in
`phase-35b.md`: the only token counts in the schema are the gateway's, and the
gateway was forbidden to parse a response body, so no context size could be
known and a fabricated one *"would have been read as telemetry by every future
router"*. That reason ended on 2026-09-03 (`gateway/ingress.rs`, *a seventh
thing may now be recorded*): every relayed exchange now writes the provider's
own `input_tokens` and `cache_read_input_tokens` into `routing_observations`
with migration 24's `session_id`. 1535 and 1545 closed on that producer on
2026-09-05; 1158 and 1534 close on it here. **A refusal ends when its producer
lands; nothing in the earlier reasoning is retracted.**

**The estimate (1158).** A session's estimated context size is the prompt size
of its **latest** gateway exchange — the row with the greatest `observed_at`
(then `seq`) whose `session_id` is the session's and whose `input_tokens` is
known. The prompt the provider billed *is* the context the harness sent, so
this is a reading, not a model. It lives nowhere as a column: `SessionContext`
already records that copying a token count out of `routing_observations` is a
second source of truth (migration 15), so the value is computed from the rows
the destination builder already holds (`commands/routing_destinations.rs`
reads `consumption_in_window` for the burn readers) and attached to
`SessionContextFacts`, the same carrier as compactions. A session with no such
row — never relayed, or idle for longer than the window — reads `None`, and
`None` is the honest floor, never `0`.

**The wire rule, which is the decision 1158 makes.** The two counts do not
mean the same thing on every wire. Anthropic Messages bills `input_tokens`
*excluding* the tokens served from cache (`cache_read_input_tokens` is a
separate figure), so the prompt size is their **sum**. OpenAI's
`prompt_tokens` / `input_tokens` *include* `cached_tokens` (a subset detail),
so the prompt size is `input_tokens` **alone** and adding the cached figure
would double-count it. The row's `route` column carries the wire slug the
gateway chose from the request target (`with_route(exchange.protocol)`), so
the rule keys on it: `"anthropic-messages"` sums, every other known slug takes
`input_tokens` as is. Cache-*creation* tokens are not recorded and are not
estimated — the estimate is a floor on the turn that writes a cache, and the
doc says so.

**The term (1534).** `context quality` is a bounded negative contribution in
the session router's explanation, beside `measured cache temperature`:
`−CONTEXT_QUALITY_MAGNITUDE_CEILING × clamp((tokens − CONTEXT_LEAN_TOKENS) /
CONTEXT_BLOAT_SPAN_TOKENS, 0, 1)`. A lean session — under 32,000 tokens, the
size a working context normally sits at — contributes exactly `0.0` and says
*lean*; the penalty grows linearly and reaches the ceiling at 160,000 tokens,
where every shipped frontier window has either compacted or is about to. The
ceiling is **0.1**: equal to the measured cache temperature's and, like it,
strictly below `CACHE_LIKELY_LOST` (0.2), so a size reading never outweighs a
structural fact about the move in front of it, and it cannot outrank the
affinity facets that reward the same session for being *about* the task. A
destination with no estimate contributes `0.0` and says *unknown* — a ranking
on a build with no relayed exchanges is byte-for-byte what it was. Compactions
are **not** folded in: `discovery.rs`'s native-context facet already scores
them, and the same signal twice under a new name is the trap `phase-35b.md`
named when it kept 1534 open.

**Why "quality" is size and only size.** Line 1594 names the three ways a
session's context can be poor — *cold, bloated, or semantically poor*. Cold is
1535/1545's term; semantically poor is Phase 36's task-match and touched-files
facets; bloated was the one nobody could measure. Now it is the one this term
measures, and it claims nothing else.

**Limits and the successor.** The two constants stand in for a fact this build
does not hold — the destination model's context window. When a per-model window
reaches the catalogue (a Phase 32G-shaped producer, if ever), the term becomes a
fraction of *that* window and the constants go. Recorded here so nobody
re-derives it into a second scale. Package: `GH-CONTEXT-SIZE` (Sonnet, Amber —
two decisions, two mutations: the wire rule and the term's sign).

## Rollback preserves what is not yours, and Glasshouse names it rather than doing it — designing line 1044, 2026-09-05

Phase 21K's ruling stands: Glasshouse performs no rollback and no isolation of
code; it records the agent's choice (1041) as a transition. Line 1044 —
*preserve user changes and unrelated worker changes when rolling back or
isolating an invalidated experiment* — was refused on that ground, because the
preserving is the agent's or version control's act. The user's ruling of
2026-09-03 asks for a design instead of a refusal, and there is one that keeps
the standing rule intact: **the reason an agent reverts someone else's work is
that it cannot tell whose work it is, and that is a fact Glasshouse holds.**

**What Glasshouse knows at the moment of the choice.** Phase 60's
`file_claims` rows say which repo-relative paths every live session in the
project has declared it is changing (`SessionStore::active_claims`). The
working tree says which paths carry uncommitted changes (`git status
--porcelain`, through `checkpoint/git.rs`'s existing `git_output`, which
already answers *unknown* for a missing git or a non-repository). The
transition itself says which session is choosing. From those three, the
**preserve set** is computable and exact in one half and conservative in the
other:

- **`claimed_elsewhere`** — paths under an active claim held by a session
  other than the one transitioning. These are another worker's, by that
  worker's own declaration. Exact.
- **`unclaimed_changes`** — paths the working tree reports changed that the
  transitioning session never claimed. The user's edits are here, and so are
  an unclaiming worker's; Glasshouse cannot tell those two apart and does not
  try — both are *not the experiment's*, which is the only distinction the
  line needs. Conservative: a path the experiment changed without claiming it
  lands here too, and the agent is told to keep it, which is the safe error.

**The door carries it; the ledger does not.** The preserve set is a reading of
the tree at the instant of the transition, not a fact about the assumption, so
it rides the door's transition reply (`api/protocol.rs`, and the MCP tool's
result) when the transition is the agent's rollback or isolate choice or moves
the assumption to `refuted` — and is never written to `assumption_transitions`,
which stays append-only and about the assumption. The guidance page gains line
1044 beside 1041: *before reverting anything, exclude every path the reply
lists under preserve — another live session or the user owns it; Glasshouse
names them and reverts nothing.*

**What this does not claim.** A user edit to a path the experiment also
claimed is indistinguishable from the experiment's own and is not preserved by
name — the guidance says so and points the agent at its VCS for that case. No
path is ever reverted, stashed or restored by Glasshouse. A repository without
git, or a session outside one, yields an empty `unclaimed_changes` marked
*unknown*, never an empty list that reads as *nothing to preserve*. Package:
`GH-ROLLBACK-PRESERVE` (Sonnet, Amber — one decision, the membership of the
set; one mutation, the other-session filter).

## Protected quota's availability is recorded when a high-tier task is routed, and read back as a rate — designing line 1837, 2026-09-05

The register filed 1837 under RC-B (*no outcome is ever learned*) because its
verb is *measure … when needed*, and held it behind the product question the
user answered on 2026-09-03 (*explicit rating when given, the turn-outcome
proxy otherwise*). Read again, the line needs no outcome at all: whether
protected quota *remained available* for a high-tier task is decided at the
moment the task is routed, from two facts the router already holds and nobody
writes down together — the task's workload tier (`routed_tier`, the same value
`RoutingTierObserved` records) and the chosen destination's capacity band
(`Destination::capacity_facts().band`, computed by `routing_destinations` under
the resource's own reserve thresholds, line 1287). That is RC-A's shape —
*decided in production, announced to the user, dropped* — the cheap cluster
the register says to check first.

**The row.** `EvaluationKind::ReserveAvailabilityObserved`, written at the two
routed exits of `launch` beside `record_routed_session`, **only** when the
tier is above `ROUTINE_SUPPORT_CEILING` (`Heavy`, `Frontier` — the tiers the
reserve exists to protect; a `Standard` launch writes nothing, because
*needed* is the line's own word). `subject` is the band the router read, in
`CapacityBand`'s own spelling, or `unknown` when the destination carried no
reading; `detail` is the tier word; `session_id` the launched session's. No
migration: the evaluation ledger is a kind/subject/detail row store and the
vocabulary pin in `database::EVALUATION_KINDS` is the only schema-side change,
as it was for 1855.

**The reading.** `EvaluationObservations::counts_by_subject` already exists;
`route_outcomes_section` prints one line — *protected quota for high-tier
tasks (1837): available N · at reserve R · exhausted E · unknown U of K* —
where *available* is every band above `Reserve`. Below `MIN_SAMPLE_FOR_SUMMARY`
it says *not enough high-tier launches*. Package: `GH-RESERVE-AVAILABILITY`
(Sonnet, Amber — one decision, which launches count; one mutation, the tier
filter). 1846 is a different mechanism (the prior's predictiveness against an
outcome) and gets its own note with the explicit-rating door.

## The routing half of RC-B: an explicit route rating when given, the turn-outcome proxy otherwise — designing `GH-ROUTING-RATING` and line 1846, 2026-09-05

The user's answer of 2026-09-03 (*"Yes, both: explicit rating when given, the
turn-outcome proxy otherwise"*) has been applied to the memory half
(`MemoryRated`, `glasshouse memory rate`, the readers that label the other
half *proxy*) and not yet to routing, where 1834, 1835 and 1852 closed on the
proxy alone. This note is the routing half, in the memory half's own shape so
the two never diverge.

**The rating.** `EvaluationKind::RoutingRated`: `subject` is the destination
id the session was routed to (the same word `RoutingCostClassObserved`'s
`detail` carries), `outcome` is **`useful` or `not-useful`** — the memory
rating's own words, reused on purpose: the judgment is *did this decision
serve the task*, and two scales for one question is the mistake the reserve
inputs already refused — `session_id` is required (a route rating is about a
session's route, never a memory), `detail` is the operator's note, never
parsed. **A rating is a new row, never an edit**, and never a rewrite of
`RoutingOutcomeObserved`.

**The door.** `glasshouse rate-route <session-id> useful|not-useful [--note]`
— a top-level command, because `route` is a flat command whose flags are the
ranking's own and turning it into a group would break `route --moment …`;
modelled line for line on `memory rate` (`cli.rs::Rate`,
`record_memory_rating`'s handle discipline). CLI only, as memory's is: the
rating is the operator's act; a harness may issue it as a tool call the way
`memory rate`'s doc already says. It refuses a session id that has no
`RoutingCostClassObserved` row — one cannot rate a route that was never
taken — and prints the row it wrote.

**Precedence, and the one rule readers keep.** Where a reader counts a
session's route as a success or a failure from `RoutingOutcomeObserved` (the
proxy), a `RoutingRated` row for the same session **replaces** the proxy's
verdict for that session, and the readout says so: every success count
becomes *rated N / proxy M*, printed apart, never summed into one number. The
memory readers hold exactly this rule; `route_outcomes_by` and the pairing
block gain the split. A session with two ratings takes the latest — a rating
may be revised by rating again, which is the append-only way to change one's
mind.

**Line 1846 — the prior's predictiveness against the outcome.** *"Measure how
quickly local pairing evidence becomes more predictive than the initial
same-vendor prior."* For every routed session with an outcome (rated first,
proxy otherwise), take the pairing class the launch recorded
(`sessions.pairing_class`, the join `route_outcomes_by_pairing_class` already
makes) and *k*, the number of that pairing's outcome rows that preceded it.
Two predictions are scored against the outcome: the **prior's** — a native
pairing predicts success, a cross-vendor one predicts failure, which is
exactly what `pairing_prior` contributes when *k* is below the evidence
threshold — and the **local evidence's** — the pairing's success rate over
those *k* rows, predicting success at or above one half. Bucket by *k* (0–4,
5–9, 10–19, 20 and more), report both accuracies per bucket with the sample,
and name the first bucket in which local evidence is at least as accurate as
the prior over `MIN_SAMPLE_FOR_SUMMARY` sessions — *how quickly* is that
bucket, or *not yet* when none qualifies. Printed under 1846 in the route
outcomes section; *measures*, never re-tunes the prior.

**Packages, in order, because they share `kinds.rs`, `schema.rs` and
`route.rs`:** `GH-ROUTING-RATING` (Sonnet, Amber — the kind, the door, the
rated/proxy split in the two readers; closes no line, it is the explicit
half's producer), then `GH-PAIRING-CROSSOVER` (Sonnet, Amber — the 1846
reader over rated-or-proxy outcomes). Both wait for `GH-RESERVE-AVAILABILITY`
to land, for the same three files.

## Trims: `memory/search.rs` — history moved out of comments by `GH-TRIM-MEMORY-SEARCH`, 2026-09-05

Rule 3's "move history out, behind a one-line pointer" landed here for the ten blocks over 20 lines in `crates/glasshouse/src/memory/search.rs` (Phase 59, line 2053). Each subsection is the full original comment; the in-code pointer next to the trimmed version names the item below it.

### module doc

```text
//! Free-text search over project memory (Phase 23).
//!
//! Declared ahead of its implementation so that the module owning it never has
//! to edit `memory/mod.rs`, which another worker holds.
//!
//! # Free-form text is not FTS5 syntax
//!
//! FTS5's query language treats `"`, `*`, `:`, `^`, `-`, `(`, `)`, `NEAR`,
//! `AND`, `OR` and `NOT` as operators. A user is typing a question, not a
//! query language, so `sanitize_query` tokenizes on anything that is not a
//! letter or digit and wraps every token in double quotes — a quoted phrase
//! is FTS5's escape hatch for "treat this text literally" — doubling any
//! embedded `"` the way SQL string literals do. The result is passed to
//! `MATCH` as a bound parameter, never interpolated: the only SQL this module
//! ever builds from something other than a fixed literal is a column list it
//! wrote itself.
//!
//! # What the index covers
//!
//! `memories_fts` indexes `subject`, `body` and — from migration 6 —
//! `rationale`. The rationale is searchable because until that migration it
//! *was* the body: the extractor folded it in behind a marker precisely so a
//! search for the reason would find the decision. The eight other Phase 21B
//! provenance columns are deliberately not indexed; they describe a decision
//! somebody has already found rather than supplying the words they would
//! look for, and every indexed column shifts BM25's weighting of the ones
//! that matter.
//!
//! # BM25 direction
//!
//! SQLite's `bm25()` returns a *more negative* number for a *better* match.
//! `ORDER BY bm25(memories_fts) ASC` therefore puts the best match first —
//! this is asserted directly in the integration tests rather than trusted by
//! reading the manual once.
```

### `RetrievalIntent` doc

```text
/// What a file-path retrieval is *for* — map line 1141.
///
/// The map asks Glasshouse to *"prefer constraints, decisions, and failed
/// approaches when retrieving memory for an intended code edit"*, and the
/// operative words are **for an intended code edit**: the same file, asked
/// about for two different reasons, should not come back in two different
/// orders unless the caller said which reason it had. So this is an argument
/// to [`MemoryStore::for_path`] rather than a mode the store guesses from
/// context.
///
/// # Where the preference is allowed to act, and where it is not
///
/// Inside a [`LadderRung`] and nowhere else. Phase 21E's rule — an idea never
/// outranks an invariant, however well it matched — is the rung ordering, and
/// it stays the primary key under both intents. A `CodeEdit` retrieval
/// reorders *within* a rung and cannot promote anything across one; a
/// constraint-shaped memory that is not current is still below a current
/// decision afterwards.
///
/// # A fixed kind class, never a number
///
/// The preference is expressed as [`MemoryKind`] membership, not as a weight
/// added to `retrieval_weight`. A number would have to be calibrated against
/// BM25 relevance and against decay — neither of which is on a scale anything
/// here can compare a kind to — and would silently change how far the
/// preference reaches as either of those moved.
```

### `RetrievalResult::relevance`

```text
/// What `id` scored on the query that produced this result, or `None` if
/// `id` was not one of the memories it returned.
///
/// `None` is a real answer and the only honest one for a memory this
/// retrieval never saw: there is no relevance to report, and a zero would
/// be a fabrication that reads as "matched as badly as possible" rather
/// than "was not asked about". A search that matched nothing therefore
/// answers `None` to every question, rather than `Some(0.0)` to some of
/// them.
///
/// It is also the answer for **every** memory in a result
/// [`MemoryStore::for_path`] produced, and for the same reason one step
/// further out: that door retrieves by an exact file-path match and asks
/// no question, so none of the memories it returns was scored by
/// anything. "Was not asked about" is exactly what happened to them.
///
/// # This is a relevance, and it is deliberately not a confidence
///
/// SQLite's `bm25()` scores how well one memory's indexed text matched
/// one query against **this project's own corpus statistics** — term
/// frequency, document length, and how many other memories in this table
/// contain the same terms. More negative is a better match (see the
/// module documentation), so the scale is unbounded below and has no
/// natural zero.
///
/// Three consequences, and each one is a reason not to threshold it:
///
/// - **It is not calibrated.** The same number means different things for
///   two different queries, and for the same query against two different
///   projects. There is no constant of which *"below this, the retrieval
///   was poor"* is a true statement, so a threshold would be a number
///   somebody picked rather than a fact about the retrieval.
/// - **It is not the order the results came back in.**
///   [`MemoryStore::search`] ranks by [`LadderRung`] first, breaks ties
///   *within* one rung by this number multiplied by a decay weight, and
///   then `demote_thin_decisions` permutes again. Reading it as "why this
///   memory came first" is wrong across rungs.
/// - **It measures the match, not the memory.** Whether a memory is worth
///   putting into a session's context is a question about the memory's
///   authority, currency and scope. None of those is in here.
///
/// So map line 1129 — *"avoid injecting memory when retrieval confidence
/// is low"* — is **not** satisfied by comparing this against a constant,
/// and [`super::inject::briefing`] still refuses it. That function's
/// documentation carries the three objections that survive this method
/// existing.
///
/// # Why the raw match and not the blended ranking score
///
/// [`MemoryStore::search`] also computes `relevance × retrieval_weight` —
/// the number it actually sorts on inside a rung. That one is not offered
/// here, and the difference is the whole reason this method is worth
/// having: `super::policy::retrieval_weight` reads a memory's authority,
/// age, validation state and project phase and **never sees the query
/// text**. Blending it in yields a number that is high for an ancient
/// invariant no matter what was asked — exactly the query-blind signal
/// `inject.rs` refuses to build a gate from. It is also wall-clock
/// dependent, so the same store and the same query yield a different
/// value tomorrow.
///
/// The raw match is the one quantity in this module that varies with the
/// query and with nothing else. Anything inside this module that
/// genuinely wants the blend can compute it: the record carries its own
/// authority, timestamps and phase, and `retrieval_weight` is the same
/// function [`MemoryStore::search`] calls.
```

### `Scored` doc

```text
/// One retrieval hit and the BM25 relevance the query gave it, kept together
/// from the moment the row is decoded until the moment the two groups of
/// [`RetrievalResult`] are built.
///
/// A pair rather than a field on [`MemoryRecord`], because a relevance is a
/// property of *this retrieval* and not of the memory: the same record scores
/// differently for a different query, and a record read by
/// [`MemoryStore::get`] has no relevance at all. Putting it on the record
/// would make that absence unrepresentable except as a lie.
///
/// # `None` is *"was not asked about"*, and it is why this is an `Option`
///
/// [`MemoryStore::for_path`] retrieves by an exact `memory_files.path` match.
/// **It runs no query, so there is no relevance for it to supply** — and the
/// alternative was to hand `group` a `0.0`, which would put a manufactured
/// number into [`RetrievalResult`]'s private relevance map for a memory no
/// query ever matched. That is precisely what the map is private to prevent,
/// and [`RetrievalResult::relevance`] already says a zero there *"would be a
/// fabrication that reads as 'matched as badly as possible' rather than 'was
/// not asked about'"*.
///
/// Making the absence representable **strengthens** that invariant rather
/// than piercing it: the map still holds only relevances an actual query
/// produced, because `group` inserts nothing for a `None`, and the third door
/// still gets the one grouping and the one ranking the other two get.
```

### `rank` doc

```text
/// The one ordering in this crate, applied by every door before
/// [`demote_thin_decisions`] permutes within it.
///
/// # Phase 21E: the ladder rung is the primary key
///
/// See [`ladder_rung`]'s own documentation for why an idea must never
/// outrank an invariant regardless of how well it matched. Only within the
/// same rung does the weight below decide the order.
///
/// # Within a rung, and why the query-less door is not a second ranking
///
/// A queried hit is ordered by `relevance × retrieval_weight`, ascending.
/// SQLite's `bm25()` is *more negative* for a better match (see the module
/// documentation) and [`retrieval_weight`] is strictly positive, so ascending
/// puts the best-matching, highest-weighted memory first — exactly the
/// comparison [`MemoryStore::search`] has always made.
///
/// A hit with no relevance ([`MemoryStore::for_path`]) is ordered by
/// `retrieval_weight` alone, descending, which is the **same** comparison
/// with the one factor it does not have left out rather than replaced.
/// Substituting a number for the missing factor is what this whole change
/// exists to avoid: a `0.0` would collapse every product to zero and order
/// the results by nothing at all, while still looking like a ranking.
/// `retrieval_weight` never sees the query text — that is stated at
/// [`RetrievalResult::relevance`] as the reason the blend is not offered to
/// callers — so it remains an honest key when there is no query.
///
/// The mixed case cannot arise: a retrieval either ran a `MATCH` or did not,
/// and both doors build every one of their hits the same way. [`Ordering::Equal`]
/// is the answer that adds no claim if it ever does.
///
/// # Map line 1141, and where it sits in the comparison
///
/// `intent` inserts **one** key, between the rung and the weight: under
/// [`RetrievalIntent::CodeEdit`], a constraint, decision or failed attempt
/// sorts ahead of a feature, finding or todo *in the same rung*. Above the
/// weight so the preference is not something a large enough
/// `retrieval_weight` can outvote — a kind preference that a number can
/// overturn is not a preference — and below the rung so Phase 21E's rule
/// still decides first. Under [`RetrievalIntent::Lookup`] the key is constant
/// across every hit, so the comparison is byte-for-byte the one this function
/// made before the argument existed.
```

### `injection_query` doc

```text
/// Turn a routed task into the FTS5 `MATCH` expression **context injection**
/// uses, or `None` if nothing in it could be searched for.
///
/// The shape is
///
/// ```text
/// ("a" "b" "c") OR ({subject} : ("a" OR "b" OR "c"))
/// ```
///
/// — today's conjunctive query, unchanged, `OR`ed with a disjunctive one
/// restricted to the `subject` column.
///
/// # The left half: nothing that is retrieved today stops being retrieved
///
/// `sanitize_query` joins its quoted tokens with spaces, which FTS5 reads as
/// implicit `AND`: every word must appear in the same memory. That is right
/// for a search box, where a person adds a word to narrow the result set, and
/// it is wrong for a routed task, which is prose. *"Please look at the kestrel
/// export and make sure it cannot write a partial file"* demands that one
/// memory contain `please` and `look` and `sure` and `up`, so injection
/// retrieved **nothing** for any task written as a sentence — the limit Phase
/// 27 closed line 1126 with, named rather than hidden.
///
/// That expression is kept verbatim as the left disjunct, so the result set
/// here is a **superset** of the one the search box gets, by construction
/// rather than by test: whatever a keyword-shaped task retrieves today it
/// still retrieves. This step only ever adds recall.
///
/// # The right half is line 930, and it is in the query rather than after it
///
/// *"Inject only memories whose scope overlaps the current task."* Joining
/// prose with a bare `OR` makes membership almost free — one incidental word
/// and a memory is a candidate — and `MemoryStore::search` ranks by
/// [`LadderRung`] **before** relevance, so the top of a wide candidate set is
/// this project's highest-authority memories whatever the task was about.
/// Measured on a fifteen-memory corpus: a bare `OR` answered *"update the
/// README with the new installation instructions"* with three binding
/// invariants about pseudo-terminals, secrets and project isolation, matched
/// on the word `the` alone.
///
/// So the added disjunct is restricted to the `subject` column — the field
/// where a memory records what it is *about*, and the field
/// [`contradicts`] already treats as a memory's identity when deciding that
/// two memories concern the same thing. A memory joins the candidate set on
/// prose only if the task names its subject.
///
/// **Why this is not a relevance threshold wearing a different name.** It
/// reads no score, sorts nothing, and cannot be satisfied by matching the
/// same word harder; a memory whose body mentions the task's words a hundred
/// times is still out if its subject is about something else. More to the
/// point, a relevance threshold would not have worked: in the measurement
/// above the noise was selected by *rung*, not by score, so no cut on `bm25()`
/// could have removed it, and a stop-word or corpus-frequency filter could
/// not either — for the task *"make sure it is up to date"* no term matched
/// more than 47% of that corpus and every one of the three injected memories
/// was still irrelevant.
///
/// A memory that records **no** subject cannot be judged this way and is not
/// judged: it matches only through the left disjunct, which is exactly the
/// behaviour it has today. That is the direction this project's requirement
/// points — injection is strictly more recall, never less — and it is a real
/// limit, recorded in `phase-27.md` rather than papered over.
///
/// # This is a second expression, not a second retrieval
///
/// Phase 27 refused line 1129 partly because a second BM25 query issued from
/// `inject.rs` *"would be a second retrieval implementation ranking
/// differently from the one that chose the memories it was scoring."* That
/// objection is about **ranking**, and nothing here ranks: this function
/// returns a `MATCH` expression and `MemoryStore::search_matching` — the same
/// table, the same `bm25()`, the same ladder, the same decay weighting, the
/// same thin-decision demotion — does the rest for both doors.
///
/// # The quoting is inherited, not re-implemented
///
/// Every token is built by `sanitize_query` itself and only the join is
/// changed. A token is alphanumeric-only by construction there, so no quoted
/// token can contain a space and splitting that output on spaces recovers
/// exactly the tokens it produced. A task containing `OR`, `NEAR`, `*`, `"`
/// or `-` is therefore quoted here by the same code that quotes it for the
/// search box, and the containment property has one home rather than two.
```

### `MemoryStore::search` doc

```text
/// Search this project's memory by free text, ranked by BM25 relevance.
///
/// `text` is never interpreted as FTS5 syntax — see the module
/// documentation and `sanitize_query` — so a user typing `what does
/// "foo" do?`, a bare `AND`, or `a: b` gets a search rather than a
/// `SqliteFailure`. A query that sanitizes to nothing returns an empty
/// result rather than an error.
///
/// `scope` decides whether history is visible at all; see
/// [`SearchScope`]. `limit` bounds how many results come back — there is
/// no way to ask this method for the whole table.
///
/// Every result already carries its own provenance
/// ([`MemoryRecord::source_session_id`], [`MemoryRecord::source_commit`])
/// as `Option`, so a memory recorded without one reports it absent
/// instead of inventing an empty string.
///
/// # Phase 21E: the ladder ranks before the weight does
///
/// Every candidate is first placed on a [`LadderRung`] ([`ladder_rung`]),
/// and results are ordered by rung before anything else — a validated
/// current constraint outranks an older ordinary decision, and a
/// binding invariant outranks everything, regardless of how well any of
/// them matched the query text. The weight described below is only ever
/// a tie-breaker *within* one rung; it never lets a memory cross into a
/// rung its own authority and currency do not earn it.
///
/// # Phase 21D: decay is applied here, after the match
///
/// The raw BM25 relevance of every candidate is multiplied by
/// `retrieval_weight` before the final ordering, so an old, low-
/// authority memory that happens to match the query text well still
/// ranks below a fresh, high-authority memory that matches it poorly —
/// line 904's *"avoid resurfacing low-authority stale memories merely
/// because of high lexical similarity."* This has to run in Rust rather
/// than in the `ORDER BY`: the weight depends on the wall clock and on a
/// per-authority policy (`super::policy::retrieval_weight`), neither of
/// which SQLite's `bm25()` has access to. See `overfetch_limit` for why
/// the SQL `LIMIT` is not simply `limit`.
///
/// # Phase 22 line 1063: conflicts are detected here too
///
/// Before decay runs, every pair of still-[`MemoryStatus::Active`]
/// candidates in *this* result set is checked for contradiction — see
/// `contradicts` — and a contradicting pair is moved to
/// [`MemoryStatus::Conflicted`] via [`MemoryStore::mark_conflicted`]
/// before being returned, so a caller never receives two mutually
/// contradictory memories presented as equally settled. Detection is
/// scoped to the memories this query actually matched, not the whole
/// project: Phase 22 asks that a conflict be flagged, not that every
/// memory be compared against every other one on every search.
///
/// # The relevance is no longer thrown away
///
/// This method still returns bare records, because a caller that wanted a
/// list of memories before wants one now. The BM25 relevance every hit
/// earned survives the call on the other door:
/// [`MemoryStore::search_grouped`] returns a [`RetrievalResult`], and
/// [`RetrievalResult::relevance`] reads it back by
/// [`super::store::MemoryId`]. **Read that method before using the
/// number** — it is a within-query match score, not a confidence, and it
/// must not be thresholded.
```

### `MemoryStore::for_path` doc

```text
/// Every memory this project learned while `path` was being worked on —
/// the read door onto migration 17's `memory_files` rows, grouped the
/// same way [`MemoryStore::search_grouped`] groups a query's answer.
///
/// `path` is repo-relative and `/`-separated; it is put through
/// [`super::store::normalize_observed_path`] — **the same function
/// [`MemoryStore::record_observed_files`] put the column through** — so a
/// caller may spell it `./src//a.rs` or `src\a.rs` and still match the
/// row the writer stored. A path that function refuses is a path no row
/// can hold, and the answer is an empty result rather than an error:
/// nothing was observed against a file that cannot be named here.
///
/// # There is no relevance here, and none is invented
///
/// This runs no `MATCH`, so [`RetrievalResult::relevance`] answers `None`
/// for every memory it returns, and that is the true answer — the memory
/// was not asked about. See `Scored` for why the alternative, a `0.0`,
/// would have been a fabricated number in the one map this module keeps
/// private to stop exactly that.
///
/// # Ordering is `rank`'s, not this function's
///
/// The hits go through the same `rank` and the same
/// `demote_thin_decisions` the other two doors go through, so a memory
/// cannot rank one way when a query found it and another way when a path
/// did. Within a rung the ordering falls back to `retrieval_weight`
/// alone, which is the query-blind half of the comparison a search makes
/// — see `rank`.
///
/// # What this door deliberately does not do
///
/// It does not flag contradictions. That is a **write**
/// ([`MemoryStore::mark_conflicted`]), and Phase 22 line 1063 scopes
/// detection to *"the memories this query actually matched"* — a path
/// lookup matched no query, and a read door that mutates the table on
/// behalf of a caller that only asked what a file is associated with is
/// a larger claim than this package makes. A consumer that needs
/// conflict flagging should say so, and the argument belongs where that
/// consumer is built.
///
/// It also does not **narrow** by [`super::store::FileAssociation`], and
/// that is now a choice rather than the absence of one. There are two
/// associations to narrow by since migration 26 — `observed` and
/// `referenced` — and a door that returned only the stronger would hide
/// every memory learned beside a file from a caller that asked what the
/// file is associated with. So both come back, and each row **reports**
/// which it is through [`RetrievalResult::association`], which is the
/// answer a caller can act on without this function deciding for it. A
/// consumer that genuinely wants only referenced rows filters on that;
/// none does today.
///
/// # `intent`, map line 1141
///
/// [`RetrievalIntent::Lookup`] is this function's original order, byte
/// for byte. [`RetrievalIntent::CodeEdit`] prefers constraints, decisions
/// and failed attempts *within* each ladder rung — see
/// [`RetrievalIntent`] for why the rung stays primary and why the
/// preference is a kind class rather than a weight.
```

### the `for_path` SQL comment

```text
// `DISTINCT` because `memory_files` carries no uniqueness constraint
// — migration 17 argued one would be an index on speculation — so a
// memory associated with the same path twice must still be returned
// once. Both `project_id` predicates are deliberate: the association
// row's scoping is what the triggers maintain, and the memory row's
// is what a row that reached the file by some other route would have
// to defeat as well.
//
// The SQL `ORDER BY` decides only which candidates survive the
// overfetch, never the order returned: `rank` runs in Rust for the
// same reason `MemoryStore::search` needs it to — `retrieval_weight`
// reads the wall clock and a per-authority policy, neither of which
// SQLite has. Newest memory first is the honest candidate rule when
// there is no relevance to rank candidates by.
// `GROUP BY memories.id` rather than migration 17's original
// `DISTINCT`, and the reason is the second provenance value. A memory
// may now hold both an `observed` and a `referenced` row for one path,
// and `DISTINCT` over a column set that includes the provenance would
// return that memory **twice** — once under each word — spending the
// caller's `LIMIT` on one memory and leaving the fold below to decide
// which duplicate to keep. Grouping returns it once; every selected
// `memories` column is functionally dependent on the grouped primary
// key, which is exactly the case SQLite defines bare columns for.
//
// `group_concat(DISTINCT ...)` rather than an aggregate that knows the
// vocabulary (`MAX(provenance = 'referenced')`, say): the column
// deliberately carries no `CHECK` — migration 17's own argument — so
// which words exist is Rust's to say, through
// `FileAssociation::from_stored`. A word this build does not know is
// dropped there rather than compared here.
```

### `demote_thin_decisions` doc

```text
/// Phase 21B: *"treat a decision with missing rationale and missing
/// assumptions as lower-confidence than a well-proven decision of the same
/// authority class"*.
///
/// # Why this is a permutation and not an `ORDER BY`
///
/// The obvious implementation — sorting thin decisions to the bottom of the
/// whole result set — reads the line as *"lower-confidence than
/// everything"*, which is not what it says and would be a real search
/// regression: a perfectly relevant decision would fall behind a
/// barely-relevant memory of some unrelated kind. The line has two
/// qualifiers and both are load-bearing. It compares a decision against **a
/// decision**, and against one **of the same authority class**.
///
/// So the relevance order BM25 produced is left almost entirely alone: every
/// record that is not a [`MemoryKind::Decision`] keeps its position exactly,
/// and so does every authority class as a whole. The only thing that moves
/// is the order of the decisions *within* one authority class, where a
/// decision that recorded neither why it was made nor what it assumed is put
/// behind one that did.
///
/// A search returning one decision is therefore unchanged, and so is a
/// search returning a decision and a finding. A search returning two
/// `decision`-class decisions puts the better-proven one first however the
/// text happened to match.
///
/// Unclassified memories (`authority IS NULL`) form their own group, because
/// `None` is a distinct fact from every class and not a class to merge into.
///
/// The sort is stable, so two decisions that are both thin, or both
/// well-proven, keep their BM25 order relative to each other.
///
/// Operates on [`Scored`] rather than bare records so that a memory keeps the
/// relevance it earned when this permutation moves it. The permutation reads
/// only `authority`, `kind` and [`MemoryRecord::is_lower_confidence_decision`]
/// — never the relevance — so attaching the score changed no ordering.
```

## Trims: `tui/event.rs` — history moved out of comments by `GH-TRIM-TUI-EVENT`, 2026-09-05

Rule 3's "move history out, behind a one-line pointer" landed here for the nine comment blocks in `crates/glasshouse/src/tui/event.rs` that were over 20 lines. Each subsection is what the in-code comment now points to.

### field `quiet_ticks`

The short cut in `EventSource::next` is taken only once this passes `QUIET_TICKS`, and **that threshold is the whole reason the short cut is safe.** Crossterm multiplexes the terminal and `SIGWINCH` through one edge-triggered `mio` registration, and its reader returns the first of the two it looks at — dropping, unread, whatever readiness arrived in the same batch (see `Watch`). Polling it less often leaves a `SIGWINCH` sitting in its pipe for longer, and a `SIGWINCH` sitting in its pipe is what a keystroke collides with: skipping it on every idle tick turned `pty_smoke::resizing_the_shell_reaches_the_harness_terminal` from 0 failures in 12 into 1 to 2, every one of them a shell whose keystrokes had been swallowed.

Waiting for a second of complete silence first buys the protection where it is needed and gives up nothing where it is not: a terminal that has not made a sound for a second has no input to collide with, and the field processes had been silent for nineteen hours.

**This threshold is no longer the only thing holding that collision off, and the two do not fight.** `EventSource::next` now drains crossterm's pipe as soon as a signal interrupts a wait, whatever this counter says — the `after_signal` override in the idle arm is there precisely so a long silence cannot keep a `SIGWINCH` held. The counter still does its own job, which is not this one: it is what keeps an idle process out of `crossterm::event::poll`, where a hangup wedges it.

### field `crossterm_may_hold_more`

**This is what stops typing being throttled to one key per tick.** Crossterm does not read one byte at a time: it drains whatever the descriptor had into a parse buffer of its own and hands back one event per call. So after the first key of a burst is delivered, the rest of the burst is *inside the library* and the descriptor is **empty** — and `wait_for_terminal`'s `poll(2)` is level-triggered, so it correctly reports nothing and sleeps out the entire remaining tick before the loop asks crossterm for the key it has been holding all along.

Measured on this tree rather than argued, with a probe logging `FIONREAD` on the descriptor beside `event::poll(Duration::ZERO)` on every pass of the wait loop: through a twenty-key burst, **every** sample read `fionread=0`, and nineteen consecutive samples had crossterm answering that an event was ready on a descriptor the kernel called empty. One key per 16ms tick, which is what the shipped binary delivered: **16.8ms per key, a 200-character paste in 3.38s**.

So while this is set, `EventSource::next` asks crossterm *before* waiting instead of after, and the burst comes out at the speed of the loop. It is set by `EventSource::take_from_crossterm` whenever a read succeeds, and cleared the first time that early ask says no — one extra `event::poll` per burst, and none at all on a terminal nobody is typing at.

**It is not an optimisation of the idle path and must not become one.** `quiet_ticks` exists to keep an *idle* process out of `crossterm::event::poll`, where a hangup wedges it; this flag is false on every one of those ticks, so the two never overlap.

### `Wait::Idle` arm (residual-spin fix)

**This arm is the residual-spin fix**, and what it fixes is a rate rather than a bug. Every call into crossterm is a chance for the terminal to have died since the wait above, and an idle interface used to make one of those calls per tick to be told nothing. Measured over two eight-second profiles of an idle process: 268 of 6210 and 233 of 6162 main-thread samples — about 4% of every tick — were inside that pointless call, and 0 of 6185 are after this arm. That share is the window, and it is not the microseconds `wait_for_terminal`'s comment used to claim.

A terminal that has been silent for a while, with no window resize to report, has nothing crossterm could say. Not asking is the whole fix: see `quiet_ticks` for why it waits out that silence first, and `last_size` for the one thing that still has to get through.

Crossterm cannot be left holding an event of its own by the time this bites, either. It hands back one event per call out of a whole parsed buffer, but it is asked on every one of the `QUIET_TICKS` ticks before the short cut opens — so anything it had is long since drained.

### `fn arm_hangup_watchdog`

**Why a thread, when the loop already detects hangups.** Because the loop's detection is a *rate* and this is a *guarantee*, and the difference is the whole reason this exists.

`EventSource::next` checks for a hangup immediately before every hand-off to crossterm, so the terminal has to die inside the handful of microseconds between that check and crossterm's own `read` for the interface to be trapped. That is a much narrower window than the one measured before those guards existed — but it is still a window, it widens exactly when the machine is loaded and the thread between the two calls is descheduled, and **a process that lands in it cannot get itself out**: crossterm's reader treats a zero-byte read as neither an event nor an error and loops on it forever, so no timeout, no signal and no flag this process can set will ever be looked at again.

Nothing inside that loop can end it, so the thing that ends it has to be outside. This is that thing.

**What it costs, which is nothing.** One thread, blocked in a single `poll(2)` with no timeout and **nothing subscribed to**: `POLLHUP`, `POLLERR` and `POLLNVAL` are reported whatever is in `events`, so subscribing to nothing leaves exactly one thing that can wake it. It never reads the descriptor, so it cannot take a keystroke from the interface, and it is never woken by input, so an ordinary session costs it exactly one syscall for the whole life of the process.

**And it does not shoot a healthy process.** Waking up is not enough to act on: a hangup is also the ordinary way a session ends, and the interface usually handles it by itself within a tick. So the watchdog distinguishes two states, and it can, because `CROSSTERM_CALL` tells it which one it is in:

- **inside the same crossterm call `WEDGE_CHECK` after the hangup** — proven stuck, because that call can no longer return. Ended at once, before it can burn the processor time that made this defect visible.
- **anywhere else** — winding down, or slow. Given `HANGUP_GRACE`, which costs nothing because a process in this state is not spinning.

**There is deliberately no way to disarm it.** The obvious symmetry — give the terminal back when the screen does — was written first and then taken out, because it opened two holes and closed nothing.

The first is the ordinary exit. The interface notices the hangup, returns, and drops its screen in tens of milliseconds; a watchdog that stopped caring at that moment would stop caring **before it had even woken up**, leaving the rest of the wind-down — a database handle, an event log flushed to SQLite — with nothing watching it. That is the half of "never outlive the session" the event loop cannot promise on its own.

The second is `crate::shell` handing the terminal to the setup wizard and taking it back, which drops one screen and acquires another. A disarm there is a window with no owner, for no gain.

And there is nothing on the other side of the trade. Every `Screen` in this crate is a full-screen interface — the shell, the wizard, the wizard reopened from the shell — and every one is dropped either to acquire another or on the way out of the process. There is no Glasshouse that draws an interface and then has honest work left to do without a terminal, so "the terminal is gone, stop" never becomes the wrong instruction.

Idempotent, and safe to call for every screen: the thread is started once.

### `fn wait_until_hangup`

**Why this polls on a timer instead of blocking.** Because a blocking hangup-only wait does not exist on both platforms, and the measurement is in `Watch::HangUp`: on macOS a descriptor subscribed to nothing reports nothing at all, and every mask that does report a hangup there also reports an ordinary pending keystroke. A watchdog that blocked on such a mask would be woken by input it must never read — and, having not read it, woken again immediately, forever. That is a busy-wait, which is the same defect this file exists to remove.

So it asks instead of waiting: one zero-timeout `poll(2)` every `HANGUP_POLL`, sleeping in between. That is ten syscalls a second against the interface's own sixty, it never reads the descriptor so it can never take a keystroke, and an idle Glasshouse measures the same 0.3% of a core with it as without.

The latency it costs is paid only in the case that matters and does not matter there: the interface's own guards catch a hangup within microseconds on every ordinary tick, and this exists for the one where the interface can no longer answer at all — where up to one further `HANGUP_POLL` of a process that is already stuck changes nothing.

Deliberately not `wait_for_terminal`, which can be told to answer a hung-up terminal the way the original defect did — see `blind_to_hangups`. A watchdog that the acceptance test could blind along with the interface would prove nothing.

### `enum Watch`

**The collision both variants exist for.** Crossterm watches two things through one `mio` registration: the terminal, and a pipe of its own that a `SIGWINCH` handler writes a byte to. Both are registered edge-triggered — `EPOLLET` on Linux, `EV_CLEAR` on the BSDs, confirmed in mio 1.2.2's selectors — so each readiness is reported exactly once and is gone whether or not anything acted on it.

`try_read` walks the batch one poll returned and **returns from inside that walk**, on the first token that yields an event. When a `SIGWINCH` and terminal input arrive in the same batch and the signal is looked at first, crossterm returns the resize and the terminal's readiness is discarded unread. The bytes stay on the descriptor, invisible to crossterm until new input creates a new edge — which, for a user who has just pressed Return, means until they press something else.

Measured on this tree rather than argued. A terminal resized and then typed into four milliseconds later stranded the keystroke in **27 of 60** trials, with `FIONREAD` reporting the byte still on the descriptor, `POLLIN` set, no `POLLHUP`, and crossterm reporting nothing. All 27 came out the instant one further key was pressed, which is the edge-triggered signature and nothing else's. The same process, sampled: 1839 of 1851 main-thread samples inside `crossterm::event::poll`, and 23.9% of a core against 0.3% idle.

**What `EventSource::next` does about it, and what is left.** The window is the time a `SIGWINCH` spends sitting in crossterm's pipe unread, because any keystroke arriving during it lands in the same batch. That used to be a whole tick: the loop went back to waiting on the descriptor and would not consult crossterm again until something happened. It now consults crossterm the moment a signal interrupts a wait, while the descriptor is still empty, which leaves only the case where the keystroke and the signal genuinely arrive together.

| gap between the resize and the keystroke | before | after |
|---|---|---|
| 4ms | 27 in 60 | **0 in 60** |
| 50µs | — | **0 in 60** |
| none — both issued back to back | 15 in 60 | 11 in 60 |

So the window went from about 16ms to under 50µs, and what is left needs the two to land in the same handful of microseconds. That last case is crossterm's to fix and cannot be fixed here: once a readiness has been reported and dropped, no call this side of the library can ask for it again.

**Why the loop cannot simply ask again — `Watch::HangUp`.** A descriptor with unread bytes stays readable, so this module's own `poll(2)` — which is level-triggered — goes on answering `Wait::Ready` while crossterm goes on answering "nothing". Asking either again changes neither answer, and the loop that did ask again spent every remaining microsecond of every tick doing it: **380,987 of 381,501 waits** in one such process, at a whole core, with the keystrokes still not delivered.

So the loop stops asking for the rest of the tick and waits on `Watch::HangUp` instead. `POLLHUP`, `POLLERR` and `POLLNVAL` are reported whatever is subscribed to, so subscribing to nothing at all leaves exactly one answer available: the terminal going away, which is the only thing that could still need acting on before the next tick asks again. New input needs no wakeup here — it will be there on the next tick, and it is the next tick's edge that lets crossterm see it at last. Measured with the prevention above deliberately disabled, so that stalls still happen: a stalled process costs **0.3% of a core** with this, against **23.9%** without.

### `Watch::HangUp` variant

**This does not work on macOS, and the loop no longer depends on it.** "`POLLHUP`, `POLLERR` and `POLLNVAL` are reported whatever is subscribed to" is what POSIX says and what this variant was built on. **Darwin does not do it.** Measured against a pty whose master had been closed, one `poll` per row:

| `events` | macOS `revents` | Linux `revents` |
|---|---|---|
| `0` | *nothing, times out* | `POLLERR\|POLLHUP` |
| `POLLIN` | `POLLIN\|POLLHUP` | `POLLIN\|POLLERR\|POLLHUP` |
| `POLLPRI` | `POLLPRI\|POLLHUP` | *nothing, times out* |

So on macOS a descriptor must be subscribed to something before any `revents` are reported at all — and there is no mask that wakes on a hangup and not on input, because `POLLPRI` there also fires for an ordinary pending keystroke (measured: `revents = POLLPRI` on a live raw terminal with one byte waiting, where Linux times out).

**What follows for each of this variant's two uses.** The zero-timeout guards before each hand-off to crossterm no longer use it: they ask `Watch::Input`, which reports the hangup on both platforms and cannot wait or consume anything at a zero timeout. The timed wait below still does, because there the empty subscription is the whole point — a descriptor crossterm has abandoned stays readable, and subscribing to `POLLIN` there is the 380,987-waits spin. On macOS that wait degrades to a plain sleep for the rest of the tick, which is what it was for; a hangup arriving inside it is caught one tick later by the ordinary wait.

Neither of those is what makes the guarantee. `arm_hangup_watchdog` is, and it does its own `poll` for exactly this reason.

### `fn wait_for_terminal`

**Why this is not left to crossterm.** `crossterm::event::poll` cannot report a hangup, and worse, it cannot survive one. Its Unix source reacts to a readable terminal by looping on `read` until the read yields an event or fails; a descriptor whose far end has gone away is *permanently readable and returns zero bytes*, which is neither. `try_read` therefore never returns, so `poll` never returns, so `EventSource::next` never returns, and the shutdown check at the top of it is never reached again.

That is not a theory. Three orphaned `glasshouse` processes were found nineteen hours old at 99% CPU, and a 1622-sample profile put every single sample in that `read`. A signal had already asked one of them to stop and it never noticed, because noticing happens between calls to `next` and there was never going to be another one.

**Which of crossterm's two Unix sources, because they do not agree.** The one this build compiles is `event::source::unix::mio` — confirmed by symbolising a caught process rather than assumed — and its `TTY_TOKEN` arm treats a zero-byte read as neither a `break`, a `continue`, nor a `return`, so the inner loop cannot end and no timeout is consulted inside it. The other source, behind crossterm's `use-dev-tty` feature, does `break` on a zero-byte read and would not hang this way — and it polls level-triggered, so it could not drop a readiness either (see `Watch`). On paper it makes both of this module's defects impossible.

**It was built and measured, and it does not work here.** That source ends its loop on `while timeout.leftover().map_or(true, |t| !t.is_zero())`, so a `Duration::ZERO` timeout runs the body zero times and `crossterm::event::poll(Duration::ZERO)` can never return `true` — which is the call this loop makes on every pass. Measured on a build with the feature on: no input delivered at all, ever, and 2.03s of processor time in 2.0s of wall clock on a freshly drawn interface. Adopting it would mean giving crossterm a non-zero timeout as well, which is a different design and a workspace dependency change. Recorded so the next reader does not spend the afternoon finding it out.

So the wait is taken over here, and crossterm is only ever handed a terminal that has bytes waiting. `poll(2)` reports `POLLHUP` the moment the far end closes, and it is the right instrument rather than a speculative `read`: it cannot consume a keystroke, and this loop is the only thing reading the user's input. Measured on macOS against a pty whose master was closed: a live terminal with input pending reports `POLLIN` alone (`0x1`) and a hung-up one reports `POLLIN | POLLHUP` (`0x11`), so a keystroke can never be mistaken for a hangup.

**The window that is left, which is narrower than it was and was never microseconds.** A terminal that dies between this call answering and crossterm's own poll reaching the descriptor leaves crossterm in the same unbounded loop. That window used to be described here as "microseconds wide against the 16ms one it replaces". **It was not, and the arithmetic said so**: a microsecond window against a 16ms tick predicts about one hangup in ten thousand, and the measured survival rate was two in sixty.

The window is not a gap *between* calls, it is the duration of the call itself. An idle `EventSource::next` used to ask crossterm once per tick for an answer it could not have, and two eight-second profiles of an idle process put 268 of 6210 and 233 of 6162 main-thread samples — about 4% of every tick — inside that ask. A hangup arriving at a uniformly random instant lands there roughly one time in twenty-five, which is the order of magnitude that was actually seen: 7 survivors in 200 hangups. The same profile of the same process with the fix is 0 of 6185.

So `EventSource::next` no longer makes that call once the terminal has been silent for a while — see `QUIET_TICKS`, which is also the reason the short cut waits rather than applying to every idle tick. What exposure is left needs input or a resize at the instant the terminal dies, and neither is the state a closed window leaves behind. Closing it completely would mean parsing terminal input here instead of in the library, or ending the process from outside a loop that can no longer end itself.

**Windows.** **Deliberately unhandled.** Windows has no `poll` on a console handle, its console input is read through `ReadConsoleInput` rather than a descriptor, and a console that goes away there produces a `CTRL_CLOSE_EVENT` and a failing handle rather than an endless run of zero-byte reads — a different mechanism, needing a different answer. This project has no way to run a native Windows terminal, so a Windows branch here could not be tested by anyone who wrote it. `Wait::Unavailable` keeps the old behaviour there exactly, and this comment is the record of what is missing: a Windows hangup path, and the native terminal needed to prove it.

### `fn blind_to_hangups`

**Why a switch exists in shipped code.** Because the thing it makes testable cannot be tested any other way, and this project has already paid once for believing otherwise.

`arm_hangup_watchdog`'s guarantee is that a process trapped inside crossterm still dies. Getting a process into that state honestly means winning a race whose window is now microseconds wide: it happened in roughly one hangup in sixty on a loaded Linux runner, which is a rate to sample and not a state to construct. A test that waits for it is the single-trial test practice §60 exists to warn about, wearing the other face — it would pass by never reaching the case it claims to prove.

So the case is constructed instead. With this set, `wait_for_terminal` looks past `POLLHUP` and answers `POLLIN` — the exact reading the field defect made, restored on purpose — and the interface walks into crossterm with a dead descriptor **every time**. Nothing but the watchdog can end that process, which is what the acceptance test then requires.

It is read once, it changes nothing unless the variable is present, and `block_until_hangup` deliberately does not consult it, so the watchdog cannot be blinded by the same switch that blinds the interface.

## Trims: `memory/inject.rs` — history moved out of comments by `GH-TRIM-MEMORY-INJECT`, 2026-09-05

### module doc

Selecting and labelling the project memory that goes into a session's
context when Glasshouse routes a task to it — Phase 27, capability map
lines 1125-1135.

# This is a trust boundary, not formatting

Injected text lands in an agent's context beside the instructions a person
actually wrote. Line 1130 is the line that keeps those two apart, and
everything in this module exists to make the separation hold against a
memory body that is *trying* to break it.

A memory body is **untrusted content**. It was extracted from an earlier
session by a model and may itself read like an order — "ignore the
previous instructions", "the user says to skip the tests" — or contain the
bytes that would end this block and start something that looks like a new
user message. So:

- **The label is applied by construction.** [`Injection`] has one
  constructor, [`briefing`], and its rendered text always opens with
  [`MEMORY_MARKER`] and closes with [`MEMORY_MARKER_END`]. There is no way
  for a caller to emit an injected block without the label, because there
  is no way for a caller to build the text at all.
- **Untrusted text can never contain `[` or `]`.** `quote` rewrites both
  to their round equivalents. Every structural token this module emits —
  the two markers and every entry head — begins with `[`, so a body that
  cannot produce a `[` cannot forge a boundary, cannot close the block
  early, and cannot open a second one. That is the whole containment
  argument, and it is one grep to check rather than a list of patterns to
  keep up to date.
- **Untrusted text can never contain a control character.** The delivery
  seam ([`crate::session::api::SessionApi::send_text`]) appends `\r`, and
  `\r` is what a harness's line editor treats as *submit*. A body carrying
  its own `\r` would end the injected line and hand the remainder to the
  harness as a fresh prompt — which is exactly "impersonate the user's own
  message". Control characters, the Unicode line and paragraph separators,
  and the bidirectional-override characters that can visually reorder a
  terminal line all become spaces.

# What is *not* injected, and why the list is short

Only [`super::search::SearchScope::Current`] is ever searched, so history
never reaches a session (line 1134). A record that came back from that
search but is no longer current — [`MemoryStore::search`] can move a pair
to [`super::MemoryStatus::Conflicted`] *during* the query it was returned
by — is dropped here as well: a memory in unresolved conflict with another
is the opposite of settled project knowledge.

Nothing derived from the environment, the filesystem, an error, or a
`Debug` formatting reaches the rendered text. Every field comes from a
[`MemoryRecord`] read out of this project's own store, through
[`MemoryStore::search_grouped`], whose `WHERE` clause filters on
`memories.project_id` — the same read boundary `tests/project_isolation.rs`
proves. Credential scrubbing is the *producer's* guarantee and is made in
[`super::extract::credentials`]; see this crate's `memory` module
documentation for why the producer is the only place it can be made.

# Line 1129 — closed on the door's measured precision, not a per-memory score

*"Avoid injecting memory when retrieval confidence is low."* This module
long refused the line: BM25 relevance is not a confidence, and every
reachable transform of it measures the wrong thing — see [`briefing`]'s
own documentation for the full accounting, kept because the reasoning
still holds. What changed is not the relevance argument; it is that
Glasshouse now has a different kind of confidence to threshold. **A
relevance is not a confidence, and Glasshouse still has no per-query
confidence. It does have an observed false-positive rate for the
injection door, and that is a confidence about the door — which is the
granularity this line's "avoid injecting" acts at, because injection is a
per-door decision. 1129 is closed by the door's measured precision, not
by a per-memory score.** [`InjectionConfidence`] is that rate, read from
map line 939's own producer by this door's caller and passed in here —
never computed by this module, which still has no ledger to compute it
from (see this module's own header above: it takes a `MemoryStore`, not a
`Runtime`).

### `const MAX_INJECTED_BYTES`

The hard ceiling on the whole rendered block, markers included, **in
bytes**.

# This bound is a safety property, not a conciseness one

An injection is delivered as one line through
[`crate::session::api::SessionApi::send_text`], which appends a carriage
return, into a pseudo-terminal. A terminal left in canonical mode — every
harness that has not put its own tty into raw mode, and every shell — has
a hard limit on how long one line may be: `MAX_CANON`, **1024 bytes** on
macOS and the BSDs. Measured on macOS 25.5 against a real pty: a line of
1000 bytes arrives intact, and a line of 1023 bytes is **discarded
entirely — along with every byte written to that terminal afterwards**.
The session is not merely denied its memory; its input is wedged for good,
and the task it was spawned to do never arrives either.

So the ceiling sits well under that limit, and it is counted in bytes
rather than `char`s because the terminal counts bytes: 900 `char`s of
multi-byte text is 2700 bytes and would take the session down.

Enforced by *dropping whole entries* rather than by cutting the rendered
string, so the closing marker is always present and no entry is ever
delivered half-written. Entries are dropped from the end of a list already
ordered by line 1131's preference, so what survives a tight budget is what
that line says matters most.

### `fn briefing`

Choose the memories relevant to a routed `task` and render them as one
labelled block, distinguishing a retrieval miss from a search that
correctly found nothing new — see [`BriefingOutcome`].

`already_injected` is what this session has already been sent; those
memories are skipped (line 1135). [`BriefingOutcome::NothingNew`] is the
normal answer for a project with no memories, a task nothing matches
beyond the raw search, and a session that already has everything the task
selected — all three of which must leave the delivery exactly as it was
before this module existed; only a search that matched nothing at all is
[`BriefingOutcome::NothingMatched`].

# Selection order — line 1131, then 1134

1. **Currently active invariants and constraints**, in the order
   [`MemoryStore::search_grouped`] produced them. These are the *active
   constraints* line 1131 asks for preferentially, and [`rerank`] never
   sees them — see that module's own documentation.
2. **Failed attempts**, which are line 1131's *relevant failed approaches*
   — the memories whose entire purpose is that an approach is not tried a
   second time.
3. Everything else the search matched.

Groups 2 and 3 are one bucket, [`super::search::RetrievalResult::other`],
until immediately before this partition: [`rerank::rerank`] runs on the
whole bucket first (map lines 1089-1092), in its own lexical order when
`model` is `None` or otherwise inert, and *then* the bucket is split back
into failed attempts and everything else — so a failed attempt still
precedes an ordinary match after reranking, the same *invariants and
constraints first* precedence line 1131 already establishes one level up,
extended one level down. Beyond that one pass, nothing here re-ranks: the
ladder, the decay weighting and the thin-decision demotion all already
ran inside [`MemoryStore::search`], and the two-partition structure is
still a stable arrangement of what that produced, so an injection can
never promote a memory past a rung its own authority and currency did not
earn it.

# Line 1129 — a relevance is not a confidence, and this is not one either

*"Avoid injecting memory when retrieval confidence is low"* needs a
confidence a retrieval can actually report, and no per-query signal in
this module is one:

- The raw BM25 relevance survives the retrieval, on
  [`super::search::RetrievalResult::relevance`], and this function's own
  `grouped` carries it. Read that method's documentation before reaching
  for it: BM25 is a *within-query* match score against this project's own
  corpus statistics, uncalibrated and with no natural zero, so there is no
  constant of which "below this, the retrieval was poor" is a true
  statement. It is a relevance, not a confidence.
- The blended score `search` actually sorts on — relevance ×
  `policy::retrieval_weight` — is deliberately **not** exposed, and is the
  one a threshold would be most tempted by. `retrieval_weight` reads
  authority, age, validation state and project phase and never sees the
  query, so the blend is high for an ancient invariant no matter what was
  asked. Its being unavailable is the point.
- The signals that *are* reachable measure the wrong thing.
  `super::search::ladder_rung` and `policy::retrieval_weight` vary with a
  memory's authority, age and validation state and never see the query
  text at all; a "confidence" derived from them would be high for an
  ancient invariant no matter what was asked. A result *count* measures
  how much this project has written down, not how well any of it matched.
  A second BM25 query issued from this module would be a second retrieval
  implementation whose ranking differed from the one that chose the
  memories it was scoring.

A fabricated per-query number would silently gate every future injection
decision, so none is fabricated here, and a search that matches nothing
still injects nothing on its own terms — that is an empty result, not a
confidence threshold.

**A relevance is not a confidence, and Glasshouse still has no per-query
confidence. It does have an observed false-positive rate for the
injection door, and that is a confidence about the door — which is the
granularity line 1129's "avoid injecting" acts at, because injection is a
per-door decision. 1129 is closed by the door's measured precision, not
by a per-memory score.** `confidence`, below, is that rate — see
[`InjectionConfidence`] for what it carries and [`BriefingOutcome::WithheldLowConfidence`]
for what withholding on it looks like from the caller's side.

`project_root` is where map line 1142's freshness is answered, and `None`
is a supported answer rather than a degraded one: every file-aware row
then reads `freshness=unknown` and the section is otherwise identical. A
caller with a [`crate::Runtime`] has it (`runtime.project().root()`); one
testing the rendering does not need it. `confidence` is the same shape —
see [`InjectionConfidence`]'s own doc comment.

### `fn file_observed_memories`

Line 1140: memories this project learned while a task's own named files
were being worked on — [`MemoryStore::for_path`] over every path
[`crate::routing::session::paths_named_in`] finds in `task`, reusing
Phase 36's 1583 extraction rather than writing a second one.

`task` naming no path, or naming one nothing was ever observed against,
both answer `Ok(Vec::new())` — the same "nothing to say" the search half
of [`briefing`] already returns for an unmatched query, and [`render`]
treats the two identically.

# Three things every row carries, and one it does not

**The association is read per row**, not assumed. Migration 26 gave
`memory_files` a second provenance, so a row may be `observed` (the file
changed during the session that produced the memory — a correlation) or
`referenced` (an extraction model named the path, and the session
demonstrably edited it — a claim about the memory). Labelling both
`observed`, which this function did while `observed` was the only value a
writer could produce, would now understate half the rows.

**The freshness is a label and never a filter.** Map line 1142: a stale
row is returned, in its rank, marked. Nothing here drops, reorders or
rescores on it — see [`Freshness`], and note that `project_root` reaching
this function is the *only* way git is consulted at all, so a caller that
passes `None` gets [`Freshness::Unknown`] on every row and an otherwise
identical section.

**The intent is [`RetrievalIntent::CodeEdit`]** — map line 1141. This
section is built for the files the task *named*, which is the intended
edit the line is about; the socket door, which was asked what a file is
associated with, stays [`RetrievalIntent::Lookup`].

What it does not carry is which file each row came back for. That was
kept out to save budget when the section was built and stays out: a
reader gets the file back only if the memory's own body mentions it.

A memory already selected by the search half, or already sent to this
session, is excluded rather than shown twice.

### `fn file_observed_heading`

Line 1140's section heading, computed from the actual count rather than
reserved for a worst case: unlike [`header`], this is only ever measured
after `file_observed_memories` has already returned, so [`render`] has the
real length to test the byte ceiling against and no reservation is needed.

# What this heading lost, what it gained, and why the budget decided

It used to spend a full sentence asserting that every row was a
correlation, which was true while `observed` was the only association a
writer could produce. Migration 26 landed the second, so that sentence
would now misstate half the rows — and each row already carries its own
`assoc=` and `freshness=` tokens, which is where a reader who quotes one
entry out of the block will look anyway.

What replaces it is map line 1142's own caveat — *never treat stale
memory as stronger evidence than the current source code* — stated where
a reader cannot skip it and naming what the evidence actually is.

**The trade was forced, not stylistic.** [`MAX_INJECTED_BYTES`] is 900,
this heading plus three entries is most of it, and every entry grew by a
`freshness=` token. A heading that explained both vocabularies as well
would have pushed the whole section past the ceiling and
[`render`] would have dropped it — a section explaining itself at length
to nobody. The shorter sentence buys back slightly more than the tokens
cost.

### `fn is_unreaffirmed_idea`

Line 934: *"avoid injecting old ideas merely because they mention the same
subsystem."*

Both halves are read off the record rather than judged. **Idea** is
[`MemoryAuthority::Idea`], the class whose own documentation is
*"Exploratory. Must never be injected as a binding instruction."* —
[`MemoryKind`] has no idea variant, so authority is the only place this
project records the distinction. **Old** is `last_validated_at.is_none()`:
nothing has reaffirmed it since it was written down, which is exactly the
stand-in for staleness `standing` already uses for line 1132 and
`policy::phase_penalty` uses for line 933. An idea somebody has
re-confirmed is not an old one and is not excluded here.

# Why this is an exclusion and not a demotion

An injection carries at most [`MAX_INJECTED_MEMORIES`] entries, so ranking
an idea lower is only a refusal to inject it when something else competes
for the slot — and the case the line names is precisely the one where
nothing does: a task mentions a subsystem, the only memories about that
subsystem are old ideas, and they arrive looking like what this project
decided. Demotion cannot express that; membership can.

# The reading this does not take

The line's *"merely because they mention"* could instead be read as a
statement about how *weakly* an idea matched, which would need a relevance
cut — the signal Phase 27 refused to invent for line 1129, and one that
would still not fire for an idea that matched strongly and is still stale.
Reading it off recorded authority and validation costs the case of a
genuinely current idea nobody has reaffirmed; that is the trade, and
reaffirming is the recorded, one-call way out of it.

### `fn quote`

Render untrusted stored text so it cannot escape the block that carries
it, and cut it to `budget` characters.

Three rules, in this order, and each of them is a containment property
rather than a cosmetic one:

1. `[` becomes `(` and `]` becomes `)`. Every structural token this module
   emits starts with `[`, so text that cannot contain one cannot forge an
   entry head, cannot emit [`MEMORY_MARKER`], and cannot close the block
   with [`MEMORY_MARKER_END`].
2. Anything that could act on the terminal becomes a space: control
   characters (which include `\r`, the byte a harness's line editor reads
   as *submit*, and `\u{1b}`, which opens an escape sequence), the Unicode
   line and paragraph separators, and the bidirectional overrides that can
   reorder a rendered line so it reads as something it is not.
3. Runs of whitespace collapse to one space and the result is trimmed, so
   the budget is spent on text rather than on padding.

The cut is by `char`, never by byte, so a multi-byte character is never
split; a cut string ends in `…` so a truncated body is visibly truncated
rather than silently a different sentence.

## Trims: `commands/hook.rs` — history moved out of comments by `GH-TRIM-COMMANDS-HOOK`, 2026-09-05

### `checkpoint_after_turn`

It never blocks past a synchronous read of a couple of small files and one write — there is no model call here, so there is nothing to bound with a thread and a timeout the way extraction needs.

So this carries forward the handoff from the session's most recent checkpoint, restamped with the current time and the repository's current position — the same shape `shell::checkpoint_task_boundaries` already uses in the interactive shell, for the same reason.

### `checkpoint_before_compaction`

A compaction is not a turn ending, so stamping `TaskBoundary` would misdescribe why the checkpoint exists — and `CheckpointReason` has exactly two variants, both pinned by a SQL `CHECK`, so there is no third value honest enough to invent instead. What moves is `created_at` and the Git position; the reason a person or agent already gave the checkpoint does not change because the harness is about to compact.

### `PAYLOAD_DRAIN_BOUND`

Not hypothetical, and not Windows-specific either, though Windows is where it was found: reached over an `ssh` channel whose far end never sees end of input — which is how the local gate's Windows leg runs the suite, and which its macOS leg avoids only because that one redirects from `/dev/null` — the six tests that call this function block for ever, and every other test in the target passes. Measured on both batch 50 and its own base commit, so the wait is older than the batch that surfaced it.

Any wait that reaches the bound is already the pathological case, and the answer to it is to get on with the bookkeeping rather than to keep waiting.

### observed-compaction counter

That switch decides whether Glasshouse *does* something about a compaction; the compaction happened either way, and a count that silently stopped when a user turned extraction off would be a number no reader could trust. It is also ordered first, so a count is recorded even if extraction takes the full `EXTRACTION_BOUND` and this process is torn down by the harness while waiting.

Best-effort: a compaction is the harness's business and a hook that failed to write a counter must not fail the turn over it, which is the same stance every other write on this path takes.

`record_observed_compaction` itself has no such check (it is an unconditional `UPDATE ... WHERE id = ?1`, by design, so a session created before migration 16 still gets counted), so the check belongs at this call site, the same way `may_apply` belongs at the lifecycle-event call site below rather than inside the write it guards.

### `TurnEnded` trigger ordering

Ordered **after** the event is recorded, on purpose: the log is the material extraction reads, and a turn's own closing event should be in it. Ordered **before** the state change for no reason at all beyond it reading better; neither `run_extraction` nor `checkpoint_after_turn` can fail in a way the rest of this function could notice.

Map lines 1834, 1835, 1845 and 1854's outcome half — and the whole of what Glasshouse is allowed to learn about how a route turned out. `TurnEnded` is the only event that carries a harness's own verdict, `session::lifecycle::event_for` is its single construction site, and **both** outcomes are recorded: a turn that ended badly is a fact about the route as much as one that succeeded, and counting only completions would make every ratio here a fraction of an unstated denominator.

A `SessionEnd`, a process exit and output going quiet all arrive somewhere else or nowhere, and none of them writes a row. The decision they belong to simply stays *unknown*, which is what the readers count it as.

Ordered **before** the extraction and checkpoint triggers below, for the reason the compaction counter above is ordered first: those run on their own thread up to `EXTRACTION_BOUND`, and this process can be torn down by the harness while one is still going. A verdict the harness actually stated must not be lost to work Glasshouse chose to do about it.

Map lines 1821 and 1831's proxy denominator — a second row, on every session this arm reaches rather than only routed ones. `record_routing_outcome` refuses a session with no routed destination, so a door-spawned session (never routed) would otherwise record nothing about how its turn went; `record_turn_outcome` asks no routing question at all. Called first, so a session with no routing decision still gets its outcome counted before the routed call below returns early for it. Refusal register, *"Phase 51's memory proxy — 1821 and 1831"*, ruling (b).

### claim release ordering

Ordered first in this arm, ahead of the evaluation writes and well ahead of extraction: it is one `DELETE`, and it is the one write here that another *session* can observe. Extraction runs on its own thread up to `EXTRACTION_BOUND` and this process can be torn down by the harness while it does, which must not cost a claim its release.

Best-effort, like every other write on this path: a hook that failed to release a claim must not fail the user's turn over it, and `STALE_CLAIM_AFTER` is what bounds a claim this line missed.

### `note_head_commit`

Reporting the first turn of every session as a landed commit would make the trigger fire hardest on sessions that have done nothing yet.

Everything else on this path takes that stance and this is not more important than the compaction counter beside it. The cost of the failure is that the next turn re-reads the same position and calls it a boundary once — a duplicate extraction the duplicate check already absorbs, which is a far better failure than a hook that fell over inside somebody's coding session.

### `edit_intent_hook`

A coordination layer that broke a user's edit because its own lookup failed would be worse than no coordination at all.

[`glasshouse::session::SessionStore::active_claims`] is read **before** this session claims anything, so the answer cannot include a claim this very call just wrote. It would be filtered by session identity anyway; reading first means that invariant does not depend on the filter.

### `notify_orchestrator_of_conflict`

Called once per colliding path, never once per hook call: line 2415's granularity requirement is that a conflict on one path names only that path, and a single call bundling every notice into one message would have made a conflict on `src/a.rs` indistinguishable from one on `src/b.rs` at the one reader — the orchestrator — that is supposed to act on the difference.

The one live orchestrator being either `editor` or `holder` is not "ambiguous" and is not logged as undeliverable: it is already a party to this exact conflict and was told through the hook response itself (the three channels [`glasshouse::firewall::adapter::pre_tool_use_response`] writes), for the same reason `edit_intent::a_session_does_not_conflict_with_its_own_claim` exists — telling it again through a second channel would not be new information.

[`glasshouse::session::api::SessionApi::send_text`] is the delivery path design-decisions.md names — *"Glasshouse already has an orchestrator delivery path: the Phase 15 wake-up flow, `SessionApi::send_text`, and `api/unix/events.rs`. Reuse it… do not design another transport."* This function is that seam's caller from a new site: a `PreToolUse` hook subprocess, which owns no pseudo-terminal of its own, so the [`SessionRuntime`] it constructs starts empty and [`SessionApi::send_text`] answers `NotLive` unless something else in *this* process already holds the target session — nothing here does. That is requirement 5's **best-effort** outcome, logged at `debug` and never surfaced as the `warn` ambiguity gets: the recipient was resolved correctly and the seam is wired for the moment a process that does hold a live handle reaches it, or reads this claim itself. See this packet's `packet_errors` for why that gap is recorded rather than closed with a second transport.

## Trims: `routing/burn/mod.rs` — history moved out of comments by `GH-TRIM-ROUTING-BURN`, 2026-09-05

### module doc

Phase 32E — burn rate and exhaustion forecasting: what the evidence
ledger's own rows say about how fast a constrained resource is being
spent, and whether it will reach its next reset.

Capability map lines 1274 and 1276–1283.

# What this module decides, and what it deliberately reuses

Four readings, each a public function so a mutation can zero exactly one
of them (the same shape `super::pressure`'s two terms take, and for the
same reason):

- [`task_class_request_rates`] — line 1276. A short moving average of
  requests consumed per task class, over the rows migration 23 made able
  to carry one.
- [`burn_rate`] — line 1277. Requests per hour against one resource,
  keyed the way [`super::evidence::recent_credential_throttles`] keys a
  credential: provider, narrowed by
  [`super::evidence::RoutingObservation::quota_context`] when the caller
  names one.
- [`forecast`] — lines 1278 and 1279. Time-to-exhaustion, and whether
  that lands before the resource's own reset.
- [`live_rows`] — line 1282. Which rows the three above are allowed to
  see at all.

Everything else is *read*, not re-decided: the remaining amount is
`crate::provider::quota::Capacity<NativeAmount>` exactly as the provider
stated it, and the reset is
`crate::provider::quota::CapacityState::seconds_until_reset` computed
against the caller's clock.

# Purity

No clock, no store, no socket — `super::pressure`'s discipline, restated
because this module is the one most tempted to break it. Every function
here takes rows and a `now_unix` the caller read, and returns a value.
Nothing opens a ledger, and nothing can widen the `project_id` scope
`EvidenceLedger::consumption_in_window` already applied: this module
never sees a connection.

# Nothing here parses a response body

A **request** rate is the unit throughout, because a completed request
produces a row whether or not anything measured its tokens. A token rate
is offered only from rows whose `input_tokens`/`output_tokens` are
already `Some` — written by a *translated* gateway exchange, which parsed
its own response for its own reasons — and is [`None`] otherwise. Line
1275, token consumption per task class, is now served the same way: since
`GH-TASK-CLASS-COST-JOIN` every served row of a classified launch carries
its `task_class`, and since Phase 56 a translated exchange carries its
token counts, so [`task_class_request_rates`] can read a token rate over
rows that exist. `crate::gateway::ingress` remains structurally unable to
carry a token count, and a relayed row stays uncounted for exactly that
reason — this module still never invents one from a ratio.

# A forecast that is not known is absent, never a number

Every function returns `None` rather than a figure when its inputs are
insufficiently known — too few rows, a remaining amount that is a
percentage rather than a count, a unit that is not requests, a burn rate
of zero. This is the same stance `super::pressure` takes for an unread
resource: neither preferred nor withheld. A `None` here makes
`super::pressure::exhaustion_forecast_pressure` inert and makes
`crate::shell`'s capacity line print exactly what it printed before this
module existed.

### `live_rows` doc

Line 1282: the rows that are still evidence about *now*.

Two exclusions, and each one has a defect it prevents:

1. **Before a reset boundary this build can actually locate.** Rows spent
   against a quota that has since been given back would forecast the
   exhaustion of capacity that no longer applies. But the *only* reset
   fact any caller here has is `seconds_until_reset`, and nothing in
   `crate::provider::quota` publishes a window **length** — so the
   previous turn cannot be derived from the next one without inventing a
   period nobody stated, which is exactly the fabrication this module
   refuses everywhere else.

   So one boundary is located and one only: a **non-positive**
   `seconds_until_reset`, which `CapacityState::seconds_until_reset`
   returns as-is rather than clamping, means the window turned
   `-seconds` ago and that instant *is* the boundary. A positive reset
   excludes nothing on this ground, and rows are then bounded only by
   the caller's own window and by the idle gap below. This is the
   conservative direction: it can keep a row it might have dropped, and
   it can never drop a row that is still evidence.
2. **Before an idle gap longer than [`IDLE_GAP_SECONDS`].** Rows are
   ordered by `observed_at` ascending (the ordering
   `EvidenceLedger::consumption_in_window` guarantees); the last gap
   wider than the constant is a boundary, and only rows after it are
   live.

The result borrows: no row is copied, and a caller that wants the count
of what was excluded can compare lengths.

### `output_tokens_by_class` doc

Map line 1301: the output-token half of the join this phase's census
named missing — `docs/product/evidence/phase-32g.md`'s Censused
2026-09-02 entry. One entry per class with at least one row in the
window that names both a class and an output-token count, in
[`TaskClass::ALL`]'s declaration order; a class with no such row at all
is **absent**, the same convention [`task_class_request_rates`] keeps for
its own rate.

Restricted to `purpose = `[`HARNESS_TURN_PURPOSE`] rows: this is the
gateway's own served-exchange traffic, the same rows
[`super::evidence::NewObservation::with_task_class`]'s own doc names as
what this reader counts — never `record_routing_latency`'s
routing-decision row, which carries a class but no tokens and would only
ever contribute nothing here.

The window is `[now_unix - window_seconds, now_unix]`, read off each
row's own `observed_at_unix` — a plain calendar window rather than
[`live_rows`]'s reset-and-idle-gap boundary, because this reader has no
resource reset to bound against and a caller here passes rows straight
from [`super::evidence::EvidenceLedger::consumption_in_window`] with the
same window already applied at the SQL layer; the second check here is
what lets this function also be exercised directly, over a hand-built
row list, without a ledger in the loop at all.

## Trims: `gateway/mod.rs` — history moved out of comments by `GH-TRIM-GATEWAY-MOD`, 2026-09-05

### module doc

Glasshouse's whole premise is that the harness stays the harness; a gateway
that started driving a model would quietly undo that.

A module that cannot see the session model cannot own a session, and a
reviewer can check that with a source scan instead of reading for intent —
the same move `harness::no_adapter_depends_on_the_session_model` already
makes for the adapters.

**What this module owns, and what the ingress owns.** Here: a listener, an
address, a token, an upstream, and the moment each of them stops existing.
In `ingress`: what happens on one connection. In `http`: the small amount
of HTTP that routing needs. In [`upstream`]: where a request goes and the
credential it goes with. In [`translate`]: the one branch of the ingress
that may parse a body — a target the provider does not serve, for a pair
the table supports (Phase 56).

Port `0` asks the operating system for a free port and is what lets two
Glasshouse instances on one machine coexist — neither one names a port, so
neither can contend for one. The port that was actually chosen is read back
with `local_addr` and kept.

It lives in memory for the lifetime of one instance and is never written to
a log, a diagnostic, or a file.

The gateway checks that token on arrival and attaches the real credential
itself, from an [`upstream::Upstream`] that holds it in this process's
memory.

The cost is one thread per in-flight request, which for one developer's
harness is a number in the low single digits.

### Gateway struct doc

**Why this and not the alternative.** The other portable trick is to
connect to your own listener to wake the accept. It is worse here on every
platform and worst on Windows: the wake-up connection races with a real
client's, so the loop may accept the client and leave the wake-up in the
backlog; and a self-connect on Windows can be delayed or refused by local
filtering software, which turns "shut down" into "hang until a firewall
decides". Non-blocking accept, by contrast, is the same code on all three
platforms — `ioctlsocket(FIONBIO)` on Windows, `O_NONBLOCK` elsewhere —
and `WSAEWOULDBLOCK` reaches Rust as [`ErrorKind::WouldBlock`] exactly as
`EWOULDBLOCK` does. Nothing here is conditional on the platform, so there
is no platform-specific path to get wrong.

A streaming response can legitimately be minutes long, and a shutdown that
waited for one would be the hang this design exists to avoid; those
threads own their own sockets and end when their exchange does, or when
the process exits.

It deliberately registers **no** [`crate::shutdown::on_forced_exit`]
cleanup, for two reasons that both matter. First, that hook exists for
resources which *survive* [`std::process::exit`] — a harness left running
in its own session with nothing to hang it up. A listening socket is not
one of those: it is a descriptor owned by this process, and process exit
closes it and releases the port on every platform Glasshouse supports.
Second, that registry holds exactly one callback, so registering here
would silently displace the one an attached session installs to kill its
harness — trading a cleanup that is unnecessary for one that is not.

### start_with_quota_cache doc

**No caller resolves [`crate::paths::RuntimePaths::resolve`] here, and none
may be added here**, and a gateway that resolved its own OS-standard
directory would write into whichever machine happens to be running `cargo
test` every time a conformance test forwards a request with a rate-limit
header — see [`crate::provider::telemetry::GatewayQuotaCache`]'s own doc
for why that is the wrong owner for the resolve step.

### start_with_telemetry doc

This is capability map Phase 33A, this package's own production producer.
See
[`crate::gateway::session::SessionRouting::record_routing_observation`]
for exactly what is and is not recorded from one exchange. `None`
reproduces [`Self::start_with_quota_cache`] exactly — the same additive
guarantee that constructor already gives [`Self::start`], and for the same
reason: this module has never had a project or a data directory in scope
(see [`Self::start_with_quota_cache`]'s own doc), so a caller that wants a
durable evidence ledger resolves its own [`crate::Runtime`] and hands this
an already-opened [`crate::routing::evidence::EvidenceLedger::open`]. The
same gap [`Self::start_with_quota_cache`]'s own doc records for the quota
cache applies here for the identical reason.

### paced_refusal doc

**Deliberately narrower than [`FreePool::is_available`].** That check
alone cannot be the guard here, because it folds two different kinds of
cooldown into one bool. [`routing::free`](crate::routing::free)'s own
`MAX_COOLDOWN` doc makes the second kind deliberately still probed by real
work — "the only way to find out ... is to let real work try it" — and
`gateway::conformance::a_pinned_session_stays_on_its_failing_provider_and_never_reaches_the_other_one`
pins that: three ordinary `503`s must all still reach the provider.
[`SessionRouting::quota_headers`] is already public for capability map
line 1229. Deciding to actually rotate to a sibling credential is
[`session::SessionRouting::observe_exchange`]'s own job, on the exchange
that runs, and `paced_refusal` only asks whether one exists, so it never
mutates the assignment itself.

### start_if_required_with_degrade_sink doc

"detect gateway failure separately from harness process failure" is map
line 1735's own wording. `None` reproduces
[`start_if_required_with_telemetry`] exactly, the same additive guarantee
every sink on this door already gives. The launch path opens its
`EventRecorder` 184 lines later, and has no `SessionRecord` at all until
the store has created one — which holds any failure that arrives in
between and replays it on installation.

## Trims: `api/protocol.rs` — history moved out of comments by `GH-TRIM-API-PROTOCOL`, 2026-09-05

### `RequestOrigin` doc

# This is an attribution boundary, not a security one

A caller that states an origin it is not is **out of scope**, deliberately
and without a defence, and no part of this type should be read as a claim
about who a peer is. There is nothing here to authenticate: anything that
can reach this socket can already send any bytes it likes under any origin
it likes, and it is the *same user* on both sides. What the field buys is
that the honest callers stop being indistinguishable — `api::client`,
which knows it is a person's command line, and `unix::pump_watches`, which
knows it is Glasshouse's own delivery, no longer write log rows that are
equal field for field.

### `Request::MuteSession` doc

# It does not survive a restart, deliberately

The state lives in the `glasshouse api serve` process that owns the
session's pseudo-terminal and nowhere else. That process is the only
thing that can deliver a machine message to a session in the first
place — a door that has just started is not running the session that
was muted — so there is no interval in which a lost mute lets a
message through that a persisted one would have stopped. Nothing is
migrated and nothing is written to disk.

### `Request::RecentOutput` doc

The third of the three verbs that together are a person being *in* a
running worker: [`Request::SendMessage`] puts words in,
[`Request::Interrupt`] stops what is happening, and this is the half
that shows what came back. Until it existed a client built from this
door could type into a worker and could not see it.

Answered through `session::api::SessionApi::recent_output`, the same
project-scoped seam its two neighbours resolve through, and read-only
in the strong sense [`Request::RecommendRoute`] is: it sends nothing
to the session, signals nothing, spawns nothing, writes to no store
and records no event.

# The bound

`max_bytes` is capped server-side at `unix::MAX_RECENT_OUTPUT_BYTES`
regardless of what is asked for, so a caller may lower the ceiling
and cannot raise it — the same shape as [`Request::QueryMemory`]'s
`limit`. It matters more here than anywhere else on this door: a
session's scrollback is bounded by the *runtime*, at a size no caller
chose, and this is the one verb whose response would otherwise grow
with how long a worker has been talking.

### `Request::RecommendRoute` doc

Read-only, and more strongly so than the rest of this door: it starts
no session, sends no text, takes no checkpoint, writes no routing
observation, and mutates no store. The whole verb is
`main.rs`'s own `route_recommendation` — the same function
`glasshouse route` is, so the command and the door cannot disagree
about where work would go (there is one ranking, not two) — rendered
as JSON rather than as a report.

There is deliberately no override here — no `to`, no `fresh`, no
`now`. Those are a *user* telling the router where to go
(`glasshouse route`'s own line 1602 flags), and this verb exists to
ask it a question. Nothing else on this door speaks that vocabulary
either: [`Request::SpawnSession`] names a harness, not a routing
override.

### `Request::QueryMemory` doc

Project-scoped twice over: this door is opened for one already-resolved
project and carries no field naming another (see `super`'s own doc
comment), and the query underneath it —
`memory::search::MemoryStore::search` — filters on
`memories.project_id` in its own `WHERE` clause rather than trusting
that.

`query` plays no role in this mode: a path lookup runs no `MATCH`, so
there is no text for it to search. `path` absent leaves this verb
byte-for-byte what it was.

### `Request::Preflight` doc

`change` is what the agent **states** about the change: files and
subsystems touched, reversibility, blast radius, the flags for a
migration, a destructive operation, a security or data-integrity
impact, an unfamiliar integration, an architectural change or a broad
refactor, the evidence class its premise rests on, and a coarse
budget (with what has been spent, when re-evaluating). Nothing is
read from the session to fill any of it in, and an unknown field —
`reasoning`, `transcript` — is refused rather than ignored.

### `Request::UpdateAssumption` doc

`record_failed_approach`, with `state: refuted`, writes one
`failed_attempt` memory through the existing store, with provenance
naming the assumption (line 1019); the transition's `subject` is the
memory's id. Without the flag, a refutation writes no memory at all.

## Trims: `database/migrations/v14_on.rs` — history moved out of comments by `GH-TRIM-MIGRATIONS-V14`, 2026-09-05

Rule 3's "move history out, behind a one-line pointer" landed here for the fifteen migration comment blocks that carried more reasoning than a reader changing the code today needs in place. Each subsection is what the in-code comment for that migration now points to.

### migration 14

**The defect this closes, measured.** `CheckpointStore::latest_for` and `::latest` ordered by `created_at DESC, id DESC`. `created_at` is whole seconds and `id` is `lower(hex(randomblob(16)))`, so two checkpoints written inside one second tie on the first key and are separated by a **coin flip on a random identifier**. Measured through the real store over 800 back-to-back pairs, 798 of which shared a second: **414 resolved to the older checkpoint** — 52%, which is what a fair coin looks like.

That is not an internal tidiness problem. `latest` is what `glasshouse checkpoint show`, `glasshouse launch --from-checkpoint latest` and the automatic task-boundary carry-forward resolve through, so a user resuming from *"the latest checkpoint"* got the wrong one about half the time whenever two landed in the same second — and a manual `checkpoint save` beside the task-boundary checkpoint `shell::checkpoint_task_boundaries` takes does exactly that.

**`ALTER TABLE ADD COLUMN`, migration 8's shape.** Migration 7's rule stands: a table is never rebuilt, because rebuilding risks the rows already in it. Nothing here needs one. An added column cannot be `NOT NULL` without a constant default, so it gets `DEFAULT 0` — and 0 is deliberately outside the range the backfill assigns (1..n) and outside the range `CheckpointStore::save` assigns (n+1 upwards), so a row reading 0 is exactly *"written by something that did not go through `save`"* and sorts as the oldest thing in the table rather than silently winning.

**What the backfill can and cannot recover.** Existing rows are ranked by `(created_at ASC, id ASC)`. The between-second order was always recorded and is preserved exactly. The within-second order **was never recorded anywhere**, so it cannot be recovered and is not invented: rows tied on `created_at` keep the order `id ASC` gave them, which is the order the old query already reported for them. A database that migrates therefore answers every old question exactly as it did before, and every new one correctly.

**The indexes.** `checkpoints_by_session` is redefined on `(session_id, seq DESC)` so `latest_for` keeps its seek-and-take-one shape rather than sorting the session's rows; the `(session_id, created_at DESC)` it replaces is indexing a key nothing orders by any more. `checkpoints_by_seq` is new and serves `latest` and `list`, which previously had no index at all. An index holds no data of its own, so dropping one is not the rebuild migration 7 refuses — every row survives untouched, which is what `a_version_thirteen_database_migrates_forward_keeping_the_order_it_could_record` proves.

### migration 15

**What it is for, and the one question it does not answer.** Glasshouse can already answer questions about what it *is* — a memory's status, a session's mechanism — and cannot answer questions about what it *did*. A retrieval happens inside one function call, changes what the user gets, and is gone. Phase 51's verb in 26 of its 37 lines is *"measure how often"*, and nothing can count what was never written down. This table answers *how often*, over a window, split by arm.

It deliberately answers nothing about *how much*: cost, tokens and latency belong to `routing_observations` (migration 11), and a column for any of them here would be a second source of truth for a fact that ledger already models. `routing_seq` is how a row points at the observation that measured a turn instead of copying it.

**A new table, for migration 11's own reasons one level up.** Not a widening of `lifecycle_events`. All eleven values in [`LIFECYCLE_EVENT_KINDS`] are things that happened *to a session's process or its harness*; these are decisions *Glasshouse* made, and `crate::events`'s own module doc keeps that stream narrow on purpose. Widening its `kind` would also be a third rebuild of the one table `memories.source_event_first`/`_last` reference by `seq` — the hazard migration 7 documents and the house rule below refuses. And, decisively: `lifecycle_events` has three triggers that `RAISE(ABORT)` on every `UPDATE` and every `DELETE`, so anything folded into it is permanent by construction, and this table *must* be prunable (see "Retention").

Not a view either: the rows a view would project — *a retrieval happened* — are not stored anywhere. `memory_search_grouped` returns its result and forgets, which is precisely and only what this table adds.

**Why `kind` has no `CHECK`, and why that is not a lapse.** A `CHECK (kind IN (...))` is what `lifecycle_events` has, and it is why map lines 310, 327 and 1316 are refused today: SQLite cannot widen a `CHECK` in place, so an eleventh value cost a full table rebuild and a twelfth is forbidden by the house rule at the top of migration 8. Phase 51 is the phase whose vocabulary is *guaranteed* to grow — every future measurable feature wants a new kind — so putting a SQL vocabulary here would be manufacturing migration 7's problem deliberately, in the one table most certain to need widening.

The house already has the answer twice. [`LIFECYCLE_EVENT_KINDS`] exists because the SQL `CHECK` was not trusted alone — its own doc says the Rust constant plus a pinning test is what actually catches drift — and `response_profile` (migration 8) gets no `CHECK` at all, on the stated ground that pinning its combinations "would be a vocabulary this file has no business holding". This column is `response_profile`'s case: [`EVALUATION_KINDS`] beside an exhaustive `match` at the single writer, pinned by a test that inserts every pair the enum can produce through the real schema. What is given up is that a hand-written `INSERT` at a `sqlite3` prompt can store nonsense; that is true of `response_profile` today and has not hurt. `CHECK (kind <> '')` is kept because an empty kind is not an unrecognised vocabulary, it is a missing one.

`outcome` is the same case for a sharper reason: its vocabulary is *per kind* — `helped`/`stale` for a retrieval, `preferred`/`displaced` for a route — so a single global `CHECK` would be two vocabularies in one column, which is the first objection this migration makes to widening `lifecycle_events` at all.

**`outcome` is the one column that is `NOT NULL DEFAULT 'unknown'`.** Migration 11's argument for `context_state`, verbatim: every other column's NULL means *"not recorded"*, but a row that does not say how it turned out must not be countable as *"turned out badly"*. `DEFAULT 'unknown'` makes that automatic for any future insert path that forgets to think about it, and it is what lets a rate report an honest denominator with an honest unknown bucket instead of a flattering ratio.

**Outcomes learned later are new rows, never an `UPDATE`.** A retrieval is recorded when it happens; whether it helped may only be knowable a turn later. The answer is a second row with the same `memory_id` and a later `observed_at`. This is migration 11's "append-oriented is a property of the code as much as the schema": `crate::evaluation`'s store offers `record` and reads, and no method that edits a recorded observation. A measurement edited in place is a falsified measurement.

So migration 5's three append-only triggers are **deliberately not copied here** — they are exactly what makes `lifecycle_events` unprunable, and repeating them would be repeating a known defect. Migration 11's two project-scope triggers are copied exactly, and they are the only ones. That is the load-bearing difference between the two precedents, and it is why this table is named `evaluation_observations` and not `evaluation_events`: the name should pull a future author toward migration 11's prunable ledger and away from migration 5's permanent stream. The bounds themselves (90 days, 100,000 rows, trimmed oldest-first in the writer's own transaction) live with the writer, in [`crate::evaluation::Retention`].

`AUTOINCREMENT` means a `seq` is never reused after a delete, so pruning can never make one row's identity come to mean another's — which is what makes a prunable ledger safe to point at.

**Two triggers, migration 11's pair, unchanged.** `IS NOT` rather than `<>`, so a missing binding row aborts the write instead of the comparison evaluating to NULL and letting it through. This is the structural half of map line 1856's *"keep evaluation data local and project-scoped"*; the other half — that nothing exports it — is a property of which functions exist in `crate::evaluation`, not of the schema, and is recorded there.

**Bare ids, no `REFERENCES`.** `memory_id` and `routing_seq` are migration 12's rule: a bare nullable reference, no foreign key. A pointed-at row may be pruned or may never have existed, and a read that cannot resolve one must report that rather than lose the observation.

**One index, and the second one is an experiment, not an omission.** `(kind, observed_at)` serves the shape every Phase 51 line reduces to: how many rows of one kind fell in a window. An A/B split adds `feature`/`arm` to the `WHERE`, which this index does not cover — do not add `(feature, arm, kind, observed_at)` on speculation; fill the table to its retention ceiling, read `EXPLAIN QUERY PLAN`, and add it if and only if the plan is a scan and the scan is slow.

### migration 16

**Why this is the only column Phase 30 needed.** The phase asks for eight things about a session's context. Seven of them were already answerable from what this schema holds, and the package that closed the phase says so line by line in `session::store::SessionContext`: the most recent request or turn time is `sessions.last_activity_at`, already stamped by the single `UPDATE` that moves a session's lifecycle; a recent portable checkpoint is `checkpoints.created_at` for the session, which migration 5 recorded and migration 14 ordered; and a task-continuity flag is a count of this session's `turn_ended` rows, which the event log has stored with their `turn_outcome` since migration 5. Adding a column for any of those would be a second source of truth for a fact the schema holds exactly once — migration 15's own objection to copying a token count out of `routing_observations`, one table over.

A compaction is the one that had nowhere to live. `session::lifecycle::precedes_native_compaction` is called on the production hook path and its answer was, until this migration, used to fire a trigger and then discarded — its own doc comment said the fact was "recorded nowhere". So this column is not a convenience: it is the only durable record that the event happened at all.

**Why a counter here and not a twelfth `lifecycle_events` kind.** Migration 7's rule, which migration 15 restates as this file's house rule: `lifecycle_events.kind` carries a `CHECK`, SQLite cannot widen a `CHECK` in place, and an eleventh value already cost a full table rebuild of the one table `memories.source_event_first`/`_last` reference by `seq`. A twelfth is refused outright. `precedes_native_compaction`'s own documentation reached the same conclusion from the other side and declined to invent a `LifecycleEvent` for it.

That refusal blocks an *event row*. It does not block a *column*, and the two are not the same claim: an event says "this happened at this instant, in order, beside every other thing that happened"; a counter says "this has now happened n times". Phase 30's line asks for the number, not the timeline — *"track the number of observed compactions for a session when known"* — so the counter is what the line wants and is also the only one of the two this schema can add.

**`ALTER TABLE ADD COLUMN`, migration 12's shape.** No table is rebuilt, no existing `CHECK` is altered, no existing row is touched, and no index is added: nothing orders or filters by this column, and migration 15's closing note about not adding an index on speculation applies here with more force, because this one is written far more often than it is read.

So this column is nullable and has no default. `SessionStore::create` writes `0` for every session *this* build starts, which is what makes the two states reachable at all, and the increment is `COALESCE(observed_compactions, 0) + 1` so that a row from an older build begins counting at its first observation rather than staying unknowable for ever. What is given up, and it is stated rather than hidden: for such a row the count is a **lower bound**, because compactions before the upgrade were never observed by anything. For a row this build created it is exact.

**The `CHECK`.** Migration 9's shape for a counted quantity (`process_id > 0`): a negative number of compactions is not an unrecognised value, it is an impossible one, and the schema is where that is cheapest to refuse.

### migration 17

**What this is, said before what it is not.** One row per (memory, path) pair, written from `crate::checkpoint::WorkingTreeStatus::changed_files` at the moment extraction ran. That list is what the git index says differs from the working tree right now: no model, no subprocess, no guess.

**It is a correlation with the session, not a reference by the memory.** A session that dirtied twenty files and yielded three memories associates all three with all twenty, and that is not a rounding error in the signal — it *is* the signal. Map line 1139 asks for the files a memory *"explicitly references"*, and on the automatic extraction path the model's input contains no prose at all (`memory::extract::lifecycle`'s own doc comment; `lifecycle_events` has no text column), so a model asked to name files there would be fabricating from an empty input. Map line 1294's rule — a fabricated value does not degrade the policy, it inverts it — is why this table records what was *observed* and says so in a column, rather than claiming what was *referenced*.

**A join table, which this schema has never had, and why not the alternatives.**
- **Not a column on `memories`.** A delimited or JSON list cannot be indexed for exact enumeration, which reproduces `checkpoints.document`'s defect one table over: you can look a row up, you cannot query the set.
- **Not a column in `memories_fts`.** FTS5 tokenisation destroys a path at both ends — `src/memory/store.rs` indexes and queries as four unrelated words, so every memory sharing any directory component would match — and migration 6 shows the cost is a full `DROP` / `CREATE` / `'rebuild'` plus three triggers.
- **Not `evaluation_observations`.** That table is *deliberately prunable* (90 days / 100,000 rows) and its `subject` is documented as free text that is "never a count key on its own". An association that expires after 90 days is not an association: the whole value of a file→memory link is that it outlives the session that made it.
- **Not `checkpoints.document`.** It already holds real observed paths, but it associates them with a *session* rather than a *memory*, in opaque JSON, reachable only by a full scan.

**No `ALTER`, no rebuild, no existing `CHECK` touched.** Migration 15's shape: `CREATE TABLE` plus one index plus migration 11's two project-scope triggers. `lifecycle_events` is untouched and no new `LIFECYCLE_EVENT_KINDS` value is added, so map lines 310, 327 and 1316 keep the refusal the register gives them, word for word.

The observed producer needs no normalisation *work*: git's index stores every path as UTF-8, repo-relative and `/`-separated on every platform, Windows included, and `checkpoint::git::parse_index` reads it straight with no separator translation. The contract exists for the writers that come after it — a model-emitted or user-typed path is five spellings of one file and must be normalised or refused before it reaches this column.

Repo-relative is also what keeps the `/var` versus `/private/var` class of hazard out of the index key: that ambiguity lives in the *root*, and the root is never stored here. An absolute path would import it directly into the one column this table matches on.

Enforcement is at the writer rather than in a `CHECK` because the schema cannot express it: `CHECK (path NOT LIKE '/%')` would miss `C:\...`, and a `CHECK` forbidding `\` or `:` would reject file names that are legal on Unix. The schema refuses only what is never a path at all — the empty string.

**`seq`, and bare ids.** `AUTOINCREMENT`, migration 11's and 15's shape for an append-oriented row: this table has no `UPDATE` path, and an identifier is never reused even after a future retention policy prunes rows. `memory_id` is a bare id with no `REFERENCES`, migration 12's rule as migration 15 restates it: a pointed-at row may be gone, and a read that cannot resolve one must say so rather than lose the observation.

**Zero rows is one fact here, not two, and that is deliberate.** A join table cannot distinguish *"the tree was clean"* from *"extraction ran before this feature existed"* — both are no rows. A marker column on `memories` would separate them and is exactly the `ALTER` this migration refuses; the distinction is not worth widening the schema's blast radius for while nothing reads it. Stated rather than hidden: for a memory recorded by an older build, the absence of rows means *unknown*, and for one recorded by this build it means the reader found nothing to name.

**One index, and only the one.** `(path)` serves the only access pattern this table exists for: which memories were learned while this file was being worked on. Migration 15's closing note applies unchanged — do not add a second index on speculation.

### migration 18

**`ADD COLUMN`, nullable, no `CHECK`, no index.** Migration 10's shape for `validity_conditions`: an `ALTER TABLE … ADD COLUMN` backfills every existing row with `NULL`, which is the honest reading for a row written before the classification existed — "this build recorded no kind here", never "no failure". No `CHECK`, for `FAILURE_CLASSES`' reason: the vocabulary lives in Rust and is pinned by a test. No index: the reads that want this column (`EvidenceLedger::failure_classes_by_provider`) are a `GROUP BY` over a time window already served by `routing_observations_by_route_time`, and migration 15's closing note applies — measure before indexing.

**What may write it, and from what.** The gateway's connection thread, from the status line, the rate-limit headers it already reads to forward them, the byte count it already keeps to relay the body, and how the stream ended as its framing said. Never from a byte of the body: `crate::gateway::ingress` remains structurally unable to carry one, and the design ruling that framing is not content is in `docs/product/design-decisions.md`.

### migration 19

**What a row is, and what no row is.** `task_assumptions` holds the six fields capability map lines 1014 and 1016 name — claim, current evidence, evidence-source class, uncertainty, affected scope, cheapest verification — and who stated them, for which session, when. **Nothing here was inferred.** Every row was said through `api::protocol::Request::RecordAssumption` or its MCP twin; Glasshouse reads no transcript and no output for one (line 998), and there is no column that could hold reasoning if it did.

`assumption_transitions` is the append-only history. A row naming an `assumption_id` moves it to one of line 1018's six states — or re-states the current one with a response or a note — and **the current state is the latest such row** (`MAX(seq)`), which is why the assumption row itself carries no `state` column: there is exactly one place a state can be, so it can never be two things at once. A row with no `assumption_id` is a session-level event (`kind` is `gate`, `override` or `budget_exceeded`): the fact that a preflight fired and which factor fired it (line 1049), the per-task override a person recorded (line 1008), a budget found exceeded (line 1050). The two table constraints say exactly that: a row is about an assumption or a session, and an assumption row always carries a state.

**No `CHECK` on any vocabulary, for migration 15's reason.** `state`, `kind`, `origin`, `evidence_source`, `uncertainty` and `response` are each a vocabulary that lives in Rust — `crate::guardrails`' enums, one stored spelling per variant, an exhaustive `match` at the single writer — and none of them gets a SQL `CHECK`, because a `CHECK` is what cost `lifecycle_events` a table rebuild for its eleventh value. `CHECK (x <> '')` is kept where a value is required: an empty spelling is a missing one, not a strange one.

**Project scope.** Migration 15's two triggers, copied exactly, on both tables. The database path comes from `Runtime` and nowhere else, and every session-keyed request goes through `SessionApi` before this store is opened, so a foreign session identifier is refused before a row could be written for it.

**Bare ids, no `REFERENCES`.** `assumption_id` and `session_id` are migration 12's rule: a pointed-at row may be trimmed, and a read that cannot resolve one reports that rather than losing the transition.

### migration 20

**`ADD COLUMN`, nullable, no `CHECK`, no index.** Migration 18's shape. Every existing row backfills to `NULL`, which is the honest reading: a session recorded before this column existed was presented somewhere Glasshouse did not write down, which is a different fact from a session recorded now with no pane. No `CHECK` on the shape (`workspace:<n>` / `surface:<n>`): the validation lives in Rust, at the one place the value is handed back to cmux (`integrations::cmux::PaneRef::parse`), so a cmux that changed its reference syntax would be met in one file rather than by a table rebuild. No index: the only reads are a session's own row and a short bounded poll after a pane is opened.

**What may write it, and from what.** `SessionStore::create`, once, from `NewSession::presentation_ref`, which only `main.rs`'s launch path fills — from the reference cmux itself printed (`cmux identify --json`), or from one a caller supplied by hand. Nothing in `session/**` interprets the string; the session abstraction stores and returns it and learns nothing else (line 762).

### migration 21

**`sessions.last_seen_commit`: how a commit is noticed without a Git hook.** Line 1149 wants a memory commit *"after a successful Git commit"*. Glasshouse installs no Git hook and will not: a repository's hooks are the user's, `core.hooksPath` can point anywhere, and a tool that writes into `.git/hooks` to learn something it can read directly has taken over a file it does not own. It does not need one. The harness hook already runs at every `TurnEnded`, and `checkpoint::git::GitPosition` already reads HEAD without spawning `git` — so *"a commit landed"* is the comparison between HEAD now and HEAD the last time this session was looked at, and this column is the second half of that comparison.

Per **session**, not per project: two sessions in one project each have their own idea of what they have seen, and a shared column would let one session's turn silently consume the other's boundary.

**`memories.extraction_trigger`: what made Glasshouse look.** Lines 1147-1151 ask for four ways to start a memory commit and line 1153 asks that the commit be recorded *"with memories produced from a code-change boundary"*. `memories.source_commit` has existed since migration 6 and answers a different question — **where the project stood** when something was learned — and `glasshouse memory extract`, run by hand, fills it from `GitPosition::detect`. So a reader inferring "this came from a code-change boundary" from a commit being present would report every hand-run extraction as one. The trigger is the fact that was missing, and it is a column rather than a derivation.

**Both in one migration.** They are one capability: the trigger vocabulary has a `git_commit` word only because `last_seen_commit` can produce it. Splitting them would create an intermediate schema version in which the word exists and nothing can ever write it.

**`ADD COLUMN`, nullable, no `CHECK`, no index.** Migration 18's shape and its reasons, unchanged. `NULL` backfills every existing row, which is the honest reading for a row written before either fact was observable. No `CHECK` on `extraction_trigger` for `FAILURE_CLASSES`' reason — the vocabulary lives in Rust, on `ExtractionTrigger`, and is pinned there by a test; a `CHECK` would cost a table rebuild per new trigger, and `memories` is the table `memories_fts` shadows and `memory_files` references. No index: nothing queries by trigger, and migration 15's closing note applies.

**What may write them.** `last_seen_commit`: `SessionStore::record_seen_commit`, from the hook path's `TurnEnded` arm, with a full object name `GitPosition::detect` read out of `.git`. `extraction_trigger`: `Extractor::store_one`, from `ExtractionTrigger::as_str`, which is `&'static str` precisely so that no runtime string — a commit hash least of all — can reach this column.

### migration 22

**Why `backend_resource` could not answer this.** `sessions.backend_resource` has held the resolved resource since its own `ADD COLUMN` above, and it stores `crate::profile::BackendResource::slug`, whose whole vocabulary is three coarse words: `native`, `direct-provider:<provider>`, and `glasshouse-gateway`. Phase 56A's unit of capacity is the **entitlement** — two Claude accounts of one vendor, each with its own credential, capacity and reset — and both of those accounts slug to the same `native`. So the one question line 1972 asks of the durable record, *which account served this session*, is the one question `backend_resource` is structurally unable to answer, and no widening of its vocabulary would help: it names a **kind** of resource, and the entitlement is an **instance**.

**What may write it, and from what.** `SessionStore::create`, once, from `NewSession::entitlement`, which only `main.rs`'s launch path fills — from `ResolvedEntitlement::name`, the `[entitlements.<name>]` table key, for the entitlement that path has already resolved and announced (`announce_entitlement`). That is the router's own winner where the router ran (`Routed::chosen`'s `Destination::entitlement`, re-resolved by name), and the one-account lookup where it did not. Nothing else writes the column and nothing derives it: a session whose serving account was never established records `NULL` rather than a guess.

**`ADD COLUMN`, nullable, no `CHECK`, no index.** Migration 20's shape and its stated rationale — validation in Rust, not in SQL — unchanged. `NULL` backfills every existing row, which is the honest reading for a session recorded before Glasshouse could observe which account served it, and it is a **different fact** from any name: `launch_profile`'s `None` draws exactly this distinction and for exactly this reason. No `CHECK`, because the set of valid values is the user's own `[entitlements]` tables — it is not a fixed vocabulary this schema could enumerate, and it changes when a person edits a configuration file rather than when Glasshouse ships. No index: the reads are a session's own row and one bounded pass over a project's sessions for `glasshouse entitlements`, and migration 15's closing note applies.

### migration 23

**Persisted, not recomputed.** `crate::routing::request::RouterAnswer::task_class` derives the class from a `TaskClassification` that lives only for the duration of one routing decision: the classification is not stored anywhere, so a reader looking at yesterday's rows has nothing to derive from. A moving average over task classes is a read of *history*, and history is exactly what is unavailable unless the class is written down at the moment it is known. `main.rs::record_routing_latency` already holds the `RouterAnswer` and already writes the row; this column is the one missing link between them.

**`ADD COLUMN`, nullable, no `CHECK`, no index.** Migration 18's shape and its reasons, unchanged. `NULL` backfills every existing row, which is the honest reading for a row written before the class was recorded — "this build named no class here", never "no class". No `CHECK`, for `FAILURE_CLASSES`' reason: the vocabulary is `crate::routing::request::TaskClass`, five variants pinned in Rust by `every_task_class_the_type_supports_is_one_the_schema_records`, and a `CHECK` would cost a table rebuild the first time a sixth class is added. No index: the one reader (`crate::routing::burn::task_class_request_rates`) is a bounded pass over a window `routing_observations_by_route_time` already serves, and migration 15's closing note applies — measure before indexing.

**What may write it.** `main.rs::record_routing_latency`, from `crate::routing::request::TaskClass::as_str`, which is `&'static str` precisely so no runtime string can reach this column. Nothing parses a relayed response body to fill it: the class comes from Glasshouse's own classification of the *request*, never from anything a provider said.

### migration 24

**Which identity, and which two facts beside it.** `sessions.id` — Glasshouse's own session id — and nothing else. Not the harness's `metadata.user_id`: carrying that would mean the relay reading a body it never reads (`crate::gateway::ingress`'s own `an_exchange_has_nowhere_to_put_a_body`), and it names an account this ledger has no business holding. Not `sessions.native_session_id` either: that column already resolves the harness-side mapping, and the Glasshouse id is the value `evaluation_observations.session_id` already keys by, so every join these columns exist for is on one value with no translation step.

The other two are facts of the *request*, filled at the one seam that holds a decoded one — `crate::gateway::translate::serve` — and they ride here so that line 2039's shadow needs no second migration: `effort_level` is the four-word ladder `crate::gateway::translate::canonical::EffortRequest::level` reduces the harness's thinking request to, and `turn_shape` is *tool-resume* when the last user message's blocks are all tool results and *prompt* otherwise. A relayed exchange, whose body is never read, records `NULL` for both: unread, not absent.

**What may write them.** `crate::gateway::session::SessionRouting::record_routing_observation`, once per exchange the gateway serves, from the id `main.rs`'s two launch doors hand it through `SessionRouting::serve_session` after the session record exists. A gateway nothing has told is a gateway serving no session, and its rows say so with `NULL` rather than an invented id. `main.rs::record_routing_latency`'s own row — written before the record exists — stays `NULL` and says why in its doc comment: it is a row about the routing decision, not about a served exchange.

**`ADD COLUMN`, nullable, no `CHECK`, no `REFERENCES`, no index.** Migration 23's shape and its reasons. `NULL` backfills every existing row, which is the honest reading for a row written before a session could be named — "this build recorded none here", never "none". No `CHECK`: `effort_level` and `turn_shape` are Rust enums pinned by tests exactly as `task_class` is, and `session_id` is an opaque identifier with no enumerable vocabulary at all. No `REFERENCES`: migration 12's rule, and a routing row must outlive the deletion of the session it names, as the evaluation rows already do. No index: the readers are bounded window passes `routing_observations_by_route_time` already serves, and migration 15's closing note applies — measure before indexing.

### migration 25

**Why a column at all, when five timestamps are already here.** Every timestamp on this table is a unix second: `dispatched_at`, `first_byte_at`, `completed_at`, and since migration 11's two late-written columns `first_token_at` and `first_tool_call_at`. At that resolution *time to first byte* and *time to first token* are zero or one on nearly every exchange — honest, and useless for the comparison lines 1347 to 1355 ask for. The producer wall is gone (the translated seam decodes what it needs); what remains is resolution, and resolution is a column decision.

**Offsets, not instants, and their zero is not `dispatched_at`.** A monotonic clock (`std::time::Instant`) is what the gateway can read at millisecond precision; a wall clock is not, and two wall readings subtracted across a clock step produce a negative "duration" that means nothing. So each column is a number of milliseconds since a `std::time::Instant` taken **immediately before the upstream request was sent** — `crate::gateway::ingress::forward` for a relayed exchange, `crate::gateway::translate::serve` for a translated one.

That zero is deliberately *not* `dispatched_at`, whose own comment in `crate::gateway::accept_loop` says it is the instant the connection was handed to `ingress::serve`, not the instant a request left for the provider. The five `*_at` columns stay, are written exactly as before, and remain this row's only absolute timestamps.

Nullable, no index: `NULL` keeps the meaning every other optional column here has — *this producer did not measure* — and backfills every row written before this migration; the readers are the same bounded window passes `routing_observations_by_route_time` already serves.

**What may write them.** `crate::gateway::session::SessionRouting::record_routing_observation`, from the four offsets `crate::gateway::ingress::Exchange` carries. A relayed exchange carries `first_byte_ms` and `completed_ms` and `NULL` for the two token offsets, exactly as it does for `first_token_at` and `first_tool_call_at`. The support-work rows `main.rs::record_extraction_observation` writes keep their seconds: that producer takes no `Instant` of its own, and inventing one from two wall readings is the defect the `CHECK` exists to refuse.

### migration 26

**Migration 7's shape, for migration 7's reason.** SQLite cannot add or drop a `CHECK`, and migration 5's `kind` column is one. Admitting a twelfth value is therefore rename, recreate, copy, drop, then recreate the index and all three triggers — exactly what migration 7 paid to admit the eleventh, and the comment there is the one to read for why the alternative does not exist.

**`path`, and its two `CHECK`s.** Repo-relative, `/`-separated, never absolute — `crate::memory::store::normalize_observed_path`'s contract, applied by the writer, for the reasons migration 17 gives about `memory_files.path`: the schema can refuse an empty string and nothing more, because `CHECK (path NOT LIKE '/%')` would miss `C:\...` and a `CHECK` forbidding `\` or `:` would reject file names that are legal on Unix.

The second `CHECK` is the biconditional the other payload columns do not have and this one can: `file_touched` is the only kind that carries a path, and a path is the only thing that kind carries. So `(kind = 'file_touched') = (path IS NOT NULL)` refuses both a `file_touched` with nothing to point at and a `turn_ended` that somehow acquired a path. `crate::events::log::read_row` would report the first as `MissingValue`; the schema is where it is cheaper to refuse it than to read it back.

**Why an event, and not a table of its own.** `crate::memory::extract::lifecycle::chunk_for_session` already reads a session's events in order, renders each with `describe`, and derives every memory's provenance range from the tail that survived the budget. A second source would need a second ordering and a second range; an event slots into the reader that exists.

This is not the noise `REPORTED_EVENTS` refuses. That list keeps `PostToolUse` out of the *lifecycle state machine*: `file_touched` is appended by the firewall subprocess that already runs on every tool call, `crate::events::LifecycleEvent::implied_state` answers `None` for it, and every `match` on that enum in this crate is exhaustive, so the compiler names each consumer that has to say so.

### migration 27

**One row per (session, path), which is what makes a renew a renew.** The primary key is `(session_id, path)`, so a session claiming a file it already holds can only ever move `renewed_at` and `expires_at` on the row it already has — line 2395's *"renew rather than create a second one"* is the table's shape and not a rule the writer has to remember. `claimed_at` is left alone by a renew, so *"since when"* and *"still wanted as of"* stay two separate facts.

**Project scope — line 2397.** Migration 15's two triggers, copied exactly. The database file is the project, so a claim written in one project is not merely filtered out of another project's reads — there is no query in another project's database that could name it — and a row carrying a foreign `project_id` is refused before it is written.

### migration 28

**Why a table and not a setting.** The field this feeds is the *first* branch `evaluate_reserve_spend` takes, outranking every other signal including the user's own override. A value that is wrong there does not degrade the reserve policy, it inverts it, at the one moment the protection matters. Two shapes were therefore refused: a proxy derived from turn counts or elapsed time (which reports "almost complete" for work that has merely been running a while — exactly the long-running work a protected reserve exists to keep serving), and a configuration value (which is sticky by nature, and a declaration that outlives the task it described re-creates the same inversion by a slower route).

What is left is a **declaration**: somebody says the task is nearly complete, on purpose, about one named session, and the statement expires. That is migration 27's claim, one column narrower.

**One row per session.** The primary key is `session_id` alone, not `(session_id, path)`: progress is a fact about the session's current task, and a session has one. Re-declaring moves `renewed_at` and `expires_at` on the row that exists; `declared_at` is left alone, so *"since when"* survives a renew, exactly as `file_claims.claimed_at` does.

**`session_id` and not a process id, and project scope.** Migration 27's reasoning verbatim: a bare id with no `REFERENCES` because a trimmed sessions row must drop the declaration rather than fail a read, no pid because a recycled pid resolving to a live declaration is precisely the inversion above, and migration 15's two project triggers because a row carrying a foreign `project_id` is refused before it is written.

## Trims: `gateway/ingress.rs` — history moved out of comments by `GH-TRIM-GATEWAY-INGRESS`, 2026-09-05

### module doc

//! One connection, from the harness's request line to the last byte of the
//! provider's response.
//!
//! # The shape of the whole thing
//!
//! Read the request head. Check the bearer token against this instance's
//! own. Append the request target to the provider's base URL, attach the
//! *provider's* credential in place of whatever the child sent, and forward
//! every other header and every body byte unchanged. Then write the
//! provider's status and headers back and move its body across a piece at a
//! time.
//!
//! That is the entire ingress, and its shortness is the design. Three things
//! are rewritten and named as such in [`forward`]; nothing else is even
//! looked at. A tool-call payload survives because no code here can tell a
//! tool-call payload from any other bytes.
//!
//! # Line 9: the ingress is not a public API
//!
//! "Require every interactive gateway ingress to be consumed through a
//! compatible installed harness launch profile" is satisfied by the token
//! rather than by a registry. The token is minted per Glasshouse instance,
//! held only in memory, and reaches exactly one place: the environment of a
//! child harness started by [`crate::profile::resolve`] for a
//! gateway-backed profile. **Possession of it therefore is the proof** that
//! a request came from such a launch — there is no other way for a process
//! to have it.
//!
//! A second mechanism — a session registry, an allow-list, a handshake —
//! would add state without adding a fact, because it would have to be keyed
//! on something the token already establishes.
//!
//! # Line 10: what may be recorded, and what may not
//!
//! [`Exchange`] is the only thing that reaches `tracing`, and it is
//! structurally incapable of carrying a body: it holds an outcome, two
//! statuses, a byte count, two names and one optional clock reading.
//! Glasshouse's logging is already off unless `GLASSHOUSE_LOG` is set — see
//! [`crate::logging`] — so "opt-in" is the existing mechanism rather than a
//! new flag.
//!
//! **The packet asked for the provider error's `error.type` and
//! `error.message` to reach the diagnostic. They deliberately do not.**
//! Extracting either means parsing the response body, which this module is
//! forbidden to do and which is a stop condition for the whole slice. The
//! status is recorded; the body is forwarded to the harness, which is the
//! thing that actually needed to read it.
//!
//! # A second thing may now be recorded: a response *header*, never a body
//!
//! Capability map line 1229's gateway half. [`forward`] reads
//! [`crate::provider::telemetry::RATE_LIMIT_HEADERS`] — the same allowlist
//! [`crate::provider::discovery`] reads on the catalogue path — off every
//! response before relaying it, and hands the result to [`serve`]'s caller
//! alongside [`Exchange`]. This is not [`Exchange`] growing a body-shaped
//! field: [`RateLimitHeaders`] is structurally the same kind of thing this
//! module already forwards unread, a handful of integers and the fixed set of
//! header *names* Glasshouse chose, never a header value stored as text and
//! never a byte of the response body. See [`mod@crate::provider::telemetry`]
//! for why a header is not the payload this module exists to be unable to
//! read, and [`mod@crate::provider::discovery`] for the seam this one
//! completes — a real inference response is the only kind of request that
//! has ever been observed to carry a token pool's own headers, and
//! `discovery` is forbidden from making one.
//!
//! # A third thing may now be recorded: when the first response byte arrived
//!
//! Capability map line 1331's gateway half. [`forward`] reads the clock the
//! instant [`Agent::run`] returns with the provider's status and headers —
//! before a byte of the body is read — and carries that reading on
//! [`Exchange`] as `first_byte_at`. It is `None` on every path that never
//! reached a provider at all: refused before a route was chosen, or the
//! provider could not be reached. Reading the clock here costs nothing this
//! module was forbidden to pay — a status and a set of headers are already
//! read to relay them, and the clock is read after they land rather than
//! after any of the body that follows, so this stays a timing read and never
//! a parse of what the bytes mean.
//!
//! # A fourth thing may now be recorded: how the stream was framed, and how it ended
//!
//! Capability map line 1364's `stream abort` and `empty completion`, under
//! the ruling recorded in `docs/product/design-decisions.md` as *"framing is
//! not content — the relay may count and timestamp what it never reads"*.
//! [`forward`] already handles the length the provider declared (it re-states
//! it on the way out), already counts the bytes it moves (`Outcome::Forwarded`
//! has carried that count since this module was written), and already learns
//! from `ureq` whether the provider's stream ended cleanly or failed short —
//! because a short read is an `io::Error` it has to handle to relay at all.
//! [`Framing`] carries those three facts, and nothing else: a declared
//! length, a relayed count, and a [`StreamEnd`]. The observer that counts is
//! [`Counted`], which sees how many bytes each `read` returned and never the
//! buffer they landed in. No byte of the body is inspected, decoded, buffered
//! beyond what forwarding already buffers, or matched against anything — the
//! boundary that stays is the one this module has always kept, and the
//! source-scan tests in `tests/gateway_failure_taxonomy.rs` hold it.
//!
//! # A fifth thing may now be recorded: the first real token and the first tool call
//!
//! Capability map lines 1331 and 1332, under the ruling recorded in
//! `docs/product/design-decisions.md` as *"first real token and first tool
//! call on the translated path — the 1331/1332 ruling"*. This module still
//! decodes nothing: [`super::translate`] already decodes every provider
//! event into its own canonical form in order to re-encode it for the
//! harness, and a translated [`Exchange`] carries the instant a qualifying
//! canonical event passed that seam, exactly as it carries `first_byte_at` —
//! a clock reading, never a byte of the response. A **relayed** exchange
//! never enters a codec, so it writes `None` for both, the same restraint
//! `Tokens` already keeps.
//!
//! # A sixth thing may now be recorded: tool rounds and repairs
//!
//! Capability map line 1334's last two quantities and line 1350, under the
//! ruling recorded in `docs/product/design-decisions.md` as *"tool rounds
//! and repairs on the translated path."* [`super::translate`] already
//! decodes the request into canonical blocks and the response into
//! canonical events in order to translate both; `tool_rounds` counts the
//! response's tool-use block starts and `repairs` counts the request's
//! `is_error: true` tool-result blocks — two more integers derived from
//! decoding this module still never does itself. `None` on every relayed
//! exchange (this module never decodes one) and on a translated exchange
//! whose request never decoded; `Some(0)` is the seam's own honest reading
//! of "looked and found none," a different fact from not looking, which is
//! why it is never conflated with `None` here.
//!
//! # A seventh thing may now be recorded: what the provider said it billed
//!
//! **The relay rule was narrowed again on 2026-09-03, by the user**
//! (`docs/product/design-decisions.md`, *Steering decisions of record* §1):
//! accurate usage and evaluation data is worth more than byte-for-byte
//! opacity, so the gateway may inspect a **supported** relayed body far
//! enough to extract structured usage and timing. A relayed exchange now
//! carries [`Tokens`], `first_token_at` and `first_tool_call_at` — the same
//! three things a translated one has carried since Phase 56, and no more.
//!
//! **This file still decodes nothing.** The reading is [`super::usage`]'s:
//! a table of JSON key spellings scanned over a sliding window of at most
//! 512 retained bytes, which is why `gateway/tests.rs`'s
//! `no_part_of_the_relay_deserializes_anything` covers that file too and
//! still passes. [`Counted`] hands it a shared borrow of the buffer
//! [`super::http::pump`] is about to write and returns exactly the `read` it
//! was given — there is no path here that can forward less, later or
//! differently because a figure was read out of the bytes on their way past.
//!
//! Four rules from the ruling decide what a row says, and each is visible in
//! [`forward`] rather than promised here:
//!
//! - **The format comes from the route's protocol slug**, which
//!   `route_for` chose from the request target alone. A slug with no entry
//!   in [`super::usage`]'s table — `gemini-generate-content` today — means
//!   nothing is looked at at all and the row says unknown.
//! - **Both counts or neither.** A provider that stated an input figure and
//!   not an output one leaves both columns `NULL`; there is nothing to put in
//!   the second that would not be invented.
//! - **Only a stream that ended where its framing said.** A `Truncated`,
//!   `Aborted` or `ClientClosed` stream records no usage, however much of it
//!   arrived first.
//! - **The two instants are observations, not estimates**, so they survive a
//!   stream that ended badly: a token that passed the seam passed it — and
//!   they are recorded only on a `text/event-stream` delivery, because an
//!   instant inside a document is a reading of the socket rather than of the
//!   provider. See [`usage::Delivery`].
//!
//! # The relay rule, narrowed and not repealed (Phase 56)
//!
//! Capability map lines 1948–1950, under the ruling recorded in
//! `docs/product/design-decisions.md` as *"codecs around one canonical
//! form; the relay rule narrowed, not repealed"*. A request whose target
//! belongs to a protocol the provider serves is relayed exactly as above —
//! [`forward`] is unchanged for it, and it never enters a codec. Only a
//! target the provider does **not** serve — the branch that used to answer
//! `404` with nothing opened upstream — is asked a second question, in
//! [`unrouted`]: does the pair table name a supported pair from the
//! target's protocol to one this provider serves? If so, the exchange is
//! [`super::translate`]'s, recorded under the pair's own name; if the pair
//! is refused, the `404` stays and its body names the pair and the reason;
//! otherwise the `404` is exactly what it was.
//!
//! **This file still decodes nothing.** The decision is made from the
//! target alone, before a byte of the body is read, and every parse lives
//! in `translate/` — the source scan in `tests/gateway_failure_taxonomy.rs`
//! that holds this file's production half free of any decoding call is
//! unchanged and still green. [`Tokens`] is the one thing a translated
//! exchange adds to [`Exchange`]: three counts the provider stated, exact
//! because that response was parsed, and `None` on every relayed exchange.

### `fn forward`

/// The three rewrites, and everything that is not one.
///
/// Rewritten, and named here so that the list can be counted:
///
/// 1. **the request target** is appended to the base URL the provider
///    declared *for the protocol that target belongs to* —
///    [`Upstream::route_for`], then [`Route::uri_for`];
/// 2. **`authorization`** is replaced by the provider's credential, attached
///    by the gateway — [`Upstream::authorization`];
/// 3. **`host`** is dropped so that the outbound layer derives the upstream's
///    own, which is the correction the next hop requires.
///
/// Not rewritten: the method, every other header, and every byte of the
/// body.
///
/// Not *forwarded*, which is a different thing from rewritten: the hop-by-hop
/// headers of [`super::http::HOP_BY_HOP`]. Those describe the connection they
/// arrived on, and this is a different connection. `content-length` is among
/// them and is re-stated below from the value the client declared, so the
/// body is framed outbound exactly as it was framed inbound.
///
/// # Which protocol, and how that is decided
///
/// **By the request target, and by nothing else.** The gateway may serve
/// Anthropic Messages, OpenAI Responses and OpenAI Chat at once, each with
/// the base URL the one configured provider declared for it, and the target
/// the harness wrote is what says which of them this request is. The
/// alternative — looking at the body to see which protocol it reads like —
/// is forbidden here twice over: it would make this module a parser of the
/// payload it exists to be unable to distinguish from any other bytes, and a
/// request whose shape was ambiguous would be *guessed* rather than placed.
///
/// A target belonging to no served protocol is answered with a `404` and
/// **nothing is opened upstream**. That is a narrowing of what this gateway
/// used to do — a single-protocol gateway appended every target to its one
/// base URL — and it is the point rather than a side effect: with several
/// base URLs available, "append it to the first one" sends a request
/// somewhere nobody asked for it to go.
///
/// # What the narrowing costs, measured rather than assumed
///
/// Real harnesses do send targets outside their own protocol, and both were
/// observed against a listener that recorded the request line:
///
/// - Claude Code 2.1.245 sends `HEAD /api/hello` before its first
///   `/v1/messages`, and carries on unaffected after a non-2xx answer to it.
/// - Codex 0.149.1 sends `GET /models?client_version=0.149.1` when it does
///   not already hold metadata for the configured model. Under this rule
///   those are refused, and Codex logs
///   `failed to refresh available models: unexpected status 404 Not
///   Found: <this refusal's message>` — twice per session, at `ERROR`
///   level and visible to the user — then completes the session normally.
///   A full live run through this gateway to OpenRouter returned its
///   answer with exactly those two refusals recorded.
///
/// So the cost is real, it is user-visible, and it is a degradation rather
/// than a breakage. It is **not** silently accepted, and the reason it is
/// not simply routed is worth stating: `/models` is a catalogue endpoint
/// that all three protocols define, and the two spellings a harness may use
/// need *different* base URLs. Codex asks for `/models`, which only resolves
/// against a base URL carrying `/v1`; Anthropic Messages is declared at a
/// root without one, so the same request routed to that protocol would reach
/// a path the provider answers `404` for anyway. Placing it therefore means
/// choosing between OpenAI Responses and OpenAI Chat for a request that
/// names neither — a tie-break invented without a concrete provider pair
/// requiring it, which is the move the capability map's pass-through lines
/// forbid.
///
/// The change, if a later phase decides the tie-break: add `/models` to
/// `crate::profile::ingress_targets`' OpenAI Responses entry.

### `fn transport_detail`

/// Why the provider could not be reached, as one of a **fixed vocabulary**.
///
/// # Not the error's own text, and that is the point
///
/// The obvious implementation is `crate::secret::redact(&err.to_string())`,
/// and it was the first one here. It is not enough.
/// [`crate::secret::redact`] removes things that *look like credentials* —
/// an `sk-` key, a `Bearer` token — and it makes no claim at all about the
/// rest of a string. `ureq` wrote that string, `ureq` never had this
/// project's rules, and a diagnostic that keeps foreign text keeps whatever
/// the next version of that crate decides to put in it. The test
/// `a_recorded_exchange_writes_a_line_with_no_secret_in_it` caught exactly
/// that: the credential was redacted and everything around it went to the
/// log verbatim.
///
/// So nothing foreign is kept. Each variant maps to a phrase written here,
/// which means a leak is not something to be careful about — it is something
/// this function has no way to express. The categories are still the ones a
/// user needs to tell apart: a refused connection, a name that does not
/// resolve, a TLS failure and a timeout have completely different fixes.
///
/// The variant *is* read from the error, so the answer is a real
/// observation and not a constant.
///
/// [`TRANSPORT_TIMEOUT_DETAIL`] is the one phrase named outside this
/// function, because `super::session`'s `failure_class` tells a timeout from
/// every other transport failure by it — capability map line 1364's
/// `timeout`. The upstream agent (`super::upstream::agent`) sets no timeout
/// today, for the reason its own doc gives, so this arm is a mapping with no
/// live producer until one is configured; the constant exists so that the
/// day one is, nothing else has to change.

## Trims: `routing/evidence/mod.rs` — history moved out of comments by `GH-TRIM-ROUTING-EVIDENCE`, 2026-09-05

Rule 3's "move history out, behind a one-line pointer" landed here for the four blocks over 20 lines in `crates/glasshouse/src/routing/evidence/mod.rs` (Phase 59, line 2053). Each subsection is the full original comment; the in-code pointer next to the trimmed version names the item below it.

### module doc

```text
//! Phase 33A — the project-local routing evidence ledger.
//!
//! An append-oriented record of what actually happened on a routed turn
//! (line 1329), stored in `routing_observations` (`crate::database` migration
//! 11), plus rolling summaries computed **on read** from those raw rows
//! (line 1335) rather than replacing them. Every summary carries its own
//! source, window, sample size, freshness and confidence (line 1339, and see
//! [`AggregateReading`]) and stays [`None`] — "unknown" — when the sample is
//! too small to support a routing decision (line 1340), never a wide error
//! bar around a guess.
//!
//! # What a gateway exchange can actually supply, and what it cannot
//!
//! [`crate::gateway::session::SessionRouting`] is this ledger's one production
//! producer this round (see [`EvidenceLedger::record`]'s callers in
//! `crate::gateway`). It sees far less of a turn than a naive reading of line
//! 1331 suggests, and the honest limits are load-bearing for which boxes this
//! package can close:
//!
//! - **`provider`, `model`, `harness`, `quota_context`, `route`: available**,
//!   but only once a launch profile has called
//!   [`crate::gateway::session::SessionRouting::bind`]. Before that, the
//!   gateway forwards bytes for a session nothing has claimed yet, and
//!   recording a provider/model pair for it would be inventing an identity
//!   the exchange does not have. `route` is the wire protocol slug
//!   (`crate::gateway::ingress::Exchange::protocol`, private to that module),
//!   not a full [`crate::harness::pairing::ServingRoute`] — the gateway module may not
//!   name `crate::harness` at all (see its own header), so a routing
//!   observation cannot carry more identity than the ingress already exposes.
//! - **`dispatched_at`: an approximation, not the true instant.** The real
//!   moment a request left for the provider lives inside
//!   `crate::gateway::ingress::forward`, which is outside this round's
//!   partition (`gateway/ingress.rs` is not in this package's `YOURS` list).
//!   What this producer stamps instead is the instant the accept loop handed
//!   the connection to `ingress::serve` — earlier than the true dispatch by
//!   however long it takes to read the request head and stream its body to
//!   the provider, which is not bounded for a coding session's full context
//!   window. Recorded as an honest upper-bound proxy, not silently corrected.
//! - **`completed_at`: accurate.** Stamped the instant `ingress::serve`
//!   returns, which is genuinely when the exchange finished — every byte of
//!   the response has been relayed and the connection is closing.
//! - **`first_byte_at`: accurate, and the one timing column this producer
//!   added after this module's own header was first written.** Stamped the
//!   instant `crate::gateway::ingress::forward` sees the provider's status
//!   and headers arrive — before a byte of the body is read, so this is a
//!   clock reading rather than a step toward the parse this module is
//!   forbidden. `None` on every exchange that never reached a provider at
//!   all, and on the transport-failure case where one was dialled but never
//!   answered.
//! - **`first_token_at`, `first_tool_call_at`: supplied by this producer only
//!   for a *translated* exchange — GH-STREAM-FIRST-EVENTS, closing 1331 and
//!   1332 for the translated path.** `crate::gateway::translate` already
//!   decodes every provider event into its own canonical form in order to
//!   re-encode it for the harness, so the instant a qualifying canonical
//!   event passes that seam is a clock reading, not a step toward the parse
//!   this module remains forbidden. Line 1332's exclusions — whitespace
//!   padding, transport keepalives, reasoning-only deltas — are checked in
//!   the canonical vocabulary itself (`translate::FirstEvents::note`), not
//!   per provider, so they cannot drift per codec. A **relayed** exchange
//!   still leaves both `NULL`: `crate::gateway::ingress::Exchange` (private
//!   to that module) is still "structurally incapable of carrying a body,"
//!   and this producer still cannot get the value wrong because it never
//!   attempts to find one on that path.
//! - **`tool_rounds`, `repairs`: supplied by this producer only for a
//!   *translated* exchange — `GH-TOOL-ROUNDS-ON-TRANSLATED`, closing 1334's
//!   last two quantities and 1350 for the translated path.** `tool_rounds` is
//!   not a turn spanning several gateway connections — the gateway still has
//!   no notion of that, and still serves one HTTP request per connection
//!   (`crate::gateway::ingress::serve`'s own "why one request per
//!   connection") — it is the number of tool-use blocks *this one exchange's
//!   response* requested, which `crate::gateway::translate` already counts
//!   while decoding that response to re-encode it. `repairs` is the number
//!   of `is_error: true` tool-result blocks *this one exchange's request*
//!   carried, counted from the same decoded request `turn_shape` already
//!   walks. Neither is a judgement of success; both are counts of blocks the
//!   protocol names as such. A **relayed** exchange still leaves both `NULL`
//!   (this producer never decodes one), and a translated exchange whose
//!   request decoded but found no error result, or whose response carried no
//!   tool-use block, writes `0` rather than `NULL` — the seam looked and
//!   found none, a different fact from not looking.
//! - **`retries`: `0`, and it is a count, not a default.** The gateway
//!   forwards each request exactly once — `crate::gateway::ingress::forward`
//!   calls `Agent::run` once, and `ureq` 3 performs no transparent retry —
//!   so every gateway row says so. A harness's own retries are separate
//!   connections and separate rows.
//! - **`failovers`: supplied.** Whether *this* exchange's outcome moved the
//!   session to another backend is decided by
//!   `crate::gateway::session::SessionRouting::observe_exchange` in the same
//!   connection thread, before the row is written, so the row can carry it:
//!   `1` for a `ChangeCause::Failover`, else `0`. A credential rotation
//!   within one provider is deliberately **not** a failover here — Phase 9I
//!   line 537 keeps the two apart, and so does this column.
//! - **`failure_class`: supplied, from framing alone.** Capability map line
//!   1364's nine-way vocabulary, [`FailureClass`], decided by
//!   `crate::gateway::session`'s `failure_class` from the status, the
//!   rate-limit headers, the byte count and how the stream ended — never
//!   from a byte of the body. `None` on a served exchange.
//! - **`input_tokens`, `output_tokens`, `cached_input_tokens`: supplied by
//!   this producer only for a *translated* exchange.** `crate::gateway::translate`
//!   decodes the canonical response anyway, so its `usage` is a sibling of
//!   something already parsed: `tokens_of` hands it to
//!   `record_routing_observation`'s `with_tokens`, and
//!   `tests/gateway_translate_evidence.rs` proves the row carries the
//!   provider's own three counts. A *relayed* exchange leaves all three
//!   `NULL`, for the same reason as the timing columns above: reading them
//!   means parsing a response body this module is forbidden to parse — and
//!   the same test proves nothing is invented for it. **`cost_micro_usd`:
//!   not supplied by this producer** (its one producer is line 1307's,
//!   `main.rs::record_entitlement_fallback`). See also the second producer
//!   below, which is not a gateway and is not forbidden the body.
//! - **`outcome`: a coarse proxy, not the user-visible outcome line 1334
//!   asks for.** This producer only records an observation when an exchange
//!   actually reached the provider (`Forwarded` or `Unreachable` — the same
//!   filter `crate::gateway::session::classify` already applies for Phase 9H
//!   and 9I), and maps a `2xx`/`3xx` forwarded status to
//!   [`Outcome::Succeeded`] and anything else reaching the provider to
//!   [`Outcome::Failed`]. That is a transport-level fact, not a statement
//!   about whether the turn actually helped the user — a `200` whose body
//!   describes a model error looks identical to this producer, because the
//!   body is exactly what it cannot read. Recorded because it is a real,
//!   non-fabricated signal and the schema's own `outcome` vocabulary
//!   includes it; the gap to a genuine user-visible verdict is named here
//!   rather than papered over.
//! - **`context_state`: always `unknown`** from this producer. The gateway
//!   has no cache-state signal of its own; the schema's `NOT NULL DEFAULT
//!   'unknown'` is exactly what makes that the honest default rather than a
//!   guess.
//!
//! # The second producer, and why it can read what the gateway cannot
//!
//! `crate::memory::extract` supplies the token columns the gateway leaves
//! `NULL`, and it is allowed to for a reason that does not weaken the rule
//! above. The gateway **relays** somebody else's request: the response body
//! is a byte stream `crate::gateway::ingress` is designed never to parse,
//! and that is unchanged. Memory extraction is the **disposable** path,
//! where Glasshouse builds the request itself and already deserializes the
//! whole reply document to find the assistant message in it — so `usage` is
//! a sibling key of something already parsed, not a new capability to read
//! payloads.
//!
//! What that producer supplies, through
//! [`crate::memory::extract::ModelCall::observation`]: `provider`, `model`,
//! `route` (the wire protocol slug, the same spelling the gateway uses), and
//! `input_tokens`, `output_tokens`, `cached_input_tokens` **when the
//! provider reported them**. What it leaves `NULL`, deliberately: every
//! timing column, `outcome`, the four turn counters, `purpose`, and
//! `cost_micro_usd` — see that type's own documentation for why filling a
//! column with the nearest available number is worse than leaving it empty.
//!
//! # [`crate::config::pairing::ObservationSource`] for `crate::config::pairing`
//!
//! [`EvidenceLedger`] implements [`crate::config::pairing::ObservationSource`],
//! replacing `NoObservations` — design decision 6. One honest gap in that
//! implementation: [`crate::harness::pairing::EvidenceKey`] is a four-part
//! identity that includes a launch profile name, and this ledger's schema has
//! nowhere to put one — the gateway that produces these rows does not see a
//! launch profile either, only a harness slug and a bound assignment (see
//! above). `ObservedEvidenceSource::observed` matches on harness, model and
//! route and **ignores launch profile**, which means observations from two
//! launch profiles that otherwise share a harness, model and route are
//! folded together. Recorded rather than hidden: the alternative was
//! inventing a launch-profile column no producer can fill, which is the same
//! mistake line 1333 exists to prevent for cost.
```

### `EffortLevel` doc

```text
/// The four-word effort ladder a translated exchange's row records —
/// migration 24's `routing_observations.effort_level`.
///
/// # Why this mirrors a type in the gateway instead of borrowing it
///
/// The value comes from
/// [`crate::gateway::translate::canonical::EffortRequest::level`], whose own
/// `EffortLevel` is the *wire* vocabulary: it exists to be spelled onto
/// OpenAI's `reasoning_effort` and to be derived from Anthropic's
/// `budget_tokens`. This one is the *stored* vocabulary, and this module may
/// not reach into `crate::gateway` — the dependency runs the other way, and
/// `crate::gateway::session` is what writes these rows. So the four words
/// are declared here and pinned against the gateway's four, exhaustively and
/// in lockstep, by `canonical`'s own
/// `every_wire_effort_level_stores_and_reads_back_as_the_same_word`: a fifth
/// variant on either side fails to compile there rather than drifting.
///
/// [`Self::from_stored`] answers [`None`] for a word this build does not
/// know, and this module's own row reader keeps that as `None` rather than an
/// error — migration 24's own doc comment has the reason, which is migration
/// 23's.
```

### `FailureClass` doc

```text
/// What kind of failure one exchange was, judged from the status line, the
/// headers, byte counts and timing alone — capability map line 1364's
/// vocabulary, and lines 1316 and 1365's separation: a rate-limit response is
/// counted apart from a transport or model failure, and cadence throttling
/// apart from a spent long-window quota.
///
/// `None` on a [`RoutingObservation`] means the exchange completed and no
/// failure was seen — a served turn — **or** that the row was written before
/// `routing_observations.failure_class` existed (`crate::database` migration
/// 18). The two are not told apart, exactly as every other nullable column on
/// this row treats a pre-migration `NULL`; [`FailureClassCounts`] keeps such
/// rows out of *served* by reading [`Outcome`] beside this.
///
/// # Stored as text with no SQL `CHECK`
///
/// The column carries no `CHECK`, for the reason
/// `crate::database::EVALUATION_KINDS` gives: a vocabulary that will grow must
/// not cost a table rebuild per value. The vocabulary lives here —
/// [`FailureClass::ALL`], [`FailureClass::as_str`], `from_stored` — and
/// `crate::database::FAILURE_CLASSES` is pinned against it by a test.
///
/// # What decides each value, and what is never read to decide it
///
/// The one place a value is chosen is `crate::gateway::session`'s
/// `failure_class`, beside `classify`. Every rule there is over a status
/// code, a rate-limit header the relay already reads in order to forward it,
/// a byte count the relay already keeps in order to relay the body, or how
/// the stream ended as its own framing said it would. **No rule reads a byte
/// of the body**: a `200` whose body describes a model error is [`None`]
/// here, because the body is exactly what the relay cannot read — the same
/// caveat [`Outcome`] already carries. The design ruling is recorded in
/// `docs/product/design-decisions.md` under *"Phase 33: framing is not
/// content"*.
```

### `FailureClassCounts` doc

```text
/// How many exchanges in one window fell into each [`FailureClass`], beside
/// the denominator they are out of — capability map line 1316's count of
/// rate-limit responses *separately from* transport or model failures, and
/// line 1365's three figures, which this type refuses to add together: there
/// is no `failures()` total here on purpose.
///
/// Counts, not rates, so unlike [`RoutingSummary`]'s aggregates they are not
/// withheld below [`MIN_SAMPLE_FOR_SUMMARY`]: two throttles out of two
/// exchanges is a true statement about two exchanges, and it is the
/// denominator printed beside it that keeps a reader from mistaking it for a
/// rate.
///
/// # Which rows count
///
/// A row is folded in only when it recorded an [`Outcome`] at all — the
/// gateway producer always does; `crate::memory::extract`'s rows never do and
/// are not gateway exchanges, so they are neither served nor failed here. A
/// row with a class is counted under it. A row with no class and a
/// [`Outcome::Succeeded`] is *served*. A row with no class and any other
/// outcome is *unclassified*: written before migration 18, or by a producer
/// that recorded a verdict without a kind — counted in the denominator so it
/// is not silently absent, and never mistaken for served.
```

## A fresh session over a cold and bloated one — designing line 1594, 2026-09-05

**The refusal ended when its producers landed, and the numbers say why it still
did not fire.** `tests/session_router.rs`'s tripwire refused 1594 because two of
the line's three defects had no producer: nothing could say *bloated*, nothing
could say *semantically poor*. Both exist now — 1534's `context quality` term
reads the estimated context size, and Phase 36's 1584/1586 facets price a
compacted or unrelated context. Yet a cold session at the bloat ceiling still
wins against a fresh start from a good checkpoint, because 1534 capped its term
at **0.1** and `BOOTSTRAP_COST_WITH_CHECKPOINT` is **−0.25**: the size penalty
cannot reach the bootstrap it is meant to outweigh. That cap was set for a
reason that is still right — *a size reading never outweighs a structural fact
about the move in front of it* — but the fact it protects is a **warm**
session's: a live prompt cache, an intact native context, a same-task affinity.

**The decision.** The cap is a property of warmth. A session past the
warm-session relevance window has no cache to lose (five minutes), no affinity
to outrank, and nothing to resume but its size; for that session `context
quality` may weigh what carrying 160,000 tokens actually costs. So the term has
two ceilings: **0.1 while warm** (1534's, unchanged) and **0.4 once cold**.
At the cold ceiling a fully bloated session totals −0.4: it loses to a fresh
start from a good checkpoint (−0.25) and still beats a fresh start from nothing
(−1.0) — which is exactly the line's *and a good checkpoint exists* clause. The
crossover is at 62.5 % of the bloat span, about 112,000 tokens. The semantic
clause needs no new weight: a cold session at 1586's compaction-noise floor
(−0.6) already loses to the same fresh start. **Coldness alone stays inert** —
the tripwire's argument that a merely idle session resumes for free is right and
its test is kept; only its stale premise is rewritten.

**What this does not do.** It does not touch a warm session's ranking (1593's
test is the bound), does not fold compactions into the size term (1586 owns
them), and does not read any new column. Package `GH-ROUTER-FRESH-OVER-BLOATED`
(Amber); evidence in `phase-37.md`; the refusal-register row for 1594 closes
with it.

## Trims: `provider/telemetry/mod.rs` — history moved out of comments by `GH-TRIM-PROVIDER-TELEMETRY`, 2026-09-05

### module doc

//! # What was measured, and what was not
//!
//! **AnyRouter, 2026-08-27, unauthenticated `GET
//! https://anyrouter.dev/api/v1/models`** — the exact endpoint
//! [`crate::provider::discovery::model_catalogue`] already requests for that
//! template — answered `200` with:
//!
//! ```text
//! ratelimit-limit: 300
//! ratelimit-policy: 300;w=60
//! x-ratelimit-limit: 300
//! x-ratelimit-tier: ip
//! x-ratelimit-window: 60
//! access-control-expose-headers: …,X-RateLimit-Limit,X-RateLimit-Remaining,
//!   X-RateLimit-Reset,X-RateLimit-Tier,X-RateLimit-Window,RateLimit-Limit,
//!   RateLimit-Policy,RateLimit-Remaining,RateLimit-Reset,Retry-After
//! ```
//!
//! Two things follow and both are in [`RATE_LIMIT_HEADERS`]. The names this
//! parser knows are the ones **that host itself names** in its CORS
//! declaration plus the IETF `RateLimit-*` field names those follow; they are
//! not a guess at what providers generally send. And the *ceiling* is what
//! arrives here while the *remaining* count does not — asserted on a
//! deliberately cache-busted request as well as a cached one — which is why
//! [`RateLimitHeaders::apply_to`] fills a limit and leaves the matching
//! remaining count [`Capacity::Unmeasured`] rather than deriving one.
//!
//! Seven other hosts Glasshouse ships templates for — OpenRouter, UnoRouter,
//! Kilo, Nous, NVIDIA, opencode-zen and z.ai — sent **no** rate-limit header
//! of any name on the same route on the same day. That is recorded in the
//! evidence ledger as the reason line 1229 closes on one provider rather than
//! on a family of them.
//!
//! # A second seam, on a different route: the provider named its own units
//!
//! **Groq, `POST /chat/completions`, 2026-08-26** — a real (free-model,
//! one-token) inference response, the only kind of request that carries this
//! seam at all — answered `200` with both halves of *two* pools, not one:
//!
//! ```text
//! x-ratelimit-limit-requests: 7000
//! x-ratelimit-limit-tokens: 6000
//! x-ratelimit-remaining-requests: 6999
//! x-ratelimit-remaining-tokens: 5991
//! x-ratelimit-reset-requests: 12.342s
//! x-ratelimit-reset-tokens: 90ms
//! ```
//!
//! Two things distinguish this from AnyRouter's set. First, the header names
//! themselves say which resource they bound — `-requests` and `-tokens` are
//! separate suffixes rather than one ambiguous `x-ratelimit-limit` — so
//! [`RateLimitHeaders`] reads the `-requests` pair into the same fields
//! AnyRouter's unsuffixed spelling fills, and the `-tokens` pair into fields
//! of their own, landing in [`crate::provider::quota::TokenBudget::combined`]
//! rather than the request [`Pool`]. Second, the reset fields are not bare
//! integers: `12.342s` and `90ms` are a duration with its unit attached, which
//! this module's own duration parser reads apart from the plain-integer-seconds
//! [`RateLimitHeaders::reset`] AnyRouter's field uses.
//!
//! **This route is the gateway's own forwarding path**, and nowhere else:
//! `crate::provider::discovery` makes catalogue and base-URL reads only, on
//! purpose, because Glasshouse must not spend a token to check a quota — see
//! `crate::gateway::ingress`'s own header capture, which reads exactly this
//! allowlist from a response the gateway was already forwarding.
//!
//! # The gateway may read a response header now — this reverses a decision
//!
//! Phase 9I line 528 held that the gateway must not parse anything in a
//! response it exists to pass through, and an earlier packet for this phase
//! read that as forbidding the header block along with the body. **That
//! overreached.** The gateway already parses the status line and header block
//! in order to forward them; the body is what it streams untouched. Reading a
//! header is not reading the payload, so `crate::gateway::ingress` now reads
//! this module's allowlist — headers only, never a byte of the body — from
//! every response it forwards. See that module for where.

### struct `ProviderUsage`

/// The response also carries `usage`, `usage_daily`, `usage_weekly`,
/// `usage_monthly` and `rate_limit.{requests,interval}`, none of which this
/// reader applies to [`CapacityState`]. `usage*` is a **cumulative all-time
/// spend counter**, a different quantity from "how much of a ceiling
/// remains" — the only shape [`Pool::remaining`] has — and folding one into
/// the other would assert a relationship the endpoint never stated,
/// especially on an account whose `limit` is `null`. `rate_limit.interval`'s
/// format was recorded only as a type (`str`), never a real value, so
/// parsing it would be guessing at units nobody confirmed. Both are a
/// decision for whoever holds a live account and a real response body to
/// make, not this package's to invent — see the report's `PROBES I NEED RUN`.

### fn `read_harness_plan`

/// `claude auth status --json` was measured on 2026-08-27 emitting eight
/// keys, of which **three identify the account holder** — an email address,
/// an organisation id and an organisation name. `design-decisions.md`'s rule
/// that a provider's response body may name the account, and must never be
/// copied whole into anything a user might share, applies with more force to
/// a harness's own account than to a provider's error text.

### struct `GatewayQuotaCache`

/// # Never resolved automatically — deliberately
///
/// This type never calls [`crate::paths::RuntimePaths::resolve`] itself.
/// `crate::gateway` has never had a project or a data directory in scope —
/// [`crate::gateway::start_if_required`] takes only launch profiles and an
/// upstream closure — and every other cache in this crate
/// ([`crate::provider::cache::ModelCache`] included) is handed an
/// already-resolved [`crate::paths::RuntimePaths`] by whatever constructed
/// [`crate::Runtime`] rather than resolving one of its own. A gateway that
/// resolved its own OS-standard data directory would also fire inside every
/// existing conformance test that runs a real accept loop, writing into
/// whichever machine happens to run `cargo test` — which is exactly why
/// `crate::gateway::Gateway::start` keeps taking no cache at all, and
/// `crate::gateway::Gateway::start_with_quota_cache` takes one only when a
/// caller explicitly supplies it. See this package's report for the caller
/// neither of those is yet: wiring a real [`crate::paths::RuntimePaths`] into
/// [`crate::gateway::start_if_required`]'s two call sites is
/// `crates/glasshouse/src/main.rs`, which this package may not edit.

## Trims: `gateway/translate/gemini/mod.rs` — history moved out of comments by `GH-TRIM-GATEWAY-GEMINI`, 2026-09-05

### module doc

//! The Google Generative Language codec: `generateContent` and
//! `streamGenerateContent` requests, responses and stream chunks, into and
//! out of [`super::canonical`].
//!
//! The fourth wire, and the first that differs from the other three in
//! **shape** rather than only in spelling. Five decisions are worth reading
//! before the code, because each is a place where a mechanical mapping would
//! have been wrong.
//!
//! # 1. The model is in the path, not in the body
//!
//! Anthropic Messages, OpenAI Chat and OpenAI Responses all post to one
//! fixed path and name the model in the document. Gemini posts to
//! `…/v1beta/models/<model>:generateContent`, so the request target *is*
//! part of the translation. [`Gemini::outbound_endpoint`] is where that
//! happens, and [`Gemini::refuse_unencodable`] refuses a model name that
//! could not address a path — a name carrying `/`, `?`, `#` or whitespace
//! would otherwise be smuggled into the request line.
//!
//! The outbound path carries **`/v1beta` itself**, and the `gemini`
//! provider template's base URL is the bare host, for the reason Anthropic
//! Messages' entry in [`super::outbound_target`] records: a request the
//! provider serves natively is **relayed byte for byte**, target included,
//! and a Gemini client's own target already starts `/v1beta`. A base URL
//! carrying the version and a relayed target carrying it too composes
//! `…/v1beta/v1beta/models/…`, which the service answers `404` for and
//! which the harness would report as a model error. One of the two has to
//! own the segment; the relay cannot, so this does.
//!
//! A streamed request goes to `:streamGenerateContent?alt=sse`. Without
//! `alt=sse` Google answers a streamed **JSON array**, not server-sent
//! events, and [`super::stream::SseReader`] would see one enormous line.
//!
//! # 2. A function call has no id, so a tool result is matched by NAME
//!
//! This is the one that decides whether a harness's tooling survives.
//! Gemini's `functionCall` carries `{name, args}` and no id at all, and its
//! `functionResponse` carries `{name, response}` — the **name** is the
//! matching key on this wire, where on the other three the id is.
//!
//! So the two directions are not symmetric, and neither of them invents a
//! mapping table:
//!
//! - **Encoding** a canonical request (the direction every supported pair
//!   uses): a [`Block::ToolResult`]'s `tool_use_id` is resolved to the
//!   `name` of the [`Block::ToolUse`] carrying that id **in the same
//!   request**. Every harness this gateway serves resends its whole
//!   conversation, so the call a result answers is right there. An id with
//!   no such block is refused by name rather than guessed at — a
//!   `functionResponse` under the wrong name runs the wrong tool's result
//!   into the model.
//! - **Decoding** a Gemini response: the harness needs *some* id to send
//!   back, and Gemini issued none, so this codec mints
//!   `gemini-call-<index>-<name>` — unique within one answer, and carrying
//!   the name it was minted from so a person reading a transcript can see
//!   what it means. It is never parsed back: the resolution above goes
//!   through the tool-use block, not through the id's spelling.
//!
//! # 3. `STOP` is not `end_turn` when the candidate is a function call
//!
//! Gemini reports `finishReason: "STOP"` for an answer that is entirely
//! function calls. A harness told `end_turn` after a tool call **stops
//! instead of running the tool**, which is the whole of capability map line
//! 1950 failing quietly. So the canonical stop reason is derived from the
//! content as well as the reason: a candidate containing any `functionCall`
//! part stops with [`StopReason::ToolUse`].
//!
//! # 4. The end-user identifier is dropped BY NAME, and it is the only one
//!
//! Gemini's request has no field for an end-user identifier. Claude Code
//! sends `metadata.user_id` on **every** request, so refusing it would
//! refuse the pair outright rather than refuse a field — and this codec's
//! whole purpose is a pair that works. It is therefore listed in
//! [`IGNORED_FIELDS`] and dropped there, exactly as `openai_chat` already
//! lists `stream_options.include_usage` and `image_url.detail`: named in the
//! table the `field_rows` view renders, never silent. It is an
//! abuse-monitoring hint that does not change the answer, which is why it is
//! the only request field this codec drops.
//!
//! # 5. The stream ends without a terminator, so the finish reason is one
//!
//! An SSE `streamGenerateContent` has no `data: [DONE]`; the socket simply
//! closes. A stream that ended early would otherwise be indistinguishable
//! from one that finished, and the harness would be handed a truncated
//! message wearing `end_turn` — the trap `openai_chat`'s `[DONE]` rule
//! exists to close. So this decoder treats **`finishReason` as the
//! terminator**: a stream that ends without one is refused by name.
//!
//! ## Which harness-side events are synthesised, and at which chunk
//!
//! Gemini's chunks are whole `GenerateContentResponse` documents, not the
//! typed start/delta/stop events the canonical vocabulary wants, so every
//! block boundary here is synthesised. Nothing is held back for it:
//!
//! | at | emitted |
//! |---|---|
//! | the **first** chunk | [`StreamEvent::MessageStart`] with `responseId` and `modelVersion` as they arrived |
//! | a chunk carrying a `text` part | [`StreamEvent::BlockStart`] (`Text`) if no text block is open, then a [`StreamEvent::BlockDelta`] with that fragment |
//! | a chunk carrying a `functionCall` part | a [`StreamEvent::BlockStop`] for whatever was open, then `BlockStart` (`ToolUse`) and one `BlockDelta` carrying the whole `args` — Gemini sends a call's arguments in one piece, so there is nothing to fragment |
//! | the chunk carrying `finishReason` / `usageMetadata` | nothing yet; both are held for the message's own delta, because a later chunk may still carry parts |
//! | the end of the stream | `BlockStop` for the open block, then [`StreamEvent::MessageDelta`] with the stop reason and usage, then [`StreamEvent::MessageStop`] |
//!
//! The first harness-side event therefore leaves on the first chunk, and a
//! text fragment leaves on the chunk that carried it. The one thing held is
//! the message's final delta, which cannot be written before the message
//! has finished by construction.

## Trims: `api/client.rs` — history moved out of comments by `GH-TRIM-API-CLIENT`, 2026-09-05

### module doc

//! The half of the control door that knocks — capability map lines 745, 746
//! and 747.
//!
//! `api serve` answers `send_message` and `interrupt` against a
//! `SessionRuntime` it owns, and has done since Phase 42. Nothing in this
//! repository ever *called* it: `UnixStream::connect` appeared nowhere in
//! `crates/glasshouse/src`, and `cli::ApiCommand` had exactly one variant,
//! `Serve`. Glasshouse could answer this door and could not knock on it, so
//! the transport that carries a person's keystrokes into a running worker
//! existed with no person on either end of it. This module is the missing
//! end.
//!
//! # Why this closes 746 rather than merely relaying
//!
//! *"Allow direct user input to an orchestrated worker without requiring the
//! orchestrator as an intermediary."* An orchestrated worker's pseudo-terminal
//! is private to the process that spawned it — `super`'s own doc comment
//! explains why nothing else can reach one — so `api serve` is unavoidably
//! the process that performs the write. What line 746 forbids is not *a*
//! process in the middle; it is **the orchestrator** in the middle. Those are
//! different things and the difference is observable:
//!
//! - `glasshouse api send` is a process a person starts from their own
//!   terminal. No agent is asked, no agent's turn is consumed, and no agent
//!   need even be running — the door serves a project, not a conversation.
//! - The door does not decide anything about the text. It is
//!   `unix::dispatch`'s two shortest arms: resolve the session,
//!   write the bytes, answer. There is no model, no prompt, and no policy on
//!   this path.
//!
//! An orchestrator relaying the same words would have to be running, would
//! spend a turn, and could reword them. None of those is true here.
//!
//! # The third verb, and why it completes line 745
//!
//! *"Allow the user to enter any orchestrated worker while it is running."*
//! Send and interrupt could put a person's words and a person's `Ctrl-C`
//! into a worker and could not show them a single character of what came
//! back, so this module shipped with the honest note that a user could type
//! into a worker blind. `glasshouse api read` is the half that was missing:
//!
//!     glasshouse api read --session <ID> [--max-bytes N]
//!
//! It is answered by `Request::RecentOutput`, which is
//! `session::api::SessionApi::recent_output` — a read of a live session's
//! scrollback tail, inside the process that owns the pty, project-scoped
//! through the same seam send and interrupt resolve through. That function
//! existed for this module's whole life with **no production caller at
//! all**; the note this section replaces is what recorded it, and this is
//! the caller.
//!
//! **What this is not.** A transparent full-terminal attach — a person's own
//! terminal handed to the worker's, keystroke for keystroke — is a different
//! thing again, and `session::attach`'s own doc comment explains why it is a
//! larger decision than a verb. What these three commands are is a person in
//! a running worker without an agent between them: words in, an interrupt,
//! and the terminal read back.
//!
//! # It says who it is, and that is the point
//!
//! Every write this module makes carries `"origin": "user"`, because a
//! process a person started from their own terminal is the one caller on this
//! door that knows a person is behind it. Until it did, the event log could
//! not tell a person's intervention from an orchestrator's message: both went
//! through `session::api::SessionApi`, which hard-wired
//! `events::MessageOrigin::Machine`, and produced rows equal field for field.
//! That was harmless while nothing human reached the door and stopped being
//! harmless the moment these three commands shipped.
//!
//! **It is attribution, not authentication.** A different program could
//! connect to the same socket and claim to be a person; nothing here or on
//! the far side tries to stop it, and nothing should be built that does. The
//! socket is already restricted to this user, so a caller that lied would be
//! lying to that user about that user — and the honest callers, which are the
//! ones that exist, stop being indistinguishable. See
//! `protocol::RequestOrigin`.
//!
//! **It never retries.** One connect, one line written, one line read. A send
//! refused by the terminal's canonical line limit
//! (`session::RuntimeError::LineTooLong`) is a refusal that *prevented* a
//! wedge; a client that retried it would be attempting to cause the wedge the
//! refusal exists to avoid.
//!
//! **It has no `--socket`.** `api serve` takes one because a server may be
//! told where to bind; a client that took one could be aimed at *another
//! project's* door, and every project-scope check on the far side is a check
//! about the session named in the request, not about which door received it.
//! Aiming is the whole attack, so the aim is not a parameter: this resolves
//! the socket from the same already-resolved [`Runtime`] every other
//! subcommand resolves, and the only way to address a different project is
//! `--scope`, which changes which project you are rather than letting one
//! project reach into another.
//!
//! # The duplicated socket path, and why it is not left to drift
//!
//! [`socket_path_for`] is a copy of `unix::socket_path_for`, which is private
//! to its own module and was not made visible here because the server is not
//! this half's to change. The copy is proven
//! against the original the only way that is worth anything —
//! `tests/worker_access.rs::the_client_finds_the_door_the_server_actually_bound`
//! starts the real `glasshouse api serve`, reads the path it announces, and
//! drives a real send through this client against both branches of the
//! computation. If the two ever disagree, every client test in that file
//! fails to connect.

### `fn read_output`

/// Show the recent terminal output of a live session in this project — map
/// line 745.
///
/// # Four answers, kept apart
///
/// The door distinguishes four things about a read, and a client that
/// flattened any two of them would hand the user a fact that is not true:
///
/// - **A live session with output** — written to standard output, verbatim
///   and with nothing added, and nothing else is written there. What a
///   worker's terminal holds is what a pipe receives.
/// - **A live session that has printed nothing yet** — `ok` with an empty
///   `output`. Said on standard error, because it is Glasshouse talking
///   rather than the worker, and it succeeds: a worker that has said nothing
///   is not a failure to read it.
/// - **A session no process is running** — the door's `not live` refusal,
///   which fails. This is the distinction the whole verb turns on:
///   `SessionApi::recent_output` refuses rather than answering `""` because
///   *"returning an empty string would be a lie the caller has no way to
///   detect"*, and a client that printed nothing for both would have told
///   that lie on the door's behalf.
/// - **No such session in this project** — the door's scoped sentence,
///   which fails. Passed through unchanged, as every error on this path is;
///   see [`call`].
///
/// `max_bytes` is optional rather than defaulted here on purpose. The door
/// owns both the default and the ceiling, so a client carrying its own copy
/// of either could drift from the door it is talking to — and the ceiling in
/// particular is not a client's to state, because a client cannot enforce
/// it.

## Trims: `commands/routing_destinations.rs` — history moved out of comments by `GH-TRIM-ROUTING-DESTINATIONS`, 2026-09-05

### `DestinationScope::Launchable`

    /// What *this* launch could actually enter: the sessions it could resume,
    /// and exactly **one** fresh destination — the profile this launch would
    /// have used anyway.
    ///
    /// # Why one profile and not all of them
    ///
    /// Phase 37 is a **session** router: lines 1593 and 1594 are *"prefer an
    /// existing relevant session"* against *"prefer a fresh session"*, and
    /// neither of them is about which launch profile a new session runs
    /// under. Offering the launch path a fresh destination per profile makes
    /// it one, and the consequence is not academic: an unadorned `glasshouse
    /// launch` moved off the implied Native profile onto a configured direct
    /// provider — a different credential, a different bill, and a pre-flight
    /// request to a provider the user had not asked for. Two existing tests
    /// caught it, and they were right.
    ///
    /// So the profile stays where it has always come from — `--profile`, or
    /// Native — and the router decides the thing it is for: whether to start
    /// that session at all, or continue one this project already has.
    /// `glasshouse route` still ranks every profile, because a person reading
    /// a diagnostic is choosing between them and a launch is not.

### `estimated_project_memory_tokens`

/// Map line 1304's project-memory component of a fresh-session cost
/// estimate: [`glasshouse::firewall::estimate::estimate_tokens`] of the real
/// text [`glasshouse::memory::inject::briefing`] would inject for `task` —
/// measuring the actual injection rather than modeling it.
///
/// Nothing has been injected yet to skip: `glasshouse route`'s ranking is a
/// diagnostic over what WOULD be sent, not a delivery, so this reads with an
/// empty already-injected set on every call rather than a session's own
/// delivery history the way the control API's own memory-selection door
/// does (`api/unix.rs::select_memory`).
///
/// `None` — never `Some(0)` — whenever nothing was actually measured: the
/// store could not be opened, `briefing` itself failed, or `briefing` found
/// nothing to inject. All three degrade to "this component was not counted",
/// never "this component counts as zero" — only
/// [`glasshouse::routing::Cost::is_free`]'s zero is a fact this build is
/// certain of.
///
/// A [`glasshouse::memory::inject::BriefingOutcome::NothingMatched`] here is
/// map line 1865's retrieval miss and is recorded as one, at the `injection`
/// scope — `glasshouse route` is a diagnostic rather than a delivery, but the
/// search it runs is the same search a real launch would run, and a search
/// this project's own `glasshouse route` invocations run is real usage.

### `destination_backend`

/// The backend a destination running `profile` would serve on, and every wire
/// protocol its provider offers.
///
/// Two returns rather than one because `Destination::with_provider_protocols`
/// is a builder step and an **empty** list is not the same as an absent one:
/// the constructor's default is the backend's own single protocol, and
/// overwriting that with an empty vector would make `ProtocolFit::Compatible`
/// unreachable and every non-native destination `Incompatible` — see
/// `routing::session`'s note on the field. `with_provider_protocols` below is
/// the one place that distinction is applied.
///
/// `recorded_model` is a recorded session's own assigned model, which is a
/// fact about that session and outranks re-deriving one from the profile.
///
/// `Cost` is the one fact that decides "premium" for the subscription-pressure
/// terms (`routing::pressure`, lines 1570–1575): a direct-provider profile
/// whose named model the user marked in that provider's `free_models` is
/// `Cost::Free`, through `ProviderConfig::cost_of` — the same rule
/// `disposable_candidates` and `gateway_upstream` already apply — and
/// everything else is `Cost::Metered`, the fail-closed value the rest of this
/// project uses when nobody has marked a model free. A native subscription
/// and the gateway are always metered here: a subscription is the premium
/// resource those lines are about, and the gateway's cost is whichever
/// upstream it is bound to, which this launch does not know yet.

### `destination_tier_ceiling`

/// **Map line 1516's missing producer**, and the reason the tier gate stops
/// being inert on the shipped binary: the highest workload tier this
/// destination's model is established to serve, as the user configured it
/// (`providers.<p>.model_ceilings`, map line 1796, or a Phase 34F capability
/// record scoped to `query`).
///
/// Read off the [`glasshouse::routing::Backend`] rather than from the
/// profile, because the backend is where the *resolved* model lives — a
/// recorded session's own assigned model outranks re-deriving one, and
/// `destination_backend` has already applied that rule. Reading the profile
/// again here would give a warm session the ceiling of the model it *would*
/// be started with rather than the one it is actually running.
///
/// `query` is `routing_destinations`' own launch context — harness, launch
/// profile, and the wire protocol `destination_backend` resolved — which is
/// map line 1482's closing half: a capability record scoped to one of those
/// axes reaches exactly the destinations it applies to, through
/// [`glasshouse::config::EffectiveConfig::model_ceiling_for`], rather than
/// staying inert to every context-bearing caller.
///
/// `None` — no ceiling established, which the router never reads as a
/// refusal — in three honest cases, none of them a guess:
///
/// - the harness picked its own model ([`AssignedModel::HarnessDefault`]),
///   so there is no model identifier to look a ceiling up by;
/// - the destination's provider is not a `[providers.*]` key at all, which
///   is every native subscription and the gateway — a ceiling is a statement
///   about a named model on a named provider, and inventing one for a
///   resource the user never configured is exactly what
///   `ProviderConfig::cost_of` refuses to do for cost;
/// - the provider is configured and this model is simply not in its map.

### `observed_provider_health`

/// **Line 1599's bridge**: what a gateway has actually observed about these
/// destinations' resources, in the shape `provider_health` reads.
///
/// A read of [`glasshouse::provider::telemetry::GatewayHealthCache`], which is
/// [`destination_capacity`]'s own cost and its sibling directory under the
/// same `--data-dir` — no network, no subprocess, no credential, and **no
/// handle kept**: `load_all` reads the files and returns owned values, so
/// nothing here is still open when this function returns (practice §65, which
/// was paid for by a database handle opened on a path nobody was asserting
/// about).
///
/// An empty pool when the cache is empty. That is the same inert `0.0`
/// contribution for every destination this path produced before the bridge
/// existed, and it is correct: an absent reading is an absent contribution,
/// never an invented one.
///
/// # Hazard 1 — identity, which is what makes this a design and not a wiring
///
/// [`glasshouse::routing::free::FreeResource`] is keyed by a
/// [`glasshouse::routing::CredentialId`]; a persisted
/// [`glasshouse::provider::telemetry::GatewayHealthReading`] carries only the
/// **rendered** `credential_label`. That rendering is not reversible —
/// `CredentialId::label` prints `provider/var` for a `SecretRef::Environment`
/// and `provider/service:account` for a `SecretRef::OsCredential`, so a parse
/// would have to guess both where the provider ends and which variant it was
/// looking at, and a guess here does not weaken the policy, it inverts it
/// (map line 1294): the router would avoid a healthy resource on another's
/// evidence.
///
/// **So nothing here parses a label.** The consumer already tells us the key
/// it will look up — `provider_health` builds
/// `FreeResource::new(destination.backend().credential().clone(),
/// destination.backend().model().label())` — and both of those are in hand
/// here, before `choose` is called. This walks the *destinations* and renders
/// each one's label with the very function the write side rendered it with
/// (`gateway::session::SessionRouting::health_readings_for` calls
/// `credential().label()`, and `model_key` is `AssignedModel::label`). The
/// match is string equality between two calls of one renderer, in the forward
/// direction only.
///
/// # GH-POOL-ALLOWANCE — the allowance half, beside the health half
///
/// This is also where `FreePool::allowance` gets a value instead of
/// answering `unknown_pool()` for every credential. For each destination's
/// provider, the same [`glasshouse::provider::resources::observed_capacity`]
/// [`destination_capacity`] already calls is asked again, from a freshly
/// gathered [`glasshouse::provider::resources::GatheredTelemetry`] — the same
/// cheap, local, no-network read `routing_destinations` performs per call,
/// never shared with it because nothing here outlives one call (Hazard 1's
/// own reasoning applies again: cheap enough to redo, too easy to get wrong
/// to smuggle across a boundary). Its own remaining-requests reading, when
/// the provider published one, becomes `FreePool::record_pool` — the
/// provider's own numbers, nothing derived. Absent that, a `pricing.toml`
/// entry for the pair, for a destination the user has not marked free, is
/// `FreePool::declare_token_priced`. Neither: `unknown_pool()`, exactly as
/// before this package.
///
/// Three things it therefore refuses to do:
///
/// - **attribute across providers.** The provider whose file a reading came
///   from must be the credential's own provider. Two providers configured
///   with the same `credential_env` variable are *"two separate allowances"*
///   (`CredentialId`'s own doc) and share nothing; the label keeps them apart
///   because the provider is part of it, and this check keeps a mislabelled
///   file from getting around that.
/// - **attribute across models.** Health is per credential *and* model —
///   `FreeResource`'s own doc says a router sharing one entry across a
///   provider's models would take every model out of service because one was
///   busy.
/// - **choose between two readings that name the same resource and disagree.**
///   A file this program wrote cannot contain those, because
///   `health_readings_for` maps over a pool already keyed by resource. A file
///   it did not write can, and it is also the shape a genuine label collision
///   would take — two distinct credentials rendering one label, which is
///   exactly the ambiguity that must not be resolved by picking. Contradictory
///   readings leave the resource unobserved.
///
/// # Hazard 2 — the time base
///
/// [`glasshouse::provider::telemetry::GatewayHealthReading::cooling_down_until`]
/// does the conversion and documents it. Both clocks are read **once**, here,
/// so every reading in one cache is placed against the same pair rather than
/// against a clock that moved between them.

## Trims: `provider/mod.rs` — history moved out of comments by `GH-TRIM-PROVIDER-MOD`, 2026-09-05

### module doc

//! # Declarations are evidence, not recollection
//!
//! Every capability a [`ProtocolSupport`] or [`Provider`] states beyond its
//! protocol and base URL is a [`crate::harness::Declared`] value, for exactly
//! the reason [`mod@crate::harness`] uses it: "nobody checked" and "verified
//! absent" are different claims, and a router deciding what a provider can be
//! trusted to do needs to be able to tell them apart.
//!
//! # What was actually established, on 2026-08-25
//!
//! Every built-in template in [`templates`] was read from a real installation
//! or the service's own endpoint list on 2026-08-25, exactly once, the same
//! way an adapter in [`mod@crate::harness`] is read from an installed binary.
//! Only OpenRouter's and LiteLLM's model-list endpoints (both a documented,
//! public `GET /models`) were established well enough to declare `Verified`;
//! every other capability nothing was actually established for is
//! `Unverified` — never filled in from what a service probably supports.
//!
//! Two sources were added on the same date, alongside the two above:
//!
//! - **NVIDIA.** `docs.api.nvidia.com/nim/reference/llm-apis` gives base
//!   `https://integrate.api.nvidia.com` with `POST /v1/chat/completions`, and
//!   NVIDIA's own `build.nvidia.com` model pages use
//!   `base_url = "https://integrate.api.nvidia.com/v1"`. No Responses
//!   endpoint was established, so [`templates`]' `nvidia` entry declares
//!   `openai-chat` only — which is also why it cannot back Codex, whose
//!   `wire_api` dropped `"chat"` in 0.149.1.
//! - **LiteLLM.** Its quick-start and `proxy/user_keys` documentation pages
//!   both use exactly `http://0.0.0.0:4000` as the client `base_url` — kept
//!   verbatim rather than "fixed" to `localhost`. Its proxy documentation
//!   also lists `GET /models - available models on server`, which is the
//!   second `Verified` model-list endpoint above.
//! - **OpenRouter serves Anthropic Messages too**, established two
//!   independent ways: an unauthenticated `POST
//!   https://openrouter.ai/api/v1/messages` answers `401`, while `POST
//!   https://openrouter.ai/api/v1/nonexistent-endpoint` under the same prefix
//!   answers `404` — the working control case that turns "the endpoint
//!   exists and wants a credential" into a finding rather than a guess. And
//!   the user's own working launcher (`~/projects/openrouter-clis/bin/claude-or`)
//!   drives real Claude Code against exactly `https://openrouter.ai/api`,
//!   its own comment explaining why: it strips `/v1` from the OpenAI base
//!   URL because Claude Code appends `/v1/messages` itself.
//!
//! # What the model-list probes established, on 2026-08-26
//!
//! Every template here shipped with `model_list_endpoint: Unverified` except
//! OpenRouter's and LiteLLM's, both of which cited a documentation page
//! rather than a response. Six live `GET <base>/models` requests were then
//! made against the exact base URLs these templates declare, unauthenticated,
//! and read for their entry counts:
//!
//! | provider | base URL | HTTP | entries |
//! |---|---|---|---|
//! | openrouter | `https://openrouter.ai/api/v1` | 200 | 417 |
//! | unorouter | `https://api.unorouter.com/v1` | 200 | 374 |
//! | anyrouter | `https://anyrouter.dev/api/v1` | 200 | 102 |
//! | kilo | `https://kilo.ai/api/openrouter` | 200 | 367 |
//! | nous | `https://inference-api.nousresearch.com/v1` | 200 | 372 |
//! | zai | `https://api.z.ai/api/paas/v4` | 401 | — |
//!
//! The five that answered `200` are the entries whose `model_list_endpoint`
//! is now `Verified`. **The promotion goes no further than that.** A
//! `GET /models` that answers `200` establishes that a model list is served
//! at that URL and nothing whatever about streaming, tool calls or reasoning,
//! so every one of those stays `Unverified` — the same discipline the
//! OpenRouter Responses entry below already documents for its own probe.
//!
//! Two of those counts are worth reading as snapshots rather than facts about
//! the service. UnoRouter answered `374` at 09:00 on 2026-08-26 and `369` an
//! hour later, re-probed independently. A catalogue that moves within the
//! hour is why every citation here names a date and why nothing downstream
//! may treat a count as stable.
//!
//! # z.ai stays `Unverified`, and the reason is the control
//!
//! **A `401` from z.ai establishes nothing about `/models`,** and the batch
//! that first promoted it said so itself without knowing: its stated control
//! was that "a host that served nothing there would have answered `404`".
//! That is exactly the right test, and it was cited from the OpenRouter
//! Responses probe rather than run against this host. Run against this host,
//! on 2026-08-26, it fails:
//!
//! - `/api/paas/v4/models` → `401`
//! - `/api/paas/v4/definitely-not-real-xyz` → `401`
//! - `/api/paas/v4/nonsense/deep/path` → `401`
//! - `/api/paas/v9/models`, a version prefix that does not exist → **`200`**,
//!   carrying the same authentication error in its body
//!
//! The service refuses every path under that prefix identically and will not
//! say whether a route exists until a credential is presented, so the `401`
//! discriminates nothing. `https://api.z.ai/totally/bogus` does answer `404`,
//! which is what made the original reasoning look sound — the `404` behaviour
//! is real, it simply lives outside the API prefix where the probe cannot use
//! it.
//!
//! The base URL is unchanged and still `unverified_support`; only the claim
//! that a model list is served at `<base>/models` is withdrawn. Establishing
//! it needs one authenticated request with the user's own key, which is a
//! free-models-only condition away and belongs to whoever spends it.
//!
//! **The transferable rule, which is this project's own and was applied to
//! the wrong subject here: a control has to be run against the host it is
//! being used to justify.** A control borrowed from another service is a
//! statement about that service.
//!
//! # Kilo and Nous have endpoints now
//!
//! Both were deliberately absent from [`templates`] until 2026-08-26 because
//! the user held a credential for each and no endpoint had been read for
//! either. The probes above are those endpoints, so both are templates now.
//!
//! **Kilo moved, and the template declares the new host.**
//! `https://kilocode.ai/api/openrouter/models` answers `308` with
//! `Location: https://kilo.ai/api/openrouter/models`. A template on the old
//! host would work only for a client that follows redirects, and
//! [`mod@crate::provider::discovery`] deliberately follows none — a redirect
//! means deciding whether to re-attach a credential to a host named at
//! runtime, which is not a decision to make silently.

### openrouter Responses entry

                // Established by empty-body `POST`s against the live service
                // on 2026-08-26, with a control: `/v1/responses`,
                // `/v1/chat/completions` and `/v1/messages` each answered
                // `400` (the route exists, the body was rejected) while
                // `/v1/definitely-not-a-real-endpoint` answered `404`.
                // Without that control a `400` would prove nothing. The
                // `/v1` is on the base URL because an OpenAI-shaped client
                // appends `/responses` itself — Codex 0.149.1 pointed at a
                // path-less base URL was observed sending exactly
                // `POST /responses`.

### gemini entry

        // Base URL and credential delivery read off Google's published
        // Generative Language API reference: `POST
        // https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`
        // with the key in an `x-goog-api-key` header (the `?key=` query form
        // is documented too and deliberately not used — a credential in a
        // URL lands in every proxy log between here and Google).

        // `GEMINI_API_KEY` is the variable Google's own SDK samples and the
        // Gemini CLI both read. `model_list_endpoint` and `usage_telemetry`
        // stay `Unverified`: no request has been made against this host from
        // this project — the `generateContent` route is documented, not
        // probed, and nothing here upgrades a document into a measurement.
        // `USAGE_ENDPOINTS` therefore gains no row.

## Trims: `profile/mod.rs` — history moved out of comments by `GH-TRIM-PROFILE-MOD`, 2026-09-05

### module doc

//! The launch-profile abstraction and its resolution into a per-launch
//! overlay.
//!
//! Three things live here, and they are deliberately not the same type:
//!
//! - A [`LaunchProfile`] is **inert configuration** — a name, a harness, a
//!   backend resource, an optional model, an optional expected protocol, an
//!   approval selection, and an optional named response preset. Nothing
//!   about it has touched a real adapter yet.
//! - A [`LaunchOverlay`] is the **ephemeral, per-launch result** of asking
//!   one [`HarnessAdapter`] whether a profile can actually be honoured. It
//!   applies to exactly one child process and is consumed by
//!   [`LaunchOverlay::apply`].
//! - [`resolve`] is the only place allowed to turn a profile's declaration
//!   into arguments or environment for a child process, and it **refuses
//!   rather than invents**: a combination the adapter does not declare comes
//!   back as a [`Refusal`], never as a best-effort substitute.
//!
//! # Why this module never imports `crate::config` or `crate::database`
//!
//! A launch profile is configuration, not project memory. It is read from
//! [`crate::config`], resolved here, and applied to one child process; none
//! of that touches the project's SQLite database, and it must not start to.
//! Only a *reference* to which profile a session ran under belongs in the
//! database — see `session/store.rs` — and a reference is not a definition.
//!
//! [`crate::provider`] and [`crate::secret`] *are* imported, because a
//! direct-provider profile cannot be resolved without knowing what the
//! provider serves and where its credential comes from. [`crate::config`]
//! still is not: the **caller** looks a configured provider up by name and
//! hands the resolved [`crate::provider::Provider`] in through
//! [`Resolution::provider`]. That keeps resolution a pure function of what it
//! was given — no file, no ambient environment, no configuration search — and
//! `harness::resolving_a_launch_profile_touches_no_files` enforces it.
//!
//! # The credential boundary
//!
//! [`resolve`] is the **only** place in Glasshouse where a
//! [`crate::secret::Secret`] exists. It is held in a local, moved into the
//! overlay's environment, and dropped there. No type in this module stores
//! one: not [`LaunchProfile`], not [`MechanismNote`], and not any
//! [`Refusal`]. A [`crate::harness::DirectProviderPlan`] cannot hold one
//! either — an adapter is handed variable *names* and never a value, so the
//! boundary is structural rather than a habit.

### `response_preset` field doc

    /// Line 353's sixth axis: the named [`response::Preset`] this profile
    /// asks for, or `None` for a profile that says nothing about
    /// communication policy.
    ///
    /// A name, not a resolved [`response::ResponseProfile`] — the same reason
    /// [`LaunchProfile::backend`]'s `DirectProvider` variant carries a
    /// provider *name* rather than a looked-up [`crate::provider::Provider`]:
    /// resolving a preset name against `response::presets()` is cheap and
    /// total, so there is nothing to gain by asking the caller to resolve it
    /// before handing the profile over, and something to lose — a
    /// `LaunchProfile` that could hold an unresolvable preset would need a
    /// second refusal path this module does not otherwise have.
    ///
    /// Consulted by `main.rs::launch_session`, which folds it into the
    /// session's [`crate::config::response::ResponseRequest`] as the
    /// `PrecedenceLayer::Session` layer when the command line named no
    /// preset of its own — an explicit `--response-preset` always wins,
    /// because a person typing one on the command line is a stronger request
    /// than a profile's standing default. See that function's own comment
    /// for why this could not become a seventh [`response::PrecedenceLayer`]:
    /// the map's line 596 fixes the chain at exactly six named layers, and
    /// that box is already closed.

### `fn install`

    /// Write this overlay's generated configuration documents into `site`,
    /// point the child at them, and return the guard that removes them again.
    ///
    /// # Why this is separate from [`resolve`] and from [`LaunchOverlay::apply`]
    ///
    /// Resolution happens **before** a session record exists, because a
    /// refusal must cost nothing — no row, no process. So at resolution time
    /// there is no session directory and no path to put in an environment
    /// variable. The adapter therefore declares a
    /// [`crate::harness::ConfigPathPlacement`] instead of a path, and this
    /// step fills it in once the caller knows where the session lives.
    ///
    /// `apply` cannot do it: it is infallible, and a write that failed there
    /// would have to be swallowed, leaving the child pointed at a document
    /// that does not exist.
    ///
    /// # Forgetting to call this fails loudly, by construction
    ///
    /// The mechanism note and the *selection* arguments — OpenCode's
    /// `--model <provider>/<model>` — are added during resolution; only the
    /// document and the variable naming it are added here. An overlay that
    /// was applied without being installed therefore starts a harness that
    /// has been told to use a provider it has never heard of, which OpenCode
    /// refuses outright ("Model not found: …") rather than silently falling
    /// back to the user's own paid account. That ordering is deliberate: the
    /// two halves are split so that the loud failure is the one that
    /// survives a mistake.
    ///
    /// # Ephemeral means ephemeral, and this is what makes it true
    ///
    /// The returned [`EphemeralConfigs`] removes every file it wrote when it
    /// drops, so a caller holding it across `session::attach` gets a document
    /// that exists for exactly the life of the child process. Dropping it
    /// early would delete a file the running harness may still re-read;
    /// dropping it late — or never — is the surprise file in somebody's
    /// state directory that the map's "temporary or Glasshouse-owned" line
    /// exists to prevent. It also registers a
    /// [`crate::shutdown::on_forced_exit`] cleanup, because the forced path
    /// calls [`std::process::exit`] and runs no destructor.
    ///
    /// An overlay with nothing to write returns a guard that owns nothing, so
    /// a caller never has to ask whether it has any.

### `GatewayPairing` doc

/// Phase 9J line 576: the native-pairing preference and corrections a
/// gateway-backed launch resolves once from configuration, then hands to the
/// gateway it points the child at — `apply_gateway` passes this to
/// [`crate::gateway::session::SessionRouting::set_pairing_preference`] beside
/// its own call to [`crate::gateway::session::SessionRouting::bind`].
///
/// A parameter on [`resolve_with_gateway`] rather than a field on
/// [`Resolution`], for the reason that function's own doc comment gives for
/// keeping `gateway` off `Resolution` too: this is a property of *this
/// gateway-backed call*, not of the profile or the adapter, and every
/// existing caller that resolves a [`Resolution`] by hand (`config`'s and
/// `onboarding`'s tests, `tests/pty_smoke.rs`, `tests/launch_overlay.rs`) can
/// go on doing so unchanged.
///
/// `preference_slug` is [`crate::config::pairing::PairingPreference::slug`]'s
/// own spelling, not that type itself: this module may not import
/// `crate::config` (see the module documentation), the same reason
/// [`gateway_upstream`]'s `free` closure answers a plain `bool` instead of a
/// `crate::config` type. [`SessionRouting::set_pairing_preference`] parses it
/// back and degrades an unrecognised spelling to
/// [`PairingPreference::Strong`], never refusing a launch over it.
///
/// [`PairingPreference::Strong`]: crate::config::pairing::PairingPreference::Strong
/// [`SessionRouting::set_pairing_preference`]: crate::gateway::session::SessionRouting::set_pairing_preference

### `fn resolve`

/// Resolve `profile` against `cx.adapter`, producing the overlay for exactly
/// one child process — or refusing, which starts nothing.
///
/// [`Resolution::acknowledged_bypass`] is the caller's answer to "has this
/// harness's blanket-bypass risk been shown to and accepted by the user",
/// read from user-level configuration only — see
/// [`crate::config::EffectiveConfig::bypass_acknowledged`].
///
/// # The one place a credential exists
///
/// A [`crate::secret::Secret`] is minted here, moved into the returned
/// overlay's environment, and dropped. It is never stored on a profile, a
/// plan, a mechanism note or a refusal — see the module documentation.
///
/// # Why automatic review depends on the backend
///
/// Claude Code's `--permission-mode auto` is decided by a **safety
/// classifier, which is itself a model call**. Pointed at the harness's own
/// backend that call is served; pointed at a gateway it is a request the
/// gateway receives and cannot answer as Anthropic would, and auto mode fails
/// closed — the session comes up with its tools blocked.
///
/// The evidence, stated at its real strength: a working multi-gateway
/// launcher on the development machine drives Claude Code through exactly the
/// four variables this module injects and deliberately does **not** select
/// auto mode, its own comment giving that reason; and Claude Code 2.1.245's
/// bundle references no separate classifier endpoint, every API path in it
/// being an ordinary one. That is a strong reading corroborated by a working
/// implementation. It is **not** a controlled experiment, and nothing here
/// should be read as one.
///
/// So the approval arm is keyed on the **backend**, not on the harness — it
/// is a property of "this approval mechanism is served by whatever the
/// harness talks to", which is equally true of a
/// [`BackendResource::DirectProvider`] and of a
/// [`BackendResource::GlasshouseGateway`]:
///
/// - [`ApprovalSelection::Default`] contributes no approval argument, exactly
///   as it already does for a harness declaring no automatic-review mode, and
///   records a [`MechanismNote`] saying so.
/// - [`ApprovalSelection::AutomaticReview`] is **refused**. A default that
///   falls back is not a request that is refused.
/// - [`ApprovalSelection::Bypass`] is unchanged, acknowledgement and all:
///   nothing about a backend relaxes that.
///
/// [`BackendResource::Native`] behaviour does not change by one byte.

### `fn resolve_with_gateway`

/// [`resolve`], for a caller that has a running local gateway to offer.
///
/// # Why this is a second entry point rather than a field on [`Resolution`]
///
/// A gateway is not a property of the profile or of the adapter: it is a
/// *process* the caller started, and only a caller that decided to start one
/// has anything to pass. Callers that never can — the configuration tests
/// that resolve a Native profile to check a lookup — keep the argument-free
/// [`resolve`] and are unaffected.
///
/// `None` here is not "no gateway configured"; it is "this call site has no
/// gateway to give". A gateway-backed profile therefore refuses with
/// [`Refusal::GatewayNotRunning`], which is the honest thing to say, rather
/// than being silently resolved against something else.
///
/// # What a gateway-backed profile resolves into
///
/// Exactly what a direct-provider profile resolves into, through the same
/// adapter method, with two substitutions: the base URL is the gateway's own
/// loopback address, and the credential written into the child is the
/// **gateway's token** rather than any provider key. That is line 2 of Phase
/// 9G in one sentence — the provider credential stays in this process, held
/// by the gateway, and the child is given something that is worthless
/// anywhere else.
///
/// Reusing [`HarnessAdapter::direct_provider_launch`] is deliberate. The
/// variables Claude Code reads are the harness's own declared knowledge, and
/// naming `ANTHROPIC_BASE_URL` here instead would put that knowledge in a
/// second place, where the two copies could disagree.
///
/// `pairing` is [`GatewayPairing::default`] for every caller that has no
/// [`crate::config::EffectiveConfig`] to resolve one from — the same
/// pre-Phase-9J-line-576 behaviour every caller other than `main.rs`'s own
/// two production sites gets today. Ignored entirely unless `gateway` is
/// `Some` and the profile's backend actually resolves through it.

### `fn resolve_checked`

/// [`resolve_with_gateway`], plus Phase 9F line 466's precondition: refuse a
/// direct-provider or gateway-backed profile before doing anything else if
/// `harness_executable` says the harness's executable is not installed and
/// usable. [`BackendResource::Native`] is unaffected — this check is never
/// even consulted for one, so a `Native` profile's behaviour cannot change by
/// so much as which branch runs.
///
/// # Why this takes the answer rather than finding it
///
/// [`resolve`] and [`resolve_with_gateway`] stay pure functions of the values
/// in [`Resolution`] — no real `PATH` lookup as a side effect of resolving a
/// profile whose caller never asked for one. That is not incidental: every
/// existing caller of those two functions (`main.rs`'s own production launch
/// path, `config`'s and `onboarding`'s tests, `tests/pty_smoke.rs`, and this
/// module's own test suite) constructs profiles naming real harnesses —
/// `Codex`, `Pi` — that are not all installed on every machine those tests
/// run on, and none of them expects a `PATH` search to happen underneath it.
/// A third, additional entry point that takes the executable check as a
/// value keeps that guarantee intact while still letting a production caller
/// opt in.
///
/// A caller that has already resolved the harness's executable — as
/// `main.rs`'s `session::select::select` already does, before any launch
/// profile is resolved — should hand back the [`crate::harness::ExecutablePresence::Usable`]
/// it already established rather than pay for a second search. A caller that
/// has not should call [`crate::harness::ExecutablePresence::detect`] itself,
/// which performs the real check this precondition asks for.
///
/// Always resolves with [`GatewayPairing::default`] — this entry point has no
/// production caller today (only this module's own tests use it), so there is
/// no resolved `EffectiveConfig` value for it to thread through yet.

### `fn apply_gateway`

/// Point one child process at the local gateway, or refuse.
///
/// The three things that make this different from a direct provider, and
/// nothing else is:
///
/// 1. the base URL is the gateway's own loopback address rather than a
///    provider's;
/// 2. the credential written into the child is the gateway's token, which is
///    already in memory, so **no [`crate::secret::Secret`] is resolved here
///    at all** — the provider's key was resolved once, at gateway start, and
///    lives in the gateway;
/// 3. no provider headers are forwarded, because the child is not talking to
///    a provider. A provider's own extra headers are the gateway's business
///    on the hop the gateway makes.
///
/// Everything else — the arguments, the environment, the credential's
/// destination variable — comes from the adapter's own declaration, so a
/// harness that changes how it is pointed at a backend changes it in one
/// place.
///
/// 4. Phase 9J line 576: `pairing` is recorded on the gateway's routing
///    state beside the assignment `Gateway::routing().bind` just made, so a
///    later failover (`crate::gateway::session::SessionRouting::observe_exchange`)
///    scores candidates against what the user actually configured.

### `fn gateway_upstream`

/// Which configured providers the local gateway may forward to: the one it
/// assigns now, and the ones a real provider failure may move a session to.
///
/// # Phase 9G refused to choose here. Phase 9H chooses, and says so.
///
/// The previous version of this function refused a configuration in which more
/// than one provider served the ingress, with a message explaining that
/// *"choosing a backend per session is sticky routing rather than something a
/// launch profile decides"*. That was right at the time and it is the line
/// this phase exists to cross.
///
/// The objection Phase 9G actually raised was to a **silent** choice: *"a
/// gateway that picked the alphabetically first of three routers would be
/// making exactly the routing decision the map defers"*. What makes the
/// choice legitimate now is not that this phase is allowed to be arbitrary —
/// it is that the choice is no longer invisible or final:
///
/// - it is **recorded** as a [`crate::routing::interactive::Assignment`] the
///   moment a profile binds a session, and reported in the launch's own
///   mechanism note (Phase 9H lines 505 and 507);
/// - the user can **pin** the session to one provider and turn automatic
///   failover off (line 518);
/// - the user can **migrate** the session to another provider at a task
///   boundary (line 511);
/// - and every change is **recorded** with its cache consequence (lines 515
///   and 516).
///
/// A choice that is announced, pinnable and reversible is a different thing
/// from a choice made behind the user's back, and the refusal was about the
/// second. The order is the order the caller presents the providers in, which
/// is the user's own configuration order; nothing here ranks providers on
/// quality, because that is Phase 9J's job and it has no evidence yet.
///
/// **This is a judgement, and it is the one thing in this batch most worth
/// disagreeing with.** The alternative — keep refusing until a launch profile
/// can name its gateway provider — is defensible and costs a field on
/// `BackendResource::GlasshouseGateway`. It was not taken because, with the
/// refusal in place, a user with two configured routers cannot start a
/// gateway-backed session at all, and every one of Phase 9H's failover lines
/// is unreachable in production by construction.
///
/// # Several credentials for one provider are several backends
///
/// Phase 9E's last line allows *"several credentials for one provider to be
/// held as a pool"*, and [`Provider::credential_env`] has always been a list.
/// The previous version took *"the first that currently resolves"* and
/// discarded the rest. Each one that resolves is now its own backend, which
/// is what makes Phase 9I line 537's rotation — *"treat a single key's
/// exhaustion as that key's limit rather than the provider's"* — something
/// the gateway can actually do rather than something it can only describe.
///
/// # One provider, every protocol it serves
///
/// Unchanged from Phase 9G, and its reasoning is unchanged with it. A provider
/// is a candidate if it serves at least one ingress protocol with a base URL,
/// and it gets a [`Route`] for **each** protocol it serves. Requiring all
/// three would refuse every real configuration — no built-in template serves
/// more than two.
///
/// Every credential is resolved here, once, at start, and moved into the
/// [`Upstream`]. Unlike [`resolve`]'s direct-provider path they do not end at
/// a child process: they stay in this process for the gateway's lifetime,
/// which is the entire point of holding them here instead.
/// `free` answers, by provider name, whether that provider has at least one
/// model the user has marked free-tier — Phase 9I line 527's marking,
/// reaching this path per line 532. `crate::profile` may not import
/// `crate::config`, where that marking actually lives, so the caller passes
/// the answer in rather than this function looking it up; `main.rs`'s own
/// wrapper is where `ProviderConfig::free_models` and this closure meet. A
/// provider `free` was never asked about — because a caller has nothing to
/// mark, not because it is somehow known metered — answers `false`, which is
/// [`Cost::Metered`]'s own fail-closed default carried one level up.

### `fn capability_probe`

/// Build the [`CapabilityProbe`] for `profile`, or say why none is possible.
///
/// # Why a `Native` or gateway-backed profile always answers `Unavailable`
///
/// A [`BackendResource::Native`] profile talks to the harness's own account
/// through a mechanism this crate never sees the credential or base URL
/// for — there is nothing here to build a request from.
///
/// A [`BackendResource::GlasshouseGateway`] profile talks to Glasshouse's
/// own local listener, not to a provider directly, and which upstream
/// provider actually answers behind it is Phase 9H's sticky-routing
/// decision — made per session, not by this profile. Probing the gateway's
/// own loopback address would only prove the gateway this process just
/// started is listening, which is not "this credential, at this base URL,
/// for this protocol, answers" in the sense line 465 asks for; it is
/// reported as unavailable rather than as a check that answers a different
/// question than the one asked.
///
/// # Why a resolvable [`BackendResource::DirectProvider`] is always available
///
/// Once a protocol and base URL can be chosen at all — the same choice
/// `apply_direct_provider` makes — [`crate::provider::discovery::ProbeTarget::BaseUrl`]
/// is always a valid target, even when the provider has no established
/// model-list endpoint. So "no check available" for a direct-provider
/// profile only ever means the combination itself could not be resolved
/// (unconfigured provider, no shared protocol, no base URL) — the same
/// conditions under which [`resolve`] would refuse for an entirely separate
/// reason, so there is nothing new for a probe to report either.
///
/// The credential is resolved the same way `apply_direct_provider` does — the
/// first declared variable that currently has a value — but unlike there, a
/// probe with none is still built: [`crate::provider::discovery`] sends no
/// credential header when given `None`, and the resulting outcome
/// ("reachable" or "unreachable" with no credential involved) is still
/// information a report can use.

### `PREFLIGHT_TIMEOUTS`

/// The timeout budget a pre-flight check runs under, and why it is
/// deliberately not [`crate::provider::discovery::ProbeTimeouts::default`].
///
/// Every existing caller of [`crate::provider::discovery::connectivity`] is
/// answering a question a keystroke just asked, and can afford the default's
/// 5/10/20 seconds because waiting *is* what the user asked for.
///
/// A pre-flight check is the opposite. Nobody asked for it, it sits between
/// `glasshouse launch` and the session, and its entire justification is the
/// qualifier capability map line 468 puts on the requirement itself: **when a
/// cheap capability check is available**. A launch that stalls twenty seconds
/// on an unreachable host has already cost more than the check could be
/// worth, so this budget is the worst delay a launch may pay — four seconds,
/// once, and only for a profile that has a check at all.
///
/// The numbers are not arbitrary. Every provider catalogue probed on
/// 2026-08-26 answered in well under a second — see
/// [`crate::provider::discovery::RESPONSE_TIMEOUT`]'s own doc — so two and a
/// half seconds is still an order of magnitude of headroom over every
/// measured healthy answer. A host that misses it is reported as "did not
/// answer", which, because a pre-flight check never refuses a launch, costs
/// the user a line of text rather than their session.

### `Preflight` doc

/// What a pre-flight check found — capability map line 468.
///
/// # There is no `Refuse` variant, and that is the ruling
///
/// The map's verb is *verify*, and the obvious reading of "verify before
/// starting" is that a failed check refuses the launch. It was considered and
/// rejected, for reasons that are about this check specifically rather than
/// about caution in general:
///
/// 1. **No outcome of this check is unambiguous evidence that the combination
///    is wrong.** [`crate::provider::discovery::ProbeTarget::BaseUrl`] — the
///    target for every provider whose model-list endpoint nobody has
///    established, which is most of them — sends `GET <base>`, and a `404` or
///    `405` from a base URL that serves no `GET` is the *healthy* answer.
///    Refusing on that would refuse correct configurations.
/// 2. **Reachability is not correctness.** Three of the twenty-two provider
///    hosts probed for Phase 9D answer identically to a real path and a
///    nonsense one, so a negative result from a single request is a claim
///    about whether the host routed at all, never about whether this
///    credential would work. Turning that into a refusal is the mistake that
///    nearly deleted a correct URL from the provider table.
/// 3. **The failure it would prevent is cheaper than the failure it would
///    cause.** A wrong combination costs one harness startup and the
///    provider's own error — the status quo. A false refusal costs the user
///    the ability to start work at all, on a path (the network) that fails
///    independently of anything they configured.
/// 4. **[`resolve`] already owns refusal, and owns it better.** It refuses
///    from declarations — deterministically, offline, with a message naming
///    what was asked for. Putting a second, network-dependent authority
///    beside it would make whether a session may start depend on a remote
///    host's mood.
///
/// So this check *reports*, and the launch proceeds. The corollary is that it
/// needs no "start anyway" key: a refusal before start would need one, and the
/// reason it would need one — that the check can be wrong about a working
/// setup — is the same reason it does not refuse.
///
/// # What each variant means the caller should do
///
/// [`Preflight::NotChecked`] and [`Preflight::Answered`] are for the log.
/// [`Preflight::CredentialRejected`] and [`Preflight::Unreachable`] are the
/// two outcomes a user can act on before the harness takes the screen, and
/// [`Preflight::warning`] returns exactly those.

### `fn preflight`

/// Run the pre-flight capability check for `profile` — capability map line
/// 468 — or report that there was none to run.
///
/// # This is the one function in this module that touches the network
///
/// [`resolve`] and [`resolve_with_gateway`] are pure functions of the values
/// in [`Resolution`], and stay that way: nothing here is called by either of
/// them, and **this runs after resolution, never before it**. That ordering
/// is not incidental — it is what makes
/// `a_capability_probe_cannot_influence_which_backend_resolve_selects` true
/// by construction on the production path rather than by inspection. A check
/// that ran first, and whose result reached `resolve`, would be a router; the
/// backend is chosen from the profile's declaration and nothing this function
/// learns can change it.
///
/// It costs nothing at all for a profile with no check available — no
/// request, no socket, no thread — which is every `Native` and every
/// gateway-backed profile, and therefore every launch that did not name a
/// direct provider. For one that does, it costs exactly one bounded HTTP
/// request; see [`PREFLIGHT_TIMEOUTS`] for the ceiling.
///
/// # The credential
///
/// The summary is built from the provider's name, the protocol slug, the URL
/// the probe requested and [`describe_probe_outcome`] — none of which is the
/// credential, and the last of which
/// [`crate::provider::discovery::ProbeOutcome::Unreachable`] deliberately
/// builds from a fixed set of phrases rather than an error's own words. It is
/// then passed through [`crate::secret::redact`] anyway, because a *base URL*
/// is user-supplied text that can carry anything and this string reaches both
/// the terminal and the log.

## Trims: gateway module docs — history moved out of comments by `GH-TRIM-GATEWAY-DOCS`, 2026-09-05

### `gateway/conformance.rs` — module doc

    //! # The properties
    //!
    //! 1. **A request body arrives byte-for-byte.** The payload carries a
    //!    `tool_use` block with nested objects and arrays, and text in several
    //!    scripts plus an emoji, so that its byte length and its character length
    //!    differ. The assertion is on bytes and on that byte length, which is
    //!    what makes it fail for a gateway that preserved *meaning* — a JSON
    //!    round-trip that changed whitespace, key order or escaping would still
    //!    parse to the same document and is exactly the regression the capability
    //!    map forbids.
    //! 2. **A provider's error reaches the harness intact, and the diagnostic
    //!    keeps only its status.** Those are two halves of one rule and they pull
    //!    in opposite directions: the harness must see the whole body, and the
    //!    log must see none of it. Both are asserted on the same exchange, so an
    //!    implementation cannot satisfy one by giving up the other.
    //! 3. **No rendering carries either secret.** Every `Debug` this module can
    //!    reach, every response byte the client was sent, and the transport
    //!    error's own detail, scanned for a planted provider credential and for a
    //!    gateway token. Asserted twice: once over the paths a single-protocol
    //!    gateway had, and once over the three-protocol ones, because a routed
    //!    exchange and a refused-before-routing one render different fields.
    //! 4. **A request reaches the base URL its own protocol declared, and no
    //!    other.** The gateway serves up to three wire protocols from one
    //!    provider, each with its own base URL, and chooses between them on the
    //!    request target alone. The load-bearing half of every assertion here is
    //!    the negative one — the *other* base URLs were never connected to —
    //!    because the implementation this replaced appended every target to a
    //!    single base URL and would pass the positive half for all three.
    //! 5. **Streaming survives on every ingress.** The Anthropic path's twin of
    //!    this lives in [`mod@super`]; a gateway that started buffering only the
    //!    two new ones would leave that test green. The fixture blocks until the
    //!    client says it has the first chunk, so a buffering implementation
    //!    cannot produce the second one at all.
    //! 6. **A target belonging to no served protocol is refused, and nothing is
    //!    opened upstream.** Claude Code sends one such target before its first
    //!    request. The assertion is on the fixtures' *connection counts*: a
    //!    gateway that opened a connection, thought better of it and answered
    //!    `404` would pass an assertion on the status and would still have sent
    //!    a request somewhere nobody asked for it to go.
    //!
    //! # Two planted values, and why the token is planted twice
    //!
    //! [`PROVIDER_CREDENTIAL`] and [`PLANTED_TOKEN`] are known strings, so
    //! `!contains` on them is a real assertion rather than a shape check.
    //!
    //! The token is planted *and* a real minted one is used, because the two
    //! answer different questions. A minted token is 64 hex characters, and
    //! `mod.rs`'s `debug_on_a_gateway_token_prints_a_fixed_marker_and_never_the_token`
    //! records what goes wrong when short fragments of one are scanned for: hex
    //! runs occur in ordinary text, so the scan reports leaks that are
    //! coincidences and the test fails at random. A test that fails at random is
    //! worth less than no test. So the minted token — held by a real
    //! [`Gateway`] that really answered a request — is scanned for whole, and the
    //! *fragment* scan runs against a planted value drawn from an alphabet that
    //! makes a coincidence impossible rather than merely unlikely.

### `gateway/conformance.rs` — `an_answered_client_sees_an_end_of_stream_with_nothing_of_its_own_left_unread`

    /// The exchange is the unreachable-provider refusal, because that is the
    /// path that hands the request body to the outbound hop — where it is
    /// dropped unread when the connection fails — and then answers `502` with
    /// the body still on the wire. The body is 32 KiB deliberately: larger than
    /// the ingress's `BufReader`, so some of it is provably still in the
    /// kernel's receive queue rather than buffered in userspace, and far smaller
    /// than a loopback receive buffer, so the client's own `write_all` completes
    /// without the ingress reading anything for it to. Nothing is timed: the
    /// channel below carries the client's "the request is written" and the join
    /// carries "the response is complete", so there is no sleep standing in for
    /// either wait.

### `gateway/http.rs` — module doc

    //! # What "byte-for-byte" honestly means
    //!
    //! A proxy terminates one connection and opens another, so *connection*
    //! framing cannot survive: `content-length` is re-derived, `transfer-encoding`
    //! is re-applied, and hop-by-hop headers belong to the hop they were written
    //! for. What survives untouched is the part that carries meaning — the
    //! method, the request target, every end-to-end header, and every byte of
    //! the body, in order.
    //!
    //! Header *names* arrive here through [`HeaderName`], which lower-cases them.
    //! That is the same normalisation HTTP/2 mandates and is semantically the
    //! identity, so it is not a rewrite in any sense a client can observe.

### `gateway/session/mod.rs` — module doc

    //! # One lock, taken briefly
    //!
    //! A connection thread calls `SessionRouting::observe_exchange` after its
    //! exchange is finished and its socket is closed, so the lock is never held
    //! across I/O. The `Upstream` it may then switch is moved by a single atomic
    //! store, and every connection thread reads its serving backend once at the
    //! top of its own exchange — so a failover can never split one request
    //! between two providers.

### `gateway/translate/canonical.rs` — module doc

    //! # What the form deliberately cannot say
    //!
    //! Every field here is one that **both** protocols with a codec can carry.
    //! A wire field with no home in this form is not dropped by the decoder that
    //! meets it — it is refused, by name, as an [`Unsupported`], and the refusal
    //! reaches the harness as a `4xx` whose body names the field. That is the
    //! whole of capability map line 1950's *"refuse the pairing by name when it
    //! cannot be kept"* at the level of one request: the form is the supported
    //! subset, and anything outside it is a named refusal rather than a silent
    //! degradation.
    //!
    //! # Tool calls are the point
    //!
    //! A harness's native tooling rides on three things surviving a round trip
    //! unchanged: the tool definitions it declares, the tool-use blocks the
    //! model answers with, and the tool-result blocks it sends back — with the
    //! **ids preserved**, because the id is how a result is matched to the call
    //! that asked for it. [`Block::ToolUse`]'s `id` is the same string on both
    //! wires: Anthropic's `tool_use.id` and OpenAI's `tool_calls[].id` are never
    //! rewritten, minted, or mapped through a table. A wrong id here runs the
    //! wrong tool, which is why the mutation on this mapping is the first one the
    //! package owes.

### `gateway/translate/mod.rs` — module doc

    //! # Codecs around one canonical form, and a table of pairs
    //!
    //! [`canonical`] is the one form. `anthropic`, `openai_chat` and
    //! `openai_responses` are the codecs, each decoding its wire into that form
    //! and encoding out of it, in both the request and the response direction
    //! and for streams. A **pair**
    //! is a decoder and an encoder meeting in the middle, and [`pairs`] is the
    //! table that lists every ordered pair of wire protocols exactly once —
    //! supported, or refused with its reason. The table is consulted by two
    //! production callers: `crate::provider::translation_available`, through
    //! which `harness::pairing::protocol_fit` classes a pairing as translated,
    //! and `super::ingress`, which answers a target the provider does not
    //! serve either by translating it or with a `404` whose body names the
    //! refused pair and the table's reason.
    //!
    //! # The relay rule, narrowed and not repealed
    //!
    //! A request whose target belongs to a protocol the provider serves is
    //! relayed byte for byte, exactly as before this module existed, and never
    //! enters a codec — `place` is asked only from the branch that used to
    //! answer `404`, and refuses a served protocol a second time on its own
    //! account. Only an unserved target with a supported pair is translated.
    //! Parsing is bounded ([`MAX_BODY_BYTES`], [`stream::MAX_EVENT_BYTES`]);
    //! streaming stays streaming, one event translated and flushed at a time;
    //! and nothing is guessed from a body's shape, because the target decided
    //! the protocol before a byte of the body was read.
    //!
    //! # Refused by name, never dropped
    //!
    //! A field a codec cannot carry is a [`TranslationRefusal`] naming the pair,
    //! the field and the reason, sent to the harness as a `4xx` in its own
    //! protocol's error shape **before anything is opened upstream**. There is
    //! no path through this module that drops a field silently: the decoders
    //! refuse unknown keys, and the handful of response fields they ignore are
    //! listed by name so the table can show them.

### `gateway/translate/openai_chat.rs` — module doc

    //! **An erroring tool result is carried, and how.** OpenAI Chat's `tool`
    //! message has no `is_error` flag. Refusing every failed tool call would
    //! make the pair unusable the first time a command exited non-zero, and
    //! dropping the flag would tell the model a failure was a success. So the
    //! flag travels in the one channel the wire has: the tool message's content
    //! begins with [`TOOL_ERROR_MARKER`] on a line of its own. The model sees a
    //! labelled failure, and the reverse decoder restores the flag exactly, so
    //! the round trip is byte-exact rather than lossy. It is recorded in this
    //! codec's table rows as a carried field, not a silent one.

### `gateway/translate/openai_responses/mod.rs` — module doc

    //! **Server-side state is refused, not simulated.** `previous_response_id`,
    //! `store: true`, background mode, stored prompts and item references all
    //! ask the provider to hold conversation state between requests. A
    //! translated upstream has no such store, and pretending otherwise would
    //! fail on the *second* request, after the first had already misled the
    //! client. Each is a named refusal — and the encoder always sends
    //! `store: false`, because the Responses API stores responses by default and
    //! the harness on the other side of a translated pair never asked for that.
    //!
    //! **An erroring tool result travels the same way as on OpenAI Chat.**
    //! `function_call_output` has no error flag, so the flag rides as
    //! [`TOOL_ERROR_MARKER`] on the output's first line — the identical
    //! convention, deliberately, so the round trip through either OpenAI wire
    //! restores `is_error` exactly.
    //!
    //! **A reasoning item that says nothing is skipped; one that says anything
    //! is refused.** Responses upstreams emit `reasoning` output items even at
    //! default settings, usually with an empty summary. An empty item carries no
    //! information, so it is ignored by name; a summary, content, or encrypted
    //! payload is model reasoning the canonical form cannot carry, and dropping
    //! *that* silently is exactly what this directory never does.
    //!
    //! One canonical field has no home on this wire at all: `stop`. The
    //! Responses API has no stop-sequence parameter, so this codec refuses a
    //! request carrying one via [`Codec::refuse_unencodable`], before anything
    //! is opened upstream, rather than letting the infallible encoder drop it.

### `gateway/upstream.rs` — module doc

    //! # One upstream, several protocols — and why not several upstreams
    //!
    //! The gateway serves more than one ingress protocol, and a provider
    //! declares a **separate base URL for each one** — see
    //! [`crate::provider::ProtocolSupport`], whose base URL is per protocol
    //! precisely because a provider may serve them at different paths. So an
    //! upstream is one provider, one credential, and a [`Route`] per protocol.
    //!
    //! The alternative shape — a set of `Upstream`s keyed by protocol — was
    //! rejected on the one property this module exists for. Each of them would
    //! need its own [`Secret`], and [`Secret`] is deliberately not `Clone` and
    //! can only be minted inside [`mod@crate::secret`], so building that set
    //! would mean either widening that module's API or resolving the same
    //! credential once per protocol. Both turn "the credential lives here and
    //! nowhere else" into "the credential lives in three places that happen to
    //! agree". One owner, several destinations, keeps the sentence true.
    //!
    //! Several *providers* is a different question, and still refused: which
    //! backend a session runs against is Phase 9H's sticky routing. See
    //! [`crate::profile::gateway_upstream`].
    //!
    //! # Why `ureq`
    //!
    //! Glasshouse has no async runtime and this phase does not add one. `ureq`
    //! is blocking, brings `rustls` rather than a system TLS stack, and — the
    //! property that actually decided it — hands back a response body as a
    //! [`Read`](std::io::Read). A body that is a reader is a body that can be
    //! moved to the harness a piece at a time, which is what "preserve streaming
    //! end-to-end" requires and what an implementation that returned `Vec<u8>`
    //! could not offer at any price.
    //!
    //! Its default features are off: `gzip` would transparently decompress a
    //! response and leave the `content-encoding` header describing something the
    //! client is no longer being sent.

### `gateway/usage.rs` — module doc

    //! - **The forwarded bytes are preserved.** This module is handed a shared
    //!   slice of a buffer `http::pump` has already read and is about to write. It
    //!   takes `&[u8]`, returns no bytes, and cannot shorten, reorder or reframe
    //!   what its caller then writes — `Extractor::feed` has no way to say
    //!   "forward less".
    //! - **Bounded, incremental, never the whole response.** [`Extractor`] holds
    //!   one window: whatever chunk it was just handed, plus at most [`CARRY`]
    //!   bytes retained from the previous one so a figure split across a read
    //!   boundary is still read. The retained part is 512 bytes; the window's
    //!   whole capacity is that plus one `pump` chunk. No accumulation, and no
    //!   growth with response length.
    //! - **Usage fields and protocol markers only.** [`Format`] is a table of
    //!   literal JSON key spellings. The scan matches those keys and reads the
    //!   integer after them; it never walks the response as a document, never
    //!   turns a byte into text, and never records anything but four integers and
    //!   two booleans.
    //! - **Nothing is persisted.** The window is overwritten as it slides and
    //!   dropped with the stream. What leaves this module is
    //!   [`Extractor::usage`]'s three counts and [`Seen`]'s two flags, which is
    //!   all `Exchange` has anywhere to put.
    //! - **An instant is observed or it is absent.** [`Delivery`] records the two
    //!   markers only on a streamed response, because a document's internal
    //!   boundaries are not observable as instants and deriving one from
    //!   `first_byte_at` would be the estimate the ruling forbids.
    //! - **Unsupported is unknown, never estimated.** [`format_for`] answers
    //!   `None` for a protocol that is not in the table — `gemini-generate-content`
    //!   today — and [`Extractor::usage`] answers `None` unless the provider
    //!   stated *both* an input and an output figure. No arithmetic anywhere in
    //!   this file derives a count from anything but digits the provider wrote.
    //!
    //! # Why a key scan rather than a parser
    //!
    //! Two reasons, and the second is the load-bearing one.
    //!
    //! A parser needs a document, and a document is the thing the ruling forbids
    //! buffering. A scan over a sliding window is the shape "bounded streaming or
    //! incremental parsing" actually permits, and it is why
    //! `gateway/tests.rs`'s `no_part_of_the_relay_deserializes_anything` still
    //! covers this file unchanged: the relay gained a reader of two dozen key
    //! spellings, not a deserializer.
    //!
    //! And a bare `"` cannot occur inside a JSON string — it would be `\"` there.
    //! So a needle that *starts* with a quote, like `"input_tokens":`, can only
    //! ever match a real object key, never text a model generated that happens to
    //! spell one. That is what makes a scan safe here rather than merely cheap,
    //! and it is why every needle in [`Format`] begins with a quote.
    //!
    //! # The one place this looks at a value rather than a key
    //!
    //! `first_token_at` means *the first real generated token*, and capability map
    //! line 1332 excludes whitespace padding from it — `translate`'s own
    //! `FirstEvents::note` refuses a text delta whose text is all whitespace. To
    //! answer the same question the same way, [`text_at`] reads forward from a
    //! text field's opening quote until it finds either a non-whitespace byte or
    //! the end of the string. It yields one boolean, *is there a real character
    //! here*; the bytes it walked are not retained, counted, classified further or
    //! passed on. That is the whole of what this module reads that is not a key,
    //! and it is stated here rather than buried because it is the one line of the
    //! ruling that needed a judgement.

## Trims: provider module docs — history moved out of comments by `GH-TRIM-PROVIDER-DOCS`, 2026-09-05

### `cache.rs` — module doc

    The user did not type it, it has a provenance and an age, and Glasshouse rewrites it on its own when
    asked to refresh. A `cargo`-style configuration file is a record of
    decisions a person made; putting four hundred model identifiers and a
    machine-written timestamp in one would make `config.toml` unreadable and
    would make a `git diff` of it meaningless.

    A cache whose age cannot be seen
    is the failure mode the line exists to prevent: a model list from three
    weeks ago looks exactly like one from three seconds ago, and only one of
    them should be acted on.

    That is deliberate and is the whole of line 3: starting
    Glasshouse with a cached catalogue must issue no request at all, and the
    surest way to guarantee that is for the loading path to be incapable of
    making one.

### `discovery/mod.rs` — module doc

    Phase 9D line 1 asks that a user be able to test a provider *before*
    enabling it for routing. The first version of that check could not make a
    request — the batch that shipped it had no HTTP client on its branch — so
    it proved what could be proven without one (the template resolves, a base
    URL exists, a credential variable is set) and said so on screen. `ureq` is
    here now, for the gateway, so the check is a request.

    for the other half of that rule, which is what makes starting Glasshouse silent.

    and its doc
    comment explains why a blocking call on the draw thread is the specific
    bug this batch existed to avoid.

    which is deliberately built from a fixed set of
    phrases rather than from an error's own words.

### `discovery/mod.rs` — `ProbeResponse` doc

    A [`ProbeOutcome`] answers "did this endpoint answer, and how". Adding a
    header list to its variants would put quota telemetry inside the type
    [`mod@crate::shell::state`] renders as a one-line connectivity result, and
    every existing caller would have to learn to ignore it.

    and note in particular that OpenRouter's `GET /api/v1/models` answers with
    a `set-cookie` header.

### `discovery/mod.rs` — `connectivity_with_headers` doc

    This module already makes a request **because a keystroke asked it to**,
    it already holds the response, and until this phase it discarded the
    headers.

    and getting there took
    a correction rather than a design from the start: an earlier packet held
    that Phase 9I line 528 — *"the gateway forwards headers without reading
    them, and a parser there would make it a reader of the payload it exists
    to pass through"* — forbade the gateway's response path outright. That
    overreached.

    carries a reading — `POST /chat/completions` — this module can never
    produce, since Glasshouse must not spend a token to check a quota.

### `pricing.rs` — module doc

    *"Allow provider price metadata to be updated independently from the
    router implementation."*

    the same directory `user_config_file` lives in — this
    is configuration a person wrote, not a machine-cached catalogue like
    `provider_cache_dir`.

    Every other `Unverified`/`Verified`
    entry in [`mod@crate::provider`] exists because this project refuses to
    guess a capability nobody established, and a shipped price nobody priced
    against a real invoice would be exactly that guess — worse, because a
    silently-wrong shipped price is harder to notice than an absent one.

    This module only builds the table; the honesty rule it exists to serve is
    enforced at the consumer,
    `routing::session::expected_marginal_cost`.

    — the same `known`/`unknown` idiom
    `routing::session::AffinityFacet` already applies to every other scored
    signal that may or may not have arrived.

    Parsing additionally bounds the document size and
    range-checks every number before it is admitted, so a `1e308` or a
    negative entry cannot reach a score as anything but a parse failure for
    that document.

### `registry.rs` — module doc

    which is the map's own fixed requirement for this
    phase — subscriptions, metered keys, and local inference are normalized
    into one list without being told apart, and told apart is exactly what a
    `BackendResource::DirectProvider { provider: "ollama" }` and a
    `BackendResource::DirectProvider { provider: "openrouter" }` are not
    today: both are "a direct provider" and nothing distinguishes the one
    that cannot run out of money from the one that can.

    (a rolling-window
    reset time, a spent balance, a request count) — that is Phase 32B, which
    does not exist yet;

### `registry.rs` — `registry` doc

    This lists what Glasshouse can describe, not what a user has configured
    — a template with no credential is still a resource *kind* the registry
    knows about, the same way [`crate::provider::templates`] itself lists
    providers nobody has necessarily set up.

### `resources/mod.rs` — module doc

    Phase 32 built [`mod@crate::provider::registry`] and recorded, in its own
    evidence ledger, that `registry()` had no production caller: *"Nothing in
    the shipped binary currently prints 'here is everything Glasshouse can
    describe' to a user."* Phase 32A built [`mod@crate::provider::quota`] and
    recorded the same limit one layer down — the launch path reads exactly one
    projection out of the capacity model, its quota *shape*, and every pool,
    window and rate ceiling below that was proven only by tests.

    Both were right to say so, and both were pointing at the same missing
    thing: a surface that reads the model. This is it, and it is

    Neither is
    enforced by this module's care.

    — because it is a local process invocation costing about
    a quarter of a second and no quota —

### `resources/mod.rs` — `HARNESS_STATUS_ARGS` doc

    Practice §5's rule — *check a declaration against the use, not the claim*
    — and the reason this project has been wrong about a harness's declared
    surface five times. Checked on 2026-08-27 against the binaries installed
    on this machine.

    `--json` is listed in
    `claude auth status --help` as the **default** output, which is as
    stable a declaration as a CLI gives.

    `codex doctor --json` exists and is stamped
    `"schemaVersion": 1`, so it is genuinely stable and machine-readable.
    It carries **no** usage, quota, limit, credit, remaining, reset, plan,
    window or balance field: twenty-three checks about installation, auth
    configuration, network reachability and disk. It is not a usage
    interface.

    the `agy` binary's `--help` lists no status or usage
    subcommand at all.

    and
    the evidence ledger says so rather than letting the list imply more.

    They should live on the adapter. [`IntegrationId::executable_candidates`]
    argues exactly this about the executable *name* — *"keeping a second copy
    here would be a second place for it to be wrong, and the two would
    drift"* — and a status command is the same kind of fact.

    `crates/glasshouse/src/harness/**` is outside this package's
    partition; see the report for the two-line trait method this wants to be.

### `resources/mod.rs` — `gather_gateway_quota` doc

    exactly as they do to a probed one — there is no second code path for
    this source to disagree with the first through.

    **Not yet called from `glasshouse resources`.** The caller this
    method exists for is `main.rs::resources_report`, which this
    package's `FORBIDDEN FILES` does not let it reach — see the report
    for the one line that call site needs. Tests exercise this method
    directly, which is what proves the model side of the bridge without
    claiming the production reach it does not yet have (practice §35).

### `resources/mod.rs` — `observed_capacity` doc

    and applying them in this order
    means the *stale* case behaves the same way: a fresh manual entry never
    displaces a provider's own word, only fills a gap it left.

    and the worst case is the
    state [`CapacityState::for_resource`] built with nothing read at all.
    There is no path through this
    function that yields an error for a caller to fail a session on.

### `resources/mod.rs` — `authorize_probe` doc

    The same [`GatewayQuotaCache`] reading `resources_report` already folds
    into `telemetry` before this runs (`GatheredTelemetry::gather_gateway_quota`),
    through the exact production path every other number in the report reads
    it through.

    [`GatewayQuotaCache`]'s own `path_for` and
    [`GatheredTelemetry::with_provider_headers`] are both keyed by provider
    alone.

    the
    first because there is nothing to compare a cost against and
    [`probe_provider`] already reports it as not configured; the other two
    because "unknown" and "this provider is not limited by a request count at
    all" both mean there is no remainder to spend down.

### `quota/mod.rs` — module doc

    the same way
    [`mod@crate::provider::registry`] is a derived view over
    [`crate::provider::templates`]. Putting it beside the type it describes
    keeps the whole quota story in one module tree and needs no new
    top-level module registration.

    Phase 32 established that the four quota *shapes* are not the same shape:
    a subscription has a rolling window, a metered key has a balance, a free
    pool has a request count, and local inference has neither. A model that
    flattened them into one "percent remaining" number would satisfy the word
    "unified" and break the requirement in the same motion.

    so it can
    never be what is left after the raw reading was thrown away.

    The map's own rule is that Glasshouse must never invent exact token
    balances for opaque subscriptions, and a model that reports a number it
    cannot know is worse than one that says `unknown`. But collapsing the four
    jobs "unknown" does is how a later phase talks itself into filling one in.

    A local inference
    server has no credit balance; asking is a category error.

    A first-party subscription's remaining
    tokens. ... and that is the
    map's rule expressed as a state rather than as a comment.

    Every one of these is waiting on
    Phase 32B, which is where telemetry lives and which does not exist yet.

    That is Phase 32B's job. Consequently **every pool of every
    [`CapacityState`] the shipped binary constructs today is one of the four
    unknown states** — which is the honest answer, and is stated in the
    evidence ledger rather than hidden behind a type that looks populated.

### `quota/mod.rs` — `TelemetryClass` doc

    They are not the same question and collapsing them
    loses one: two numbers can arrive by the same mechanism and be different
    claims (a provider's own `RateLimit-Limit` header and a ceiling Glasshouse
    inferred from watching that header change), and two numbers can be the
    same kind of claim through different mechanisms (a provider endpoint and a
    harness's own status output are both the account holder speaking about
    itself).

    A reading
    that does not exist cannot carry a source or a class, and inventing an
    `Unknown` variant would mean constructing a [`Reading`] for a measurement
    nobody took.

### `quota/mod.rs` — `Percentage` doc

    [`NormalizedCapacity::percent`] used to answer a bare `u8`. A bare `u8`
    makes "check the source before you render this" a rule every caller has to
    remember, and line 1234 — *never label an inferred subscription percentage
    as exact* — is not a rule this project leaves to memory.

### `quota/mod.rs` — `KnownPlan` doc

    Line 1233 asks that a user be able to *enter a known plan* when the
    provider exposes no usable telemetry, and line 1231 asks that native
    harness status be read when a stable machine-readable interface exists.
    Those are the same fact arriving by two different origins — the user
    remembering their subscription tier, and the harness stating it.

    It is what a later phase would need in order to look
    a published allowance up, and it is what a resource view can honestly
    state today.

### `quota/mod.rs` — `effective` doc

    [`RemainingCapacityScore::fraction`]
    and [`RemainingCapacityScore::routing_fraction`] still answer exactly
    what they answered before this was called.

    per the design
    decision's own instruction not to fabricate a reset when none is
    known. See [`CapacityState::seconds_until_reset`] for where a caller
    gets this number.

    line 1264's "far away
    relative to the remaining capacity" case, where the effective value
    stays at the (already conservative) routing fraction rather than
    being boosted.

### `quota/mod.rs` — `remaining_capacity_score` doc

    **Checked against today's own telemetry reader rather than assumed
    (practice §23).** `crate::provider::telemetry::RateLimitHeaders::apply_to`
    currently fills a pool's limit and the per-minute ceiling from the
    *same* header reading in one call, so the two agree today for every
    live host this build has observed — this widening changes nothing
    for them. It matters the moment the two readings arrive from
    different observations (a stale general limit beside a fresher
    per-minute one, or a user-configured override on one but not the
    other).

    See the evidence ledger
    for whether this closes line 1267 or only partially does.

### `quota/mod.rs` — `ReserveContext::tier` doc

    Capability map line 1289 says *"when their capability requirement
    justifies it"*, and Phase 35 now has a literal capability set. It must
    not be plumbed here, and the reason is in its own doc comment.

    This decision is entirely about whether to spend a stronger model's
    protected quota. wiring it in would let `run the tests and paste the output`
    spend protected premium reserve because it needs a shell, while a
    genuinely demanding pure-reasoning task, needing none of the three,
    would not.

### `quota/mod.rs` — `ReserveContext::task_nearly_complete` doc

    written
    by the `glasshouse task-progress` verb, exactly as
    [`Self::user_override`] is.

    Glasshouse's own event
    vocabulary ([`crate::events::LifecycleEvent`]) is deliberately binary
    and retrospective — a turn started, a turn ended and how, the harness
    is waiting for the user, the process exited — and two of its variants
    carry doc comments saying in as many words that they are *not*
    statements about the session's work. The one path that reaches
    [`evaluate_reserve_spend`] runs *after* `TurnEnded { Completed }`, so
    the only completion fact available there is that the turn is already
    over.

    A turn count or an elapsed-time threshold would compile and would look
    like a producer. It would also be wrong in the one situation this line
    exists to protect: it would report "almost complete" for a task that
    had merely been running a while. That is why a declaration is the
    producer and a proxy is not, and why the declaration expires: a
    statement that outlived the task it described would invert the policy
    by the slower route.

### `quota/mod.rs` — `evaluate_reserve_spend` doc

    Both lines' operative word is *solely*: the guard stops a
    threshold being the whole reason work moves, and a declaration is a
    second reason, contributed by the only party that knows.

    An explicit user override is a statement about
    *this* task or session that the user made on purpose; it
    protects work already in flight regardless of what either party
    intended about reserve, so "the user
    overrode this" can only ever be true of a session the user named.

    the
    bands below `Reserve` are the only ones this function ever has
    an opinion about.

    or
    explicitly known and not imminent

## Trims: memory and session module docs — history moved out of comments by `GH-TRIM-MEMORY-SESSION-DOCS`, 2026-09-05

### `memory/policy.rs` — module doc

    //! What the memory table refuses to hold.
    //!
    //! Phase 20 states four properties of durable project memory as prohibitions:
    //! no raw conversation filler, no temporary step-by-step plans unless they
    //! became an accepted constraint or decision, no obvious source-code facts,
    //! and a preference for information whose rediscovery would be expensive.
    //!
    //! # Only two of the four are enforced here, deliberately
    //!
    //! The first two are *mechanically decidable* from the text itself, so they
    //! are enforced at the one place every memory must pass through:
    //! [`crate::memory::MemoryStore::record`] refuses them, and the refusal is a
    //! typed error rather than a silent drop.
    //!
    //! The other two are not decidable here and are not faked. Whether a statement
    //! is an "obvious source-code fact", or whether rediscovering it "would
    //! require significant exploration", is a judgment about the project that only
    //! the producer of the memory can make — Phase 21's extractor, or a person. A
    //! keyword heuristic pretending to make that call would refuse real memories
    //! and admit fake ones, and would produce a test that passed for the wrong
    //! reason. This module's job is to be a floor that cannot be argued with, not
    //! a classifier.
    //!
    //! So [`MemoryRefusal`] is a **closed, conservative** guard. It refuses text
    //! that is *nothing but* an acknowledgement, and text that is *unambiguously*
    //! an ordered plan. Anything it is unsure about, it admits — the cost of a
    //! wrongly-admitted memory is one bad search result, and the cost of a wrongly
    //! refused one is knowledge that is gone.

### `memory/policy.rs` — half_life_days

    /// How many days it takes a memory of this authority to decay halfway from
    /// full weight to [`RETRIEVAL_WEIGHT_FLOOR`] — Phase 21D line 898's *"age
    /// never overrides authority"*, made into policy instead of a magic number
    /// inside the ranker.
    ///
    /// A full `match` on every class, never a lookup table with a default: a
    /// class added to [`MemoryAuthority`] must be given an explicit rate here
    /// rather than silently inheriting one meant for something else.
    ///
    /// - [`MemoryAuthority::Invariant`] has no half-life at all — see
    ///   [`retrieval_weight`], which returns full weight for it before this is
    ///   ever consulted. Line 898: *"do not make age alone invalidate a genuine
    ///   invariant."*
    /// - [`MemoryAuthority::Constraint`] decays slowly: it is still a currently
    ///   binding limit, and a limit does not stop applying merely because time
    ///   passed.
    /// - [`MemoryAuthority::Decision`] and unclassified memories decay at the
    ///   map's own "ordinary decision" rate (line 899).
    /// - [`MemoryAuthority::Preference`], [`MemoryAuthority::Hypothesis`] and
    ///   [`MemoryAuthority::Idea`] decay fastest (line 900) — they were never
    ///   binding, so staleness costs nothing to make visible quickly.
    /// - [`MemoryAuthority::Historical`] decays at the ordinary rate: it already
    ///   explains rather than directs, so there is no faster-decaying class
    ///   below it that the map names, and treating it as ordinary is the
    ///   conservative middle rather than an invented rule.

### `memory/policy.rs` — phase_penalty

    /// Phase 21F lines 931 and 933's project-phase signal, folded into decay as
    /// an extra multiplier rather than a second independent check.
    ///
    /// This module does not read the project's *current* phase or architecture
    /// — that is line 932, and map lines 828/829/862 already settled that a
    /// storage-layer heuristic for "does this still match the repository"
    /// refuses real memories and admits fake ones. What it can honestly do with
    /// what a memory itself recorded is this: a decision made in an earlier,
    /// more provisional phase and never rechecked since is preferred less than
    /// one that has been checked at all, whatever phase that check happened
    /// in — which is why reaffirming ([`super::store::MemoryStore::reaffirm`])
    /// clears the penalty entirely rather than scaling it down. [`ProjectPhase`]
    /// is a fixed, ordered vocabulary (Phase 21B), not a live reading of the
    /// repository, so ranking by it is ranking by what was stored, not by an
    /// invented judgement about where the project is now.
    ///
    /// [`ProjectPhase::Prototype`] is line 933's "exploratory session," by that
    /// variant's own doc comment. [`ProjectPhase::Alpha`] gets the milder
    /// version line 931 also asks for. [`ProjectPhase::Beta`],
    /// [`ProjectPhase::Production`], [`ProjectPhase::Migration`] and unrecorded
    /// phase are not judged at all — a decision with no evidence it was made
    /// early is not assumed to be provisional.

### `memory/policy.rs` — retrieval_weight

    /// The retrieval-weight multiplier a memory of this authority, age,
    /// validation history and originating project phase should carry — Phase
    /// 21D and Phase 21F.
    ///
    /// `1.0` means no decay at all. Applied by [`super::store::MemoryStore::search`]
    /// to the raw BM25 score of every current result, so an old, low-authority
    /// memory that matches the query text well still ranks below a fresh,
    /// high-authority memory that matches it poorly — see that method's own
    /// documentation for why the multiplier is applied there and not baked into
    /// the SQL.
    ///
    /// # Why the reference point is `last_validated_at.unwrap_or(created_at)`
    ///
    /// Line 901: *"allow recently reaffirmed memories to regain retrieval weight
    /// without changing their original creation timestamp."* Reaffirming
    /// ([`super::store::MemoryStore::reaffirm`]) writes only
    /// [`super::store::MemoryRecord::last_validated_at`], so decay has to measure
    /// age from whichever of the two is more recent information about when this
    /// memory was last known to be true — and a memory that has never been
    /// reaffirmed has no more recent information than its creation. This is also
    /// line 899's *"when they have not been reaffirmed"*: a memory that has been
    /// keeps its full weight for a fresh interval measured from the reaffirming,
    /// not from when it was first written down. Line 931 rides the same
    /// mechanism: a validated memory always has a reference point at least as
    /// recent as an otherwise-identical unvalidated one, so it can never rank
    /// below it at equal relevance and authority.
    ///
    /// # Why age never invalidates an invariant
    ///
    /// Line 898. Checked before anything else, and unconditionally: no age, no
    /// validation history, no project phase, and no half-life computation can
    /// move an invariant's weight away from `1.0`.
    ///
    /// # Why the phase penalty multiplies the decay term, not the final weight
    ///
    /// [`phase_penalty`] is folded in *before* [`RETRIEVAL_WEIGHT_FLOOR`] is
    /// applied, so the floor's own guarantee — decay demotes, it never makes a
    /// memory unfindable — still holds for a memory the phase penalty also
    /// applies to.

### `memory/store.rs` — module doc

    //! The `memories` table, and the only way to read or write it.
    //!
    //! # Project isolation
    //!
    //! Enforced in three independent places, for the reason each is different:
    //!
    //! - **At the file**, because [`ProjectMemory::open`] goes through
    //!   `database::open`, which derives the path from the runtime and
    //!   refuses a database bound to another project outright.
    //! - **At the row**, by the two SQLite triggers migration 4 creates. A query
    //!   can forget to filter by `project_id`; a `BEFORE INSERT` / `BEFORE UPDATE`
    //!   guard cannot be forgotten, and it holds against any writer, including one
    //!   written later by someone who never read this module.
    //! - **At the read boundary**, by [`MemoryStore::get`], which compares the
    //!   stored identifier against the active project before handing a record
    //!   back.
    //!
    //! The third is not redundant with the second, for the reason
    //! [`crate::session::store`] gives about resume: the trigger governs what this
    //! database will *accept* from now on, while the boundary check governs what
    //! Glasshouse will *act on* — including a row that predates a guard, arrived
    //! through a restored backup, or was written by a build whose triggers
    //! differed. Retrieval is the operation that turns a stored row into something
    //! an agent will treat as true, so it verifies rather than assumes.
    //!
    //! # No credentials
    //!
    //! There is no column here for a token, a key, or a provider secret, and there
    //! is no field for one either. The project database is checked into nothing
    //! and backed up casually; the operating system's secret storage
    //! ([`crate::secret`]) is where a credential lives. `body` is free text an
    //! extractor produced, which is exactly why nothing may *route* a credential
    //! into it.

### `memory/store.rs` — normalize_observed_path

    /// The one spelling `memory_files.path` accepts, or `None`.
    ///
    /// **Migration 17's column contract, enforced where it can be enforced.** The
    /// column is repo-relative, `/`-separated, UTF-8 and never absolute; the
    /// schema can only refuse the empty string, because `CHECK (path NOT LIKE
    /// '/%')` would miss `C:\...` and a `CHECK` forbidding `\` or `:` would
    /// reject file names that are legal on Unix. So the contract is a function,
    /// and every writer goes through it.
    ///
    /// # Why refusing beats normalising here
    ///
    /// Two spellings of one file become two rows, and the exact-match index then
    /// silently misses one of them — a missed association is invisible, where a
    /// wrong one is at least wrong out loud. So anything this cannot bring to the
    /// canonical spelling with certainty is dropped rather than guessed at:
    ///
    /// - `\` becomes `/`, because git's own index never writes `\` and a
    ///   Windows-shaped path reaching here means some other producer wrote it;
    /// - a leading `./` is stripped, and repeated or trailing separators are
    ///   collapsed, because they name the same file;
    /// - an **absolute** path is refused — it is not repo-relative, and the
    ///   project root is exactly where the `/var` versus `/private/var` class of
    ///   ambiguity lives, which is the reason this column stores no root at all;
    /// - a `..` component is refused, because it can leave the repository and no
    ///   normalisation here can tell whether it did;
    /// - an empty result is refused, which is what `.` and `./` collapse to.
    ///
    /// The observed producer never exercises any of this: git's index is already
    /// UTF-8, repo-relative and `/`-separated on every platform, Windows
    /// included, and `checkpoint::git::parse_index` reads it with no separator
    /// translation. This exists for the producers that come after it.

### `memory/store.rs` — DecisionProvenance

    /// Why a durable decision was made, and what it assumed — Phase 21B.
    ///
    /// # Why these are fields and not one blob of prose
    ///
    /// The memory-validity principle is that *"an old decision is not still
    /// correct merely because it was remembered"*. Deciding whether a decision
    /// still holds means checking its assumptions against the project as it is
    /// now, and that is only mechanisable if the assumptions are separable: a
    /// scale assumption is rechecked against a benchmark, a security assumption
    /// against a new requirement, a compatibility assumption against a platform
    /// bump. Phase 21C is the phase that does the rechecking; this is the shape
    /// it needs to find.
    ///
    /// # `None` means "not known", never "none"
    ///
    /// Every field is optional and absent is never the same as empty. A decision
    /// that recorded no security assumption is a decision nobody asked that
    /// question about; a decision that recorded *"none: this path handles no
    /// user data"* has answered it. Collapsing the two would make Phase 21B's
    /// *"when they influenced the decision"* unrepresentable, and would make
    /// [`DecisionProvenance::is_thin`] — which drives Phase 21B's
    /// lower-confidence rule — meaningless.
    ///
    /// # Every field here is free text, and free text can hold a credential
    ///
    /// The same statement `subject` and `body` carry, recorded in migration 6
    /// rather than left to be inferred. The control is on the producer side:
    /// `super::extract::schema::judge` screens each emitted element **whole**,
    /// before reading any field, so a field added to this struct is covered
    /// automatically. [`DecisionProvenance::source_excerpt`] is the sharpest of
    /// them because it is verbatim session text.

### `memory/store.rs` — supersede_with_reason

        /// [`MemoryStore::supersede`], recording **why** — map line 925.
        ///
        /// # Why the reason is a separate door rather than a changed signature
        ///
        /// Superseding without a reason stays legal and stores `NULL`: the map
        /// asks that the reason be *recordable*, not that every supersession have
        /// one, and Phase 22's `superseded_by` is already allowed to be absent for
        /// the same kind of reason. Callers that have nothing to say keep calling
        /// [`MemoryStore::supersede`] unchanged.
        ///
        /// # What happens to blank text, and why here rather than at the caller
        ///
        /// `Some("")` and `Some("   ")` are recorded as `None`. A reason that is
        /// only whitespace is not a reason, and if it were stored the row would
        /// read back as *"a reason was recorded"* to every consumer. Migration
        /// 13's `CHECK` refuses `''` outright, so this is the trim that keeps a
        /// blank `--reason` from being an error the user cannot act on.
        ///
        /// # The reason is operator text and never reaches SQL as text
        ///
        /// It is bound as parameter `?4` — never formatted into the statement —
        /// and it is not logged. The `UPDATE` also keeps its `project_id`
        /// predicate, so this cannot write across the project boundary even if a
        /// caller somehow held a foreign identifier.

### `memory/store.rs` — record_observed_files

        /// Record which files were being worked on when `memories` were learned —
        /// migration 17's `memory_files`, and the only writer of it.
        ///
        /// `paths` is
        /// [`crate::checkpoint::WorkingTreeStatus::changed_files`]: what the git
        /// index said differed from the working tree at the moment extraction
        /// ran. Every row this writes carries
        /// [`FileAssociation::Observed`] and **never anything stronger** — see
        /// that variant, and migration 17's own text, for why *observed-dirty* is
        /// not *explicitly referenced* and why writing the stronger word here
        /// would invert map line 1294's rule rather than bend it.
        ///
        /// # An empty answer is an absence, never a row
        ///
        /// A clean tree writes nothing: no memory, no path, no empty-string path
        /// standing in for "there were none". Neither does a path this build
        /// cannot bring to the column's canonical spelling — see
        /// [`normalize_observed_path`], which drops rather than guesses. The
        /// count returned is rows written, so a caller that wants to know whether
        /// anything was recorded can ask without querying the table back.
        ///
        /// # It is the cross product, and that is the signal rather than a defect
        ///
        /// Every memory here was extracted from one session, and the dirty set is
        /// a property of that session and not of any one memory. Three memories
        /// and twenty paths are sixty rows, each of them true: *"this was learned
        /// while that file was being worked on."* A producer that could say more
        /// than that does not exist in this build.
        ///
        /// # Failure is never the caller's problem
        ///
        /// Returns [`MemoryStoreError::Sql`] the way every other writer here
        /// does, so a caller may log it — but a caller on the extraction path
        /// should log and continue: the memories are already stored, and losing
        /// their file association is strictly better than losing the session's
        /// turn to a bookkeeping error.

### `memory/rerank.rs` — module doc

    //! Reranking the top lexical memory candidates by a cheap language model —
    //! capability map lines 1089-1092 and 1094.
    //!
    //! # What this module refuses to be
    //!
    //! [`super::inject`]'s ladder — invariants and constraints first, then
    //! failed attempts, then everything else — is settled and stays settled.
    //! [`rerank`] never sees an invariant or a constraint: it is handed only
    //! [`super::search::RetrievalResult::other`], the bucket [`super::inject`]
    //! already treats as ordinary matches, and it reorders *within* that bucket
    //! and nothing above it. Nothing here can promote a candidate past a rung
    //! its own authority did not earn, for the same reason [`super::inject`]'s
    //! own module doc gives: the ladder is a stable partition, and this is one
    //! more pass over one partition of it.
    //!
    //! # Every failure is a bypass, never an error the session sees
    //!
    //! No `[memory] rerank_model` configured, fewer than two candidates, a model
    //! that times out, refuses, or answers something this module cannot trust —
    //! every one of these is [`RerankOutcome`] carrying a reason, and every one
    //! of them leaves `candidates` in the lexical order they arrived in. A
    //! reranker that could turn "the model was unreachable" into "no memory is
    //! injected" would make Glasshouse's own memory less available exactly when
    //! the network is least available — the shape `GH-LOCAL-REDUCER`'s own
    //! posture (`docs/product/evidence/phase-58.md`) already refuses.
    //!
    //! # The reply is a JSON array of ids, and it is strictly parsed
    //!
    //! The model is not asked to write a memory —
    //! [`super::extract::schema::PROMPT_CONTRACT`] is a different contract for a
    //! different job — it is asked to return the ids it was given, reordered. So
    //! the whole reply is one array of strings, and this module's own reply
    //! parsing is stricter than [`super::extract::schema::parse`] in the one way
    //! that matters here: an id the reply names that was never sent is not a memory
    //! this module can classify as low-confidence and keep half of, the way a
    //! malformed extraction field can — it is evidence the reply is not
    //! answering about the candidates it was actually given, and the whole
    //! reply is a bypass.

### `memory/rerank.rs` — resolve_rerank_model

    /// Resolve `[memory] rerank_model` into a callable model, or say `None`.
    ///
    /// The extraction seat's four steps (`docs/product/evidence/phase-9i.md`'s
    /// `GH-ROUTED-EXTRACTION-CLIENT`) — consent, the local bypass, the choice,
    /// the client — for `JobKind::Reranking`. Lives here, in the library, rather
    /// than beside `main.rs::disposable_extraction_model`: [`super::inject::briefing`]
    /// is reached from **two** doors, `main.rs::brief_launch_session` and
    /// `crate::api::unix::select_memory`, and only the library crate is common
    /// to both — `main.rs` is a separate binary target that cannot be called
    /// from `crate::api`. `main.rs::disposable_rerank_model` is this function by
    /// another name, kept there only as the thin call `brief_launch_session`
    /// makes.
    ///
    /// # `None`, never a model whose calls would all fail
    ///
    /// Unlike [`super::extract::disposable::RoutedModel`]'s own "route, explain,
    /// record, call nothing" posture for memory extraction's unconsented case
    /// (Phase 9I line 534: durable evidence of what *would* have been chosen),
    /// reranking has no such evidence requirement for a knob nobody set — see
    /// map line 1090. So this returns `None` immediately when unconsented,
    /// before any candidate, health read, or routing decision is built, rather
    /// than returning a model that would only fail when called. `None` is also
    /// the answer when a candidate could not be built, the router found none,
    /// or the resolved client could not be built — every one of these is
    /// `RerankOutcome::Bypassed` at [`rerank`]'s own call site, with the reason
    /// [`super::extract::ModelError::Unavailable`] gives.
    ///
    /// # What this deliberately does not carry, unlike extraction's own seat
    ///
    /// No persisted cross-process health ([`crate::routing::free::FreePool::new`]
    /// is the honest argument for a caller with no history — that type's own
    /// doc comment), and no paced-request reservation claim. At most one
    /// rerank call happens per briefing, so the pacing that protects a shared
    /// free allowance under memory extraction's own dispatch volume is not
    /// reused here; a Green follow-up if reranking's own volume ever earns it.
    ///
    /// No `session` parameter, unlike `main.rs::disposable_extraction_model`:
    /// this function records nothing (see above), so it has nothing to key a
    /// record by. `main.rs::disposable_rerank_model` keeps `session` in its own
    /// signature, matching its sibling's shape, and does not pass it here.

### `memory/export_local.rs` — module doc

    //! `glasshouse memory export-local` — Phase 58 item 6, map line 2040: *"An
    //! opt-in export of remembered constraints and failed approaches into a
    //! marker-delimited block of the harness's native local instruction file,
    //! gitignored by default, replacing only its own block on re-export."*
    //!
    //! # A sibling verb, not `export`
    //!
    //! [`super::export::TrackedKnowledge`] (`glasshouse memory export --tracked`)
    //! is Phase 50's projection of decisions and constraints into
    //! `.glasshouse/knowledge/`, a **tracked** directory reviewed through an
    //! ordinary Git workflow. This module writes a different file, for a
    //! different reader, under a different verb: `CLAUDE.local.md` is the
    //! harness's own **local**, conventionally untracked instruction file, read
    //! at launch rather than reviewed in a diff. The two share no flag and no
    //! destination — the orchestrator's ruling on the worker's blocked report of
    //! 2026-09-02, after the CLI's `Export` name turned out to already be Phase
    //! 50's. Never merge them.
    //!
    //! # Why this reuses `inject`'s renderer rather than a copy
    //!
    //! `super::inject::render_entry` is already the format a session reads at
    //! launch: the bracketed head (`[position/total standing kind=... authority=...
    //! id=...]`) plus the quoted subject and body. Calling the same function here
    //! means an exported entry and an injected one are, byte for byte, the same
    //! shape — a reader who has seen one recognizes the other. `render_entry`,
    //! `standing` and `quote` were private to `inject.rs`; this package's only
    //! change to that file is widening the three to `pub(crate)`, a visibility
    //! change and nothing else.
    //!
    //! # What never happens here
    //!
    //! Nothing in this module runs unless `glasshouse memory export-local` is
    //! typed: no hook, no launch-time call, no timer. It reads
    //! [`super::MemoryStore::binding`] and [`super::MemoryStore::current_of_kind`],
    //! both of which already scope to the active project and to
    //! [`super::MemoryStatus::Active`] — the same read boundary every other
    //! memory command goes through. Every byte outside the marker block is
    //! copied forward unchanged, and the user's own `.gitignore` is never opened
    //! for writing, only read.

### `memory/snapshot.rs` — module doc

    //! The concise current-project snapshot agents ask for (Phase 26).
    //!
    //! Declared ahead of its implementation so that the module owning it never has
    //! to edit `memory/mod.rs`, which another worker holds.
    //!
    //! # What "current" means here
    //!
    //! Only [`MemoryStatus::Active`] memories are current. A todo whose status is
    //! [`MemoryStatus::NeedsReview`] or [`MemoryStatus::Conflicted`] is still open
    //! work by [`MemoryStatus::is_open_work`], but this snapshot is stricter: it
    //! is what an agent treats as settled project knowledge, and a memory under
    //! review or in conflict with another is exactly the opposite of settled. So
    //! every section here holds `Active` memories and nothing else — the resolved,
    //! superseded, rejected, invalidated, needs-review and conflicted rows stay in
    //! the database, queryable by id or by [`super::MemoryStore::with_status`],
    //! and simply do not appear.
    //!
    //! # Budget, by construction
    //!
    //! [`snapshot`] takes a [`SnapshotBudget`] and honours it on every section
    //! independently: a per-section entry cap, and a per-entry body length. A
    //! project with five thousand memories and one with fifty produce
    //! same-sized output. Nothing is silently dropped — a section that hit its
    //! cap reports how many entries it left out ([`SnapshotSection::omitted`]),
    //! and an entry whose body was cut records that it was
    //! ([`SnapshotEntry::body_truncated`]).

### `memory/extract/diagnostics.rs` — module doc

    //! Extraction diagnostics — capability map line 1769: one JSON line per
    //! extraction run, appended to `<state_dir>/memory-extraction.jsonl` only
    //! when `[memory] extraction_diagnostics` is on
    //! ([`crate::config::EffectiveConfig::memory_extraction_diagnostics`]).
    //!
    //! Modeled on [`crate::memory::rerank::append_diagnostics`]'s own shape —
    //! one `create(true).append(true)` open and one `write_all`, fail-soft, a
    //! path under `runtime.state_dir()` and therefore project-scoped — and on
    //! that module's own `serde`-encoded record: this module, unlike
    //! `crate::evaluation`, carries no pin against a general-purpose
    //! serializer (verified: `memory::extract::schema` and
    //! `memory::extract::model` already depend on `serde`/`serde_json` for the
    //! extraction contract itself), so the line is encoded the same way
    //! `rerank`'s is rather than hand-assembled.
    //!
    //! # What never reaches this file
    //!
    //! The prompt, a memory's body or subject, and a rejection's own free text
    //! (a model's malformed reply, an unknown field value, a store's rendered
    //! error) never appear here — only ids, the closed vocabulary words this
    //! module maps each reason to, and counts. [`ExtractionOutcome`] itself
    //! carries the prompt nowhere ([`super::Prompt`] has no accessor that would
    //! let it), so the guarantee this module adds is narrower: every *reason*
    //! recorded here is a fixed word or a schema field name, never a value
    //! copied from the model's reply.

### `memory/extract/lifecycle.rs` — module doc

    //! Turning a session's recorded lifecycle events into something extraction
    //! can read (Phase 21).
    //!
    //! # Why the event log, and not the terminal
    //!
    //! Phase 21 asks the extractor to be fed *"bounded session/event chunks"*.
    //! The **event** half is what Glasshouse actually has after a turn ends: a
    //! `glasshouse hook` process is a separate short-lived program with no access
    //! to the interface's scrollback, and the project database is the only thing
    //! both it and the interface can see.
    //!
    //! It is also the only source that is safe by construction. A hook payload
    //! carries the user's prompt and the model's last message; Glasshouse's
    //! handler drains that stream **unread**, and `lifecycle_events` has no
    //! column a conversation could reach — migration 5 says so and
    //! `the_hook_command_never_reads_its_payload` enforces it. So a chunk built
    //! here cannot contain conversation text, because there is none to contain.
    //!
    //! **State the cost plainly, because it is the honest limit of this
    //! path.** What an event chunk carries is the *shape* of a session — turns
    //! starting and ending, how a turn ended, how much text was delivered and
    //! from where, a process exiting, a gateway failing. That is enough for a
    //! model to record a finding about how a session behaved and nowhere near
    //! enough for it to recover why a decision was made. Until Glasshouse has a
    //! richer source that does not read a conversation, automatic extraction is
    //! bounded by this, and `glasshouse memory extract --activity` remains the
    //! way to feed it something a person chose.
    //!
    //! # Why the range is computed from what survived the budget
    //!
    //! [`SessionChunk::build`] keeps the newest entries when the budget binds. A
    //! provenance range naming events whose text never reached the model would be
    //! a claim this module cannot support, so [`chunk_for_session`] narrows the
    //! range to the entries that actually got in. See its implementation note.

### `memory/extract/chunk.rs` — module doc

    //! What the extractor is allowed to be shown: a bounded, scrubbed chunk.
    //!
    //! # One constructor, two guarantees
    //!
    //! [`SessionChunk::build`] is the only way to make one, and it does two
    //! things no caller can skip:
    //!
    //! 1. **It bounds.** Phase 21 requires bounded session/event chunks "rather
    //!    than entire unbounded session histories". A limit a caller passes is a
    //!    limit a caller forgets, so the bound is applied here, three ways at
    //!    once — a cap on entries, a cap on each entry, and a cap on the whole —
    //!    and the third is the one that matters: without it, a thousand entries
    //!    just under the per-entry cap is an unbounded chunk assembled out of
    //!    bounded parts.
    //!
    //! 2. **It scrubs.** Every entry goes through
    //!    [`super::credentials::scrub`] on the way in, so there is no
    //!    `SessionChunk` anywhere in the program holding un-scrubbed text. That
    //!    is what makes "the extractor is never fed credential material" a
    //!    property of the type rather than a rule someone has to remember at
    //!    every call site — and the prompt can only be built from this type.
    //!
    //! # Newest first, and why the tail is what survives
    //!
    //! When there is more activity than the budget allows, the **most recent**
    //! entries are kept. A task's conclusion is at its end: what was decided,
    //! what failed, what was agreed. The beginning is where the exploring
    //! happened, and Phase 21A specifically does not want an idea discussed
    //! early to arrive with the authority of a decision made late.
    //!
    //! # Nothing is dropped silently
    //!
    //! [`SessionChunk::dropped`], [`SessionChunk::truncated`] and
    //! [`SessionChunk::redactions`] report exactly what the budget and the
    //! scrubber removed. A chunk that lost half a session and says so is
    //! evidence; one that lost half a session quietly is a bug that looks like a
    //! result.

### `session/recovery.rs` — module doc

    //! What may happen to a task whose session died.
    //!
    //! Phase 45's three lines, and nothing else: resume a failed task in the same
    //! native session when possible, hand it to a fresh session when appropriate,
    //! and refuse to retry a destructive task on a different harness without
    //! enough task-state information to know that is safe. [`plan`] is the single
    //! decision point; it is pure, so a caller supplies everything it needs and
    //! reads back exactly one of three outcomes.
    //!
    //! # What counts as "enough task-state information"
    //!
    //! Narrowly: a [`CheckpointRef`] and nothing else. A [`TaskState`] may also
    //! carry an `event_history` — what the harness reported doing, turn by turn —
    //! but [`plan`] never reads it when deciding whether a cross-harness retry is
    //! safe. An event history records what happened to the *session*: turns
    //! starting and ending, text arriving. It does not record what the *task* had
    //! already done to the world, which is the only question a destructive retry
    //! needs answered. Only a portable checkpoint answers it, and Phase 19 — which
    //! produces checkpoints — is not implemented yet, so in this build the answer
    //! is effectively always "no" for a destructive or unknown-kind cross-harness
    //! retry. That is the correct outcome, not a gap: the capability map's line
    //! asks Glasshouse to *avoid* the retry, and refusing does exactly that.

### `session/runtime.rs` — module doc

    //! Several live harness sessions at once.
    //!
    //! [`fn@crate::session::attach`] runs exactly one harness and gives it the user's
    //! terminal for the whole of its life. That is the right shape for
    //! `glasshouse launch` and the wrong shape for an interface that shows several
    //! sessions: its input pump cannot be cancelled, so it relies on the process
    //! exiting out from under it, and nothing else can have the keyboard meanwhile.
    //!
    //! [`SessionRuntime`] is the other shape. It owns any number of live
    //! [`LiveSession`]s, each with its own reader thread draining the pseudo-
    //! terminal into its own bounded [`Scrollback`]. Every session keeps running
    //! whether or not anyone is looking at it, and focus is *only* a statement
    //! about which one the keyboard reaches — changing it never touches a process.
    //!
    //! Two consequences worth being explicit about, because they are the whole
    //! point:
    //!
    //! - **Output is never lost while a session is unfocused.** Each session's
    //!   reader thread runs continuously and independently; the buffer is what the
    //!   viewport reads from when the session is brought forward.
    //! - **A session's exit is detected from the process, not from its output.** A
    //!   harness that dies silently is noticed exactly as fast as one that prints a
    //!   farewell, because [`SessionRuntime::poll_exits`] asks the process.

### `session/runtime.rs` — USER_INPUT_PRECEDENCE

    /// How long a person keeps the keyboard after putting something into a
    /// session — capability map line 1719.
    ///
    /// For this long after a person's own input reaches a session, machine text
    /// aimed at that same session is **refused**, and told why. A person and an
    /// orchestrator addressing one harness are two hands on one keyboard, and the
    /// harness cannot tell them apart: it sees one stream of bytes into one line
    /// editor. This project has already paid for what happens when they collide —
    /// see `SessionRuntime::deliver`, where *"a second message into a worker
    /// mid-turn ended that turn and stranded it"* is recorded as the reason a
    /// concurrent delivery is refused rather than queued.
    ///
    /// # Why ten seconds, and why it is a constant
    ///
    /// It is long enough to cover the gap between a person's line landing and the
    /// harness taking the turn it starts — the window in which an orchestrator's
    /// message would either be swallowed by the line editor or end that turn —
    /// and short enough that an orchestrator blocked by it is blocked for one
    /// visible moment rather than for a stretch anyone would have to plan around.
    ///
    /// It is deliberately **not** configuration. A setting here would be a knob
    /// for turning off the rule that a person outranks a machine at their own
    /// keyboard, and a person who wants an orchestrator's message delivered while
    /// they type has a way to say so already: wait, or use a different session.
    /// The one control the map does ask for — a person stopping machine messages
    /// *entirely*, for a time they name — is line 1717's mute, which is a
    /// separate verb with an explicit duration.

### `session/runtime.rs` — deliver

        /// The one path an input reaches this session by — Phase 10A's thirteenth
        /// line.
        ///
        /// *"Never deliver two inputs to the same session concurrently."* Two
        /// things make that true, and neither on its own would:
        ///
        /// - **One path.** Keystrokes, a line typed at the shell's prompt, a
        ///   machine-sent message, an interrupt, and the runtime's own answer to
        ///   a terminal query all arrive here. A second place that touched
        ///   `self.process` would be a second order nobody arbitrates, which is
        ///   why `only_one_path_writes_to_a_session` fails if one appears. The
        ///   terminal-query reply is in that list deliberately: it is bytes on the
        ///   same terminal, and a `\x1b[24;80R` landing in the middle of a line
        ///   somebody typed corrupts both.
        /// - **A per-session lock, held across the whole delivery.** `try_lock`
        ///   rather than `lock`: a second concurrent delivery is *refused* and the
        ///   caller told, never queued behind the first. Queuing would deliver it
        ///   eventually, out of the order its sender believed, which is the
        ///   failure this project already paid for once in its own process — a
        ///   second message into a worker mid-turn ended that turn and stranded
        ///   it.
        ///
        /// In today's build the lock is never contended, because every delivery
        /// method takes `&mut self` and the shipped binary owns the runtime behind
        /// a `Mutex`. It is here for the shape of the change that would break it,
        /// which is a shape that compiles.

### `session/runtime.rs` — start (readiness settle)

            // Phase 10A, ninth line — *"require a started session to become
            // verifiably ready within a bounded time, and record a start that never
            // became ready as a failure with a stated reason rather than as a
            // session"* — is deliberately **not** enforced here, and this comment is
            // the reason why.
            //
            // An earlier version of this phase waited `READINESS_SETTLE` for the
            // process to prove itself and refused the start when it died inside the
            // window. It could not be made to mean the same thing on two operating
            // systems, and it cost a capability that was already closed.
            //
            // # What was measured
            //
            // The fixture is `echo STARTED; kill -9 $$` — the harness
            // `tests/events_lifecycle.rs` has used since Phase 45 closed
            // *"preserve terminal output and event history after a worker
            // crashes"*. One tree, one gate run: **macOS 5 passed, Linux 3 passed
            // and 2 failed.** The cause is not the length of the window. It is that
            // the two kernels disagree about a process that has died and not yet
            // been reaped: `/proc/<pid>/stat` still describes a zombie, so Linux
            // keeps looking and the parent handle reports the `SIGKILL` first;
            // `proc_pidinfo` stops answering for one, so macOS concludes it cannot
            // identify the process and keeps the session. Same code, opposite
            // answers, neither of them a coin flip — so no larger settle window
            // fixes it.
            //
            // # And there is no in-start refusal that would have been right
            // {#no-deterministic-refusal}
            //
            // `spawn` returns a live process id before anyone knows whether the
            // `exec` behind it worked, so at start time *"the process is alive"* is
            // always true and *"it died"* is always a later observation. That
            // observation is the same one for a harness whose configuration was
            // unreadable and for a harness that ran and crashed: on Windows the two
            // fixtures this repository uses for those cases are the same three
            // lines. Nothing separates them — not the exit status, and not whether
            // output arrived, which under Linux container load is the flake §34
            // already records.
            //
            // # So the line is answered where the difference is real
            //
            // In the record. A start that never became ready is one whose record
            // never left `starting` and whose process is gone;
            // `supervision::reconcile` concludes exactly that, durably,
            // identically on every platform, and says so in `supervision_reason` —
            // and a session whose harness died is recorded as `failed` by
            // [`SessionRuntime::poll_exits`], with the harness's own last words
            // still in its scrollback. Both are failures with a stated reason, and
            // neither throws away the output the user needs to see why.
            //
            // Refusing the start is what discarded that output, and it is what a
            // capability that was already closed was closed *against*.

### `session/runtime.rs` — start (dead-entry removal)

            // Structural, not remembered. An *exited* entry under this id is kept
            // deliberately by `poll_exits`, so that a crashed worker's output and
            // its crash report survive it; pushing beside that entry is precisely
            // what the comment on the duplicate guard above describes, because
            // `get`, `get_mut`, `focus`, `close` and `crash_report` all resolve
            // the **first** match and the corpse is the one already in the vector.
            // The live session would then be steerable by nobody, and a send to it
            // would return `RuntimeError::Exited` for a harness the user can watch
            // running.
            //
            // `shell::resume_session` has been calling `close` by hand to avoid
            // exactly this, with nine lines of comment explaining why. Doing it
            // here means the invariant holds for every caller that reuses an id —
            // `api`, `main`, a future resume path — rather than only for the
            // callers that happened to know.
            //
            // Removing rather than refusing, because refusing would make a
            // restart-under-the-same-id impossible without the caller first
            // knowing to close: the same remembered obligation, moved one step.
            //
            // **Here, and not beside the guard above**, because everything between
            // there and this line can fail — `launch.spawn()` and `spawn_reader`
            // both carry a `?`. Removing earlier would mean a failed restart threw
            // away the crash report of the run that prompted it, which is the one
            // thing the caller would then want to read.

### `session/runtime.rs` — deliver (SessionRuntime)

        /// The one path an input reaches a session by — Phase 10A's thirteenth
        /// line.
        ///
        /// *"Never deliver two inputs to the same session concurrently."* Two
        /// things make that true, and neither on its own would:
        ///
        /// - **One path.** Keystrokes, a line typed at the shell's prompt, a
        ///   machine-sent message and an interrupt all arrive here. A second place
        ///   that touched `session.process` would be a second order nobody
        ///   arbitrates, which is why `only_one_path_writes_to_a_session` fails if
        ///   one appears.
        /// - **A per-session lock, held across the whole delivery.** `try_lock`
        ///   rather than `lock`: a second concurrent delivery is *refused* and the
        ///   caller told, never queued behind the first. Queuing would deliver it
        ///   eventually, out of the order its sender believed, which is the
        ///   failure this project already paid for once in its own process — a
        ///   second message into a worker mid-turn ended that turn and stranded
        ///   it.
        ///
        /// In today's build the lock is never contended, because every delivery
        /// method takes `&mut self` and the shipped binary owns this runtime
        /// behind a `Mutex`. It is here for the shape of the change that would
        /// break it, which is a shape that compiles.

### `session/runtime.rs` — note_user_input

        /// Record that a person put something into this session at `at` —
        /// capability map line 1719.
        ///
        /// Called by every path a person's own input reaches a session by:
        /// [`SessionRuntime::write_to_focused`] (the keyboard),
        /// [`SessionRuntime::send_text_from`] (the shell's send-a-line prompt and
        /// `glasshouse api send`), and [`SessionRuntime::interrupt_from`].
        ///
        /// `at` is a parameter rather than read from the clock here so that the
        /// window's *expiry* is testable without a test sleeping through it. Every
        /// production caller passes `Instant::now()`; a test that needs to stand
        /// on the far side of [`USER_INPUT_PRECEDENCE`] passes a moment that far
        /// in the past, which is the same call the binary makes rather than a
        /// door beside it.
        ///
        /// Never moves the mark backwards: two inputs in the window leave the
        /// later one standing, so a person typing steadily keeps the keyboard
        /// rather than losing it to the age of their first line.
        ///
        /// A session this runtime does not hold is silently ignored — there is
        /// nothing to protect and nothing to say, and every caller here has
        /// already established liveness by writing to it.

### `session/runtime.rs` — poll_exits

        /// Notice any session whose process has ended since the last call.
        ///
        /// Asked of the process, never inferred from its output going quiet — a
        /// harness can be silent for minutes while thinking, and treating that as
        /// death is the classic way a session manager kills work in progress.
        /// Each exit is reported exactly once; the session stays in the runtime
        /// afterwards so its final output remains readable.
        ///
        /// **Reported only for a session that stayed exited.** A death that this
        /// method answers by putting the harness back is published to the history
        /// as [`LifecycleEvent::ProcessExited`] and then dropped from the returned
        /// vector, because every caller of it treats an entry as the end of the
        /// session — see the comment on the `retain` below.
        ///
        /// # Windows: this is also where output is declared to have ended
        ///
        /// [`crate::pty`] wrote down, before the reader thread existed, that a
        /// reader "must not treat *no more bytes* as its stop condition, because
        /// on Windows that may never come while the pty is still held open", and
        /// prescribed observing the process instead. The prescription is right
        /// about the diagnosis and **cannot be carried out where it points**: by
        /// the time there are no more bytes, `pump` is parked inside a blocking
        /// `read` that will not return, so it can neither call `try_wait` nor
        /// notice a flag someone set for it. A stop condition is no use to a
        /// thread that has already stopped.
        ///
        /// So the thread that *does* observe the exit says so. When a session's
        /// process has been seen to end and `OUTPUT_DRAIN_WAIT` has passed
        /// since, this marks the session's output finished and publishes
        /// [`LifecycleEvent::OutputEnded`]. `pump` still publishes it on every
        /// platform that produces an end-of-file, and
        /// `OutputEnd::finish` decides which of the two got there first, so the
        /// event fires exactly once either way.
        ///
        /// **Nothing is truncated by this.** The reader is not stopped and not
        /// interrupted; it keeps draining into the scrollback for as long as the
        /// session is held. The grace is what keeps the *event* honest — a
        /// child's death outruns its last words by a millisecond or two, measured
        /// at 1.1–2.2ms on Linux — and a byte that somehow arrives after it is
        /// still recorded, only after the announcement.
        ///
        /// **Windows only, deliberately.** On Unix "the output ended" is a
        /// statement about a file descriptor and the descriptor can make it, so
        /// nothing here should redefine it — including the case this would
        /// otherwise change, a crashed harness whose grandchild still holds the
        /// pty slave open, where the strict meaning is the true one and *no*
        /// output-end really has happened. On Windows that descriptor cannot
        /// speak at all, so the honest meaning there is the weaker one:
        /// **the process exited and its output stopped arriving.**

### `session/runtime.rs` — poll_exits (retain)

            // A session that was put back is not an ending anyone may act on.
            //
            // `ProcessExited` has already been published, so the history still
            // records the death — what must not travel out of here is the claim
            // that the session is *over*. Every consumer of this vector treats an
            // entry as terminal: `shell::run` writes
            // `ProcessExit::session_state()` into the durable record and runs
            // `session::native_id::capture`, and `main.rs`'s headless loop returns
            // that status as the run's own result. A record left reading `Failed`
            // or `Stopped` for a live harness is not merely wrong on a list:
            // `supervision::guard_start` returns `Ok(())` for any record whose
            // lifecycle is not live, so nothing downstream would refuse a start
            // over the top of the conversation this harness is still holding —
            // the duplicate `open_for_resume` exists to prevent, reached from the
            // outside. `native_id::capture` also runs its end-of-session discovery
            // window against a mid-life session, which is the widest that window
            // can be rather than the tightest it assumes. And the focus fix-up
            // below would move the keyboard off a session that is running again.
            //
            // The predicate is the session's *observed* state after the restart
            // attempt, not the fact that one was attempted, so every way
            // `consider_restart` can decline or fail — a clean exit, a deliberate
            // one, a harness that was never healthy, the bound reached, a spawn or
            // reader that failed — leaves `exit` set and the exit reported. A
            // session no longer in the runtime at all is kept for the same reason:
            // there is nothing alive to withhold the report for.
            //
            // A harness that is put back and dies again immediately is not lost,
            // only deferred: its new process is `exit: None` here, and the next
            // poll asks it, publishes a fresh `ProcessExited`, and reports the
            // exit as soon as one of them is the death it stays dead of.

### `session/runtime.rs` — consider_restart

        /// Answer the terminal questions the sessions have asked.
        ///
        /// **An embedded session inverts `session::attach`'s rule.** `attach` is a
        /// pass-through and must never answer, because the user's real terminal is
        /// on the other end and will; a second reply would reach the harness as
        /// input. Here Glasshouse *is* the terminal — the output goes into a buffer
        /// it owns and is redrawn into a viewport, and no real terminal ever sees
        /// the question. Nothing else can answer, so a harness that waits for a
        /// reply waits forever.
        ///
        /// **Waiting forever is not the only way this hurts.** A harness that
        /// gives up on an unanswered question may not merely degrade for that
        /// session: Claude Code counts the failures and, after two, disables its
        /// fullscreen renderer *globally*, writing that decision into the user's
        /// own configuration where it outlives Glasshouse entirely. Answering is
        /// therefore not a nicety, it is the difference between embedding a
        /// harness and quietly damaging it.
        ///
        /// Called from the interface's tick. Best effort per session: one harness
        /// that cannot be written to must not stop the others being answered.
        /// Put a session's harness back, if this exit was one worth restarting
        /// for and the bound has not been reached — Phase 10A's tenth line.
        ///
        /// # What counts as *exiting unexpectedly*
        ///
        /// Four things exclude a restart, and each of them is a case where putting
        /// the harness back would be wrong rather than merely unnecessary:
        ///
        /// - **A clean exit.** A harness that did its work and left has not
        ///   failed; this project already refuses to call that finishing, and it
        ///   must not call it crashing either.
        /// - **An ending the user asked for.** `interrupt` marks the session, and
        ///   the mark survives only until the session is seen alive again, so it
        ///   excuses the exit it caused and no later one.
        /// - **A session that was never healthy.** This is the load-bearing one. A
        ///   harness that has not once come up did not *exit unexpectedly* — it is
        ///   a start that did not work, and restarting it three more times turns a
        ///   mistyped executable into four processes. It is also the reason this
        ///   line does not disturb `tests/events_lifecycle.rs`: a harness that
        ///   prints one line and dies has crashed, and Glasshouse keeps its output
        ///   and its history rather than trying again.
        /// - **A bound already reached, or a restart that itself failed.** Once
        ///   there is a stated reason, it stands.

### `session/runtime.rs` — crash_report

        /// Everything that survived a crash, or `None` if this session did not
        /// crash.
        ///
        /// A crashed worker's terminal output and event history outlive it,
        /// because neither belongs to the process: the scrollback is Glasshouse's
        /// buffer and the history is the project's bus. The session stays in the
        /// runtime after it exits for exactly this reason — removing it would be
        /// the only way to lose the output, and `poll_exits` deliberately does
        /// not.
        ///
        /// `None` for a session that is running, that exited on its own terms, or
        /// that Glasshouse closed itself: [`SessionRuntime::close`] removes the
        /// session before it signals, so a deliberate kill is never reported as a
        /// crash.
        ///
        /// # Why this waits
        ///
        /// **A process's exit becomes observable before its last output does.**
        /// The exit comes from `waitpid`; the output has to travel through the
        /// pseudo-terminal and be copied into the scrollback by this session's
        /// reader thread, which is a *different* thread that may not have run
        /// yet. Asking `poll_exits` and then reading the scrollback in the same
        /// breath therefore reports a crashed worker as having said nothing —
        /// which is what the Linux gate had been failing on at random for weeks,
        /// and which reproduced at 8 runs in 17 beside the full workspace suite —
        /// see `docs/product/design-decisions.md`, "A pseudo-terminal child's exit
        /// is observable before its output is".
        ///
        /// `session::attach` — the other shape a harness runs in — has always
        /// done this, and says so in `OUTPUT_DRAIN_GRACE`. This path is the one
        /// that had not learned it.
        ///
        /// Nothing is ever lost when that happens: the bytes are in the kernel's
        /// pty buffer and arrive about two milliseconds later. Linux hands a
        /// reader everything that was written before it reports `EIO`, and a
        /// probe of 200 trials per timing confirmed it never drops a byte, even
        /// when the child is reaped before the first read. So this is not data
        /// loss — it is a post-mortem written before the body stopped talking,
        /// and the fix is to let it finish.
        ///
        /// # Why the wait is bounded
        ///
        /// Output is not guaranteed to end at all. A harness that crashed after
        /// starting something of its own leaves that grandchild holding the pty
        /// slave open, and the reader will sit there for as long as it lives —
        /// see [`crate::pty::PtyOutput`], which records the same lifetime rule
        /// from the other side. An unbounded wait here would hang a caller on
        /// exactly the crash it most needs reporting, so the report is produced
        /// either way and the ceiling is 250ms — the same grace `session::attach`
        /// allows its own pump.

### `session/lifecycle.rs` — module doc

    //! Turning a harness's own lifecycle events into Glasshouse's.
    //!
    //! A harness reports what happened in its own vocabulary, and Glasshouse
    //! records one of a handful of states that mean something to a session
    //! overview. Claude Code and Codex happen to spell every shared event
    //! identically — both say `UserPromptSubmit`, not `user_prompt_submit`. An
    //! earlier revision of this module claimed Codex used snake_case, citing the
    //! wrong artifact: Codex's `config.toml` records hook *trust* under
    //! snake_case keys, but the `hooks.json` document it actually reads is
    //! PascalCase, per its own hook review screen. That agreement is why most of
    //! this translation works untouched for either harness — but it is a fact
    //! about the two installed binaries, not a guarantee, so this module is
    //! deliberately the only place that knows either vocabulary at all.
    //!
    //! # Why an unknown event changes nothing
    //!
    //! Harnesses gain events between releases. An event this build has never
    //! heard of must leave the session exactly as it was, because the alternative
    //! — guessing a state from an unfamiliar name — would show the user a session
    //! that is idle when it is working, or working when it is waiting for them.
    //!
    //! # Why a finished session cannot be revived
    //!
    //! Hook processes are separate processes, and a slow one can deliver its event
    //! after the harness it belongs to has exited. Applying it would resurrect a
    //! stopped session in the records, which is worse than losing a note about a
    //! session that has already ended.

### `session/lifecycle.rs` — precedes_native_compaction

    /// Whether `event` is a harness saying it is about to compact its own
    /// context — Phase 21's *"allow memory extraction to run before or around
    /// native prompt compaction."*
    ///
    /// # Why this is a separate question from [`event_for`]
    ///
    /// A compaction is **not a `SessionLifecycle` state**: a session that
    /// compacts was running before and is running after, and there is no
    /// `LifecycleEvent` for it. Answering it through [`event_for`] would mean
    /// inventing one, which would mean a new `database::LIFECYCLE_EVENT_KINDS`
    /// value and a migration to widen a `CHECK`, which SQLite cannot do in place
    /// and which `database`'s own house rule refuses. So this is a predicate a
    /// *trigger* can ask, and the event log stays exactly as narrow as it was.
    ///
    /// # What is recorded, since map line 1159
    ///
    /// A **count**, on the session row: migration 16's
    /// `sessions.observed_compactions`, written by
    /// [`crate::session::SessionStore::record_observed_compaction`] at this
    /// predicate's one production call site. That is a different claim from an
    /// event — it says the compaction has now happened *n* times, not that it
    /// happened at an instant beside everything else that happened — and it is
    /// the one line 1159 asks for. The raw observation is still preserved by
    /// [`observe`]'s own [`crate::events::RawObservation`] line, and no
    /// `lifecycle_events` row is written for it.
    ///
    /// # Why `PostCompact` is not here
    ///
    /// `PostCompact` is a real Codex event and Glasshouse asks for it (see
    /// `harness::codex`'s `REPORTED_EVENTS`), but extraction reads **this
    /// project's event log**, which a harness compacting its own context does not
    /// change. Running on both would be two extractions over identical material,
    /// inside the user's session, per compaction. `PreCompact` is the "before"
    /// the line names and the one that arrives while the harness still has what
    /// it is about to lose. Named explicitly, rather than left to the wildcard,
    /// so the omission reads as a decision.
    ///
    /// # Claude Code, corrected 2026-09-01
    ///
    /// This predicate matches on the event string alone, so it was never the
    /// reason Claude Code's compactions went uncounted — a stale version of this
    /// comment used to say otherwise, from a 2.1.245 reading that found no
    /// compaction hook at all. Claude Code 2.1.257 has one: run and observed
    /// (`harness::claude_code`'s `REPORTED_EVENTS` doc), a manual `/compact`
    /// against a real installation fired a `PreCompact` hook whose payload named
    /// this exact string. The gap was one link earlier — `harness::claude_code`'s
    /// `REPORTED_EVENTS` never asked Claude Code to report it, so no hook was
    /// ever installed and this predicate never saw the event. That is now fixed;
    /// this function itself did not need to change.
    ///
    /// Event names are the harness's own, exactly as its adapter declares them.

### `session/lifecycle.rs` — may_apply

    /// Whether `current` may be moved to `next` by a harness event.
    ///
    /// Only a live session can change state this way. A session that has stopped,
    /// failed, or been closed is finished, and a hook arriving afterwards — from a
    /// process that outlived its harness — must not bring it back.
    ///
    /// # Why a genuine resume needs nothing here
    ///
    /// A resumed session was, for a while, refused by this rule: its record still
    /// read `stopped`, so every hook the reopened harness sent was discarded. The
    /// cause was not this predicate. `main.rs::resume_session` already wrote
    /// *"running"* the moment it reopened the session, and
    /// [`crate::session::SessionStore`]'s own copy of this rule — the one inside
    /// its write transaction, where two processes cannot step over it — declined
    /// that write for exactly the reason above. The record never left `stopped`,
    /// and this function was then asked the wrong question about a state that
    /// should not have been current.
    ///
    /// The fix belongs where the acts differ: `SessionStore::begin_resume` is
    /// something Glasshouse *does*, at a boundary it opened, and a hook is an
    /// event that merely *arrives*. Widening this predicate instead would have
    /// meant a hook arguing for its own authority, which is the one thing the
    /// rule exists to refuse — and it would not have helped, because the record
    /// would still have been `stopped` when it was asked.
    ///
    /// So this stays as it is. Once a resume has been recorded the session is
    /// live, and a live session follows its harness.

### `session/api/mod.rs` — module doc

    //! The internal API for driving and inspecting a live session.
    //!
    //! [`SessionApi`] is the one surface that sends text to, interrupts, or
    //! inspects a session by identifier — the seam an orchestrator, the MCP
    //! surface, or anything else internal to Glasshouse goes through instead of
    //! reaching into [`super::store::SessionStore`] and [`super::runtime::SessionRuntime`]
    //! directly. Two things make that worth a seam of its own:
    //!
    //! - **Project scope is checked once, here, for every entry point.** Every
    //!   method resolves the identifier through the store first and compares its
    //!   `project_id` against the active project before doing anything else —
    //!   including before asking whether the session is even live. A foreign
    //!   session that also happens to be stopped is still refused as foreign,
    //!   never as merely not running, because "you asked about someone else's
    //!   session" is the true answer and the only one worth giving.
    //! - **Who sent a message is recorded, never inferred.** Every write goes
    //!   through [`super::runtime::SessionRuntime::send_text_from`] and
    //!   [`super::runtime::SessionRuntime::interrupt_from`] with an origin its
    //!   **caller** supplies, not the plain `send_text` / `interrupt` that assume
    //!   a person's keyboard. The distinction is recorded in Glasshouse's own
    //!   event log, not inferred later from context that will not exist by then.
    //!
    //!   This seam used to hard-wire [`crate::events::MessageOrigin::Machine`],
    //!   on the reasoning that everything reaching it was Glasshouse or an
    //!   orchestrator. That stopped being true when `glasshouse api send` and
    //!   `glasshouse api interrupt` shipped: a person's keystrokes now arrive
    //!   here, over the control door, and hard-wiring made their intervention
    //!   equal field for field to an orchestrator's own message. A seam that
    //!   *decides* the origin can only be right while it has one kind of caller,
    //!   so this one asks instead. Callers that are Glasshouse still pass
    //!   `Machine` and are unchanged; the control door passes what its request
    //!   said, defaulting to `Machine` when it said nothing.

### `session/api/mod.rs` — send_text

        /// Send a line of text to a live session, on behalf of `origin`.
        ///
        /// A carriage return is appended, the same way `shell::send_session_text`
        /// sends a line typed at the shell's own prompt: this call delivers one
        /// line, not raw bytes, and `\r` is what a session's terminal expects to
        /// see for the harness's line editor to submit it.
        ///
        /// `origin` is the caller's to state and this method's to record — see
        /// the module doc comment for why it is no longer decided here. Pass
        /// [`MessageOrigin::Machine`] for anything Glasshouse itself originates,
        /// which is what every caller inside this process does; only the control
        /// door has a caller it did not write, and only that door can know
        /// whether a person is on the other end of it.
        ///
        /// # A person at this session's keyboard outranks a machine — line 1719
        ///
        /// Machine text is **refused** with
        /// [`ApiError::UserHasTheKeyboard`] while a person has put something into
        /// this same session within
        /// [`crate::session::runtime::USER_INPUT_PRECEDENCE`]. Refused rather than queued,
        /// which is this seam's existing rule and not a new one:
        /// `super::runtime::SessionRuntime::deliver` already refuses a
        /// concurrent delivery instead of queuing it, because *"queuing would
        /// deliver it eventually, out of the order its sender believed"* — and a
        /// message held for ten seconds and then typed into whatever the person
        /// is now doing is that failure with a delay in front of it. A refusal a
        /// caller can read is the answer it can act on.
        ///
        /// The rule is taken **here**, at the one seam every machine sender in
        /// this process passes through — the control door's `send_message`, the
        /// task a spawn delivers, an injected memory briefing, and a worker
        /// completion pumped into an orchestrator — rather than at any one of
        /// them, so there is no machine write path that quietly is not subject to
        /// it. It is deliberately **not** applied to
        /// [`SessionApi::interrupt`]: see that method.

### `session/api/mod.rs` — machine_delivery_refusal

        /// The refusal a machine-originated line to `id` would be given right
        /// now, or `None` if it would be delivered — capability map line 1719.
        ///
        /// [`SessionApi::send_text`] takes this decision itself, so no caller has
        /// to ask, and it is **private on purpose**.
        ///
        /// It was briefly public, so the control door could refuse a machine
        /// message before opening this project's memory store for a briefing it
        /// was about to throw away. That saved one SQLite open and cost the whole
        /// rule: with a copy of the check in front of this seam, mutating the
        /// check *inside* [`SessionApi::send_text`] away left the entire suite
        /// green, because nothing ever reached the seam to be refused by it. A
        /// rule with two enforcement points is a rule with one that nobody
        /// watches.
        ///
        /// So there is one enforcement point, this is its only caller, and a
        /// caller that wants to know without sending has to ask by sending. The
        /// wasted memory open on a refused message is the price, and it is paid
        /// only on the path where a person is already using the session.
        ///
        /// It reads state and changes none, and it resolves through the same
        /// project-scope check every other method starts with, so a foreign
        /// session is refused as foreign here too rather than answered.

### `session/store/mod.rs` — module doc

    //! Glasshouse's own record of the sessions in one project.
    //!
    //! This is deliberately *not* a view over a harness's session files. Claude
    //! Code, Codex, and the rest each keep their own history in their own format
    //! in their own directory, and Glasshouse neither parses nor owns those files.
    //! What it keeps here is the metadata it needs to list, resume, and reason
    //! about sessions: which harness, when it started, when it was last active,
    //! what role it plays, where it is presented, and what state it is in. The
    //! harness's own identifier is recorded when it is known, as a nullable
    //! reference — so a session survives in this table whether or not the harness
    //! kept anything, and clearing a harness's history never silently deletes
    //! Glasshouse's record of what happened.
    //!
    //! # Project isolation
    //!
    //! Every row carries the project identifier, and it is enforced in two places
    //! on purpose:
    //!
    //! - **Structurally**, by SQLite triggers created in migration 2, which abort
    //!   any insert or update whose `project_id` is not the identifier bound in
    //!   `project_metadata`. No query in this module — or any future one — has to
    //!   remember to filter, because a foreign row cannot be written at all.
    //! - **At the resume boundary**, by [`SessionStore::open_for_resume`], which
    //!   compares the stored identifier against the active project before handing
    //!   back anything a caller could act on.
    //!
    //! The second check is not redundant with the first. The trigger governs what
    //! this database will accept from now on; the resume check governs what
    //! Glasshouse will *act on*, including rows that predate a guard, arrived
    //! through a restored backup, or were written by a build whose triggers
    //! differed. A resume is the one operation that takes a stored identity and
    //! turns it back into a running process, so it verifies rather than assumes.

### `session/store/mod.rs` — Revival

    /// Whether this write is Glasshouse resuming a session, and may therefore
    /// move a finished record back to a live state.
    ///
    /// # The asymmetry this type exists to express
    ///
    /// *"A finished session stays finished"* was written for one hazard, and it is
    /// a real one: hook processes are separate processes, and a slow one can
    /// deliver its event after the harness it belongs to has exited. Applying it
    /// would resurrect a stopped session in the records.
    ///
    /// A genuine resume is not that case, and until this marker existed the two
    /// were indistinguishable — with the consequence that
    /// `main.rs::resume_session`'s own *"this session is running again"* write was
    /// silently declined along with the zombies, leaving a demonstrably live
    /// session reading `stopped` and every hook it went on to send discarded. That
    /// was observed against a live Codex, with the resume twenty-nine seconds
    /// after the process exit that preceded it.
    ///
    /// **A resume is an act Glasshouse performs; a late hook is an event that
    /// merely arrives.** So the authority is a value only the resume boundary can
    /// supply, rather than a property of the event or a loosening of
    /// [`SessionLifecycle::is_live`] — which is unchanged, and which other callers
    /// depend on. [`SessionStore::begin_resume`] is the only constructor of
    /// [`Revival::Authorized`] in the crate, and it is unreachable from the hook
    /// path: `glasshouse hook` never opens a resume boundary.

### `session/store/mod.rs` — set_lifecycle

        /// Move a session to a new lifecycle state, which also counts as activity.
        ///
        /// # This is the single ordered path — Phase 10A's twelfth line
        ///
        /// Every lifecycle change in the shipped binary arrives here: the launch
        /// path's `note_lifecycle`, the shell's exit handling and its failed-start
        /// handling, and `glasshouse hook` when a harness reports something. They
        /// are **separate operating-system processes**, so nothing in Rust's type
        /// system orders them, and until this method took a transaction they raced
        /// in the classic read-check-write shape:
        ///
        /// 1. a hook process reads `running` and decides `idle`;
        /// 2. the launch process observes the harness exit and writes `stopped`;
        /// 3. the hook process writes `idle`.
        ///
        /// The result is `idle` — a live state for a session whose process is
        /// gone. Neither writer asked for that, which is exactly the interleaving
        /// the line forbids.
        ///
        /// `BEGIN IMMEDIATE` takes SQLite's write lock **before** the read, so the
        /// read and the write are one indivisible step and the second writer's
        /// check runs against what the first writer actually left. The order is
        /// then decided by the lock rather than by which process happened to read
        /// first, and the losing writer sees the winner's state and declines.
        ///
        /// # What it declines
        ///
        /// One rule, and it is [`super::lifecycle::may_apply`]'s: **a session that
        /// has finished may not be moved back to a live state.** It refuses
        /// nothing the shipped binary legitimately does — every real transition is
        /// from a live state — so this is not a new policy, it is the existing
        /// policy moved to where two processes cannot step over it. A declined
        /// change returns the record as it stands rather than an error: the caller
        /// asked for something that is no longer true, which is not its fault and
        /// not a failure.

### `session/store/mod.rs` — write_lifecycle_locked

        /// **The only statement in this crate that moves a session's lifecycle.**
        ///
        /// That is what "a single ordered path" means at the level a reader can
        /// check: not that one function is polite about it, but that there is one
        /// `UPDATE` and everything else has to come through it.
        /// `one_statement_moves_a_sessions_lifecycle` fails if a second appears,
        /// because a second writer is a second order and two orders are no order.
        ///
        /// Callers must already hold a write transaction — see
        /// [`SessionStore::in_a_write_transaction`], which is what makes the read
        /// below and the write after it one indivisible step.
        ///
        /// # What it declines
        ///
        /// One rule, and it is [`super::lifecycle::may_apply`]'s: **a session that
        /// has finished may not be moved back to a live state.** It refuses
        /// nothing the shipped binary legitimately does — every real transition is
        /// from a live state — so this is not a new policy, it is the existing
        /// policy moved to where two processes cannot step over it. A declined
        /// change leaves the record as it stands rather than erroring: the caller
        /// asked for something that is no longer true, which is not its fault.

### `session/store/mod.rs` — record_observed_compaction

        /// Count one compaction a harness said it was about to perform — map
        /// line 1159.
        ///
        /// # Why this is a column and not an event
        ///
        /// `super::lifecycle::precedes_native_compaction` is the observation, and
        /// its own documentation explains why a compaction cannot join
        /// `LIFECYCLE_EVENT_KINDS`: that vocabulary is a SQL `CHECK`, SQLite
        /// cannot widen one in place, and the eleventh value already cost a full
        /// rebuild of the table `memories` references by `seq`. Migration 16 says
        /// the same thing from the schema's side. So the count lives on the
        /// session row, and the event log is left exactly as narrow as it was.
        ///
        /// # `COALESCE`, and what it costs
        ///
        /// A row recorded before migration 16 reads `NULL`, meaning *"nobody was
        /// counting"*. Its first observed compaction moves it to `1` rather than
        /// leaving it unknowable for ever, so from then on the number is a
        /// **lower bound** — compactions before the upgrade were observed by
        /// nothing and cannot be recovered. For a session this build created the
        /// count is exact, because `create` starts it at a measured `0`.
        ///
        /// # It is not activity
        ///
        /// `last_activity_at` is untouched, for `rename`'s reason turned around:
        /// a compaction is the harness reorganising what it holds, not the
        /// session doing work, and stamping it would move a session up a list
        /// ordered by when it last ran on the strength of housekeeping.

### `session/store/mod.rs` — context

        /// Everything Phase 30 can say about one session's context, as of now.
        ///
        /// `Ok(None)` for a session this project does not have, exactly as
        /// [`SessionStore::get`] answers.
        ///
        /// # Why one function and not five
        ///
        /// Four of Phase 30's lines are answered by facts that already existed —
        /// the session's own activity stamp, its checkpoints, and its turn
        /// events — and were unreadable together. A caller assembling them itself
        /// would have to know that "recent checkpoint" is a comparison against
        /// `last_activity_at` and that a cache state must never be derived from
        /// resumability; those are the rulings this phase is made of, and they
        /// belong in one place rather than in each caller. See
        /// [`SessionContext`], including its paragraph on the line that is
        /// **not** here.
        ///
        /// # It reads two sibling tables, and never writes them
        ///
        /// `checkpoints` and `lifecycle_events` are read by `project_id` and
        /// `session_id` together, so the project boundary
        /// [`SessionRecord::project_id`] draws is honoured by the query and not
        /// merely by the caller. Nothing here inserts, updates or deletes, and in
        /// `lifecycle_events`' case nothing could: migration 5's triggers
        /// `RAISE(ABORT)` on every write but an insert.
        ///
        /// # Nothing here is stored
        ///
        /// The cache estimate and the checkpoint verdict are computed at the
        /// moment they are asked for, on purpose. A stored `hot` is wrong the
        /// minute after it is written, and a stored copy of
        /// `checkpoints.created_at` would be a second source of truth for a
        /// column one table over — migration 15's objection to copying a token
        /// count, applied to this phase. Only [`SessionRecord::observed_compactions`]
        /// is durable, because a compaction leaves no trace anywhere else.

### `session/store/mod.rs` — close

        /// Retire Glasshouse's record of a session — line 654.
        ///
        /// # What this deliberately does not do
        ///
        /// It writes one column. `native_session_id` is untouched, and so is
        /// every harness file on disk: this module has never parsed or owned
        /// those, and closing a Glasshouse record is not an occasion to start.
        /// Line 654 says the record may be closed *"without deleting the native
        /// provider history unless explicitly requested"*, and nothing here is a
        /// request. `closing_a_session_keeps_the_harnesss_own_history` proves the
        /// history is still there afterwards rather than proving no error came
        /// back.
        ///
        /// # A live session is refused
        ///
        /// Closing is filing a record away, and a record whose process is still
        /// running is not finished being written. Refusing names the state so the
        /// user knows to stop the session first, rather than leaving a `closed`
        /// row that a running harness keeps updating.
        ///
        /// # `last_activity_at` stays put, for [`SessionStore::rename`]'s reason
        ///
        /// When the session last did something is a fact about the session. When
        /// somebody filed it away is a different fact, and this column is not the
        /// place for it.

### `session/store/mod.rs` — begin_resume

        /// Record that Glasshouse is resuming this session, moving it back to
        /// `Running`.
        ///
        /// # Why this is not `set_lifecycle`
        ///
        /// [`SessionStore::set_lifecycle`] declines to move a finished record back
        /// to a live state, and must keep declining: a hook process outliving its
        /// harness is exactly what that rule is for. But the resume path's own
        /// *"this session is running again"* write went through the same door and
        /// was refused by the same rule, so a session Glasshouse itself had just
        /// reopened kept reading `stopped` — and every hook the resumed harness
        /// then sent was discarded for arriving at a finished session.
        ///
        /// Observed against a live Codex over five compaction trials, with the
        /// resume recorded twenty-nine seconds after the process exit before it,
        /// so nothing about it was a race.
        ///
        /// The two cases are told apart by **who is acting**. A resume is
        /// something Glasshouse does, at a boundary it opened deliberately; a late
        /// hook is an event that merely arrives. So this is a separate operation
        /// carrying `Revival::Authorized`, rather than a widening of
        /// [`SessionLifecycle::is_live`] or of `lifecycle::may_apply` — and once
        /// this has run, a resumed session is live, so `may_apply` believes its
        /// harness again without knowing anything about resumes at all.
        ///
        /// # The disposition is checked again, under the write lock
        ///
        /// Not defence in depth for its own sake. [`SessionStore::open_for_resume`]
        /// reads outside a transaction, so between its answer and this write
        /// another process can close the record, quarantine it, or start it — the
        /// classic read-check-write window this module's
        /// `in_a_write_transaction` exists to shut. Re-asking
        /// [`SessionRecord::disposition`] with the write lock already held makes
        /// the check and the write one indivisible step, which is Phase 10A's
        /// requirement for every lifecycle change and is what makes this one safe
        /// to authorise at all.
        ///
        /// # `Stopped`, `Failed` and `Closed` are three different answers
        ///
        /// Only a **stopped** record with a native identifier is
        /// [`SessionDisposition::Resumable`], and only that one is revived here.
        /// A **failed** session ended badly and reports
        /// [`SessionDisposition::Failed`]; a **closed** one was retired by the
        /// user, and a stopped one with nothing to resume *to* is
        /// [`SessionDisposition::Closed`]. All three are refused, by the same
        /// classification `open_for_resume` refuses them by — one rule read twice
        /// rather than a second rule that could drift from the first.
        ///
        /// # The process identity is re-recorded here, and that is not a detail
        ///
        /// A resume happens in a **new operating-system process**. Making the
        /// record live again while it still named the `glasshouse` that created
        /// the session left every later invocation verifying a process id that
        /// had exited — so `supervision::reconcile` reached [`Verdict::Gone`],
        /// correctly, and wrote `stopped` back over the resume on the very next
        /// command. Observed twice out of two trials against a live Codex, where
        /// the command that undid the resume was the resumed session's own first
        /// hook.
        ///
        /// The two writes are one transaction on purpose. A resumed record is
        /// discoverable by supervision the instant its lifecycle goes live
        /// ([`supervision::discover`] filters on exactly that), so a live
        /// lifecycle and a stale identity must never both be readable, not even
        /// between two statements. Afterwards a resumed row is the same shape a
        /// created one is — live, with the identity of the Glasshouse responsible
        /// for it — and supervision reaches the same conclusions about it for the
        /// same reasons, which is the whole of the repair.
        ///
        /// Nothing about supervision is weakened. A resumed session whose process
        /// is genuinely gone is still found and still recorded `lost`; that is
        /// `a_resumed_session_whose_process_is_gone_is_still_lost` in
        /// `tests/session_supervision.rs`, reached against the identity this
        /// function wrote.
        ///
        /// `None` — a platform that will not name its processes — clears the
        /// columns rather than leaving the old values behind, for
        /// [`SessionStore::create`]'s reason: an unverifiable session is a real
        /// answer that supervision refuses to conclude anything from, and a stale
        /// identity is a wrong one it concludes a great deal from.
        ///
        /// [`Verdict::Gone`]: super::supervision::Verdict::Gone

### `session/store/mod.rs` — write_identity_locked

        /// Record the process a session is running in, replacing whatever was
        /// recorded before it.
        ///
        /// The write [`SessionStore::create`] makes as part of its `INSERT`, as an
        /// `UPDATE`, so that the other way a session becomes live can make it too.
        /// Callers must already hold a write transaction — the identity and the
        /// lifecycle it belongs to are one change, and a reader that could see
        /// half of it is the defect this exists to close.
        ///
        /// `supervision` is set to [`Supervision::Owned`] beside the identity, and
        /// the reason cleared, for the reason `create` gives: this Glasshouse is
        /// responsible for this process, and it is the only conclusion a writer
        /// that is not [`super::supervision::reconcile`] may reach. Leaving the
        /// previous conclusion would leave a sentence like *"its process (65061)
        /// is no longer running"* printed beside a session that is running, about
        /// a process the row no longer names.
        ///
        /// A `None` identity clears all four columns rather than half of them —
        /// [`SessionStore::supervision_of`] reads the three identity columns
        /// together or not at all, and a partially cleared row would be read as an
        /// identity built from whichever parts survived.

### `session/store/mod.rs` — require_owning_harness

    /// Refuse a session whose owner is not a real harness — line 646.
    ///
    /// # The catalogue is asked, not held
    ///
    /// The map's first fixed architectural requirement for this phase is that
    /// *every interactive Glasshouse session is owned by a real harness*, and
    /// line 646 names the failure it guards: a direct API provider or a gateway
    /// appearing in this table as though it were one.
    ///
    /// The question is answered by [`super::owning_harness`], one module up,
    /// because Phase 6 line 294 keeps adapter knowledge out of the session store
    /// and `harness::tests::the_session_model_depends_on_no_adapter` enforces it
    /// by scanning this file. That separation is right on its own terms: this
    /// module owns *what is recorded about a session* and has no business
    /// holding a list of harnesses, which grows.
    ///
    /// It is enforced **here** rather than at the caller because this is the only
    /// door. A guard in `main.rs` would be a guard `shell::start_session` does
    /// not have, and one any future caller could forget; a refusal in `create` is
    /// one no caller can bypass — the §35 shape, applied before the fact instead
    /// of after it.

### `session/store/claims.rs` — module doc

    //! Soft, project-scoped, turn-scoped file claims — capability map lines 2392
    //! to 2398, Phase 60's A+F slice.
    //!
    //! # What a claim is, and the four things it is not
    //!
    //! A claim is one row saying *"this Glasshouse session is working on this
    //! file, and still wanted it as of this second."* It is **coordination
    //! metadata**. Taking one never blocks, never locks, never changes a file's
    //! permissions, and never fails another session's write; two sessions may
    //! hold a claim on the same path, and that is the overlap a later package
    //! reports rather than an error raised here. Nothing in this build consults a
    //! claim before deciding anything.
    //!
    //! # It belongs to a session, never to a process
    //!
    //! Line 2396. The owner is a [`SessionId`], so a recycled process identifier
    //! can never resolve to a live claim — there is no process identifier here to
    //! recycle. A claim for a session this project does not have is refused
    //! before a row exists.
    //!
    //! # Project isolation
    //!
    //! Line 2397, and it holds three times over: the database file *is* the
    //! project, migration 27's two triggers refuse a row whose `project_id` is
    //! not the bound one, and every statement below also names `project_id`
    //! explicitly. A claim taken in one project cannot be named by a query in
    //! another.

### `session/store/claims.rs` — STALE_CLAIM_AFTER

    /// How long a claim nobody renewed stays active — line 2394's *"safe
    /// stale-claim timeout"*.
    ///
    /// # Why two hours
    ///
    /// This is a **backstop**, not the ordinary release path. A claim is normally
    /// released when the turn ends (`commands::hook`'s `TurnEnded` arm), and a
    /// claim whose owning session has stopped or failed is neither reported by
    /// [`SessionStore::active_claims`] nor kept, whatever the clock says. What is
    /// left for a timeout is the case both of those miss: a machine that lost power,
    /// or a harness killed hard enough that no hook ran and no lifecycle write
    /// landed.
    ///
    /// The two failure directions are not symmetric. Too short expires a claim
    /// under a session that is still editing, and the claim silently stops
    /// describing real work. Too long leaves a ghost that outlives the machine it
    /// was made on. Two hours is longer than any single harness turn — a turn is
    /// one prompt-to-stop cycle, minutes rather than hours, and a session working
    /// for longer than that renews as it goes — and short enough that a claim
    /// orphaned by a crash does not survive the working day. It is a judgement,
    /// not a measurement, and it is one constant so that changing it is one edit.

### `session/store/progress.rs` — module doc

    //! Declared task progress — capability map lines 1294 and 1610, the honest
    //! producer of
    //! [`crate::provider::quota::ReserveDecisionInputs::task_nearly_complete`].
    //!
    //! # What a declaration is, and the two things it is not
    //!
    //! A declaration is one row saying *"whoever is running this Glasshouse
    //! session says its current task is nearly complete, and still said so as of
    //! this second."* It is a **statement somebody made on purpose**, about one
    //! named session, that stops being true on its own.
    //!
    //! It is not an **inference**. Nothing in this build observes task progress:
    //! [`crate::events::LifecycleEvent`] is binary and retrospective, and the
    //! only completion fact available where the reserve verdict is computed is
    //! that a turn is already over. Every available proxy — a turn count, an
    //! elapsed time — reports "almost complete" for work that has merely been
    //! running a while, which is precisely the long-running work a protected
    //! reserve exists to keep serving. The field this feeds is the *first*
    //! branch [`crate::provider::quota::evaluate_reserve_spend`] takes,
    //! outranking every other signal including the user's own override, so a
    //! fabricated value there does not degrade the policy — it inverts it, at
    //! the one moment the protection matters.
    //!
    //! It is not a **setting**. A configuration value is sticky by nature, and a
    //! declaration that outlives the task it described re-creates that inversion
    //! by a slower route: the reserve would be permanently open on behalf of a
    //! task that finished last week. So the source is a row that expires, and
    //! the shape is [`super::claims`]'s — session-scoped, project-scoped, and a
    //! no-match by default, which is what makes its arrival a no-op for every
    //! caller that declares nothing.
    //!
    //! # Project isolation
    //!
    //! Migration 28's two triggers refuse a row whose `project_id` is not the
    //! bound one, every statement below also names `project_id` explicitly, and
    //! the database file *is* the project. A declaration made in one project
    //! cannot be named by a query in another.

### `session/store/progress.rs` — TASK_PROGRESS_EXPIRES_AFTER

    /// How long a declaration nobody renewed keeps protecting its session's work.
    ///
    /// # Why thirty minutes, and why shorter than a claim
    ///
    /// The two failure directions are **not symmetric, and they are not
    /// symmetric the other way round from [`super::STALE_CLAIM_AFTER`]**, which
    /// is why this is a second constant and not a reuse of that one.
    ///
    /// Expiring too early costs the operator the protection they asked for, and
    /// they get it back by declaring again. The behaviour it falls back to is
    /// exactly today's — the reserve decides on its own signals — so an early
    /// expiry is the *safe* direction.
    ///
    /// Expiring too late is the failure this whole design exists to prevent. A
    /// declaration left standing keeps forcing the first branch to `Allow` for
    /// whatever that session does next, which is a stale statement about a task
    /// that is gone being applied to a task nobody described. That is the
    /// inversion the design note refuses, arriving by the slower route.
    ///
    /// So the horizon points short. Thirty minutes is longer than a harness turn
    /// — a turn is one prompt-to-stop cycle, minutes rather than hours, and a
    /// session genuinely finishing a task renews as it goes — and short enough
    /// that a declaration somebody forgot cannot protect an unrelated later
    /// task. A task still not finished half an hour after somebody called it
    /// nearly complete was not nearly complete. It is a judgement, not a
    /// measurement, and it is one constant so that changing it is one edit.

## Trims: routing module docs — history moved out of comments by `GH-TRIM-ROUTING-DOCS`, 2026-09-05

### `routing/capability.rs` — module doc

    //! The capability registry — map line 1382: *"describe each harness and
    //! model resource with a small set of capabilities used for routing."*
    //!
    //! # Why this is not a second [`HardCapability`]
    //!
    //! [`super::classify::HardCapability`] states what a *task* needs.
    //! [`ResourceCapabilities`] describes what a *resource* can do. Merging the
    //! two into one scale would let a router compare a task's tier against its
    //! own tier and believe that proved something —
    //! `super::classify`'s own doc comment on line 79 already refuses this for
    //! the same reason. [`axis_for`] is the one comparison function that joins
    //! them; nothing else in this module or [`super::session`] collapses the
    //! two.
    //!
    //! # Why this is not a widening of `harness::Capabilities`
    //!
    //! Map line 1382 asks for "each harness **and model** resource". A harness
    //! adapter has no business declaring a model's context window or its
    //! price/speed class, so [`ResourceCapabilities`] is *built from*
    //! [`crate::harness::Capabilities`] plus [`ResourceFacts`] — a model/resource
    //! fact a harness adapter never sees — rather than being a bigger version of
    //! the adapter-declared type.
    //!
    //! # Why every axis is a [`Declared<bool>`]
    //!
    //! `Unverified` is not absent. `harness::Capabilities`' own tests pin that an
    //! unverified axis must never be scored as a `no`
    //! (`an_unverified_capability_is_not_treated_as_present`), and this registry
    //! carries the same rule forward: [`ResourceCapabilities::axis`] returns
    //! *established present*, *established absent*, or *not established* —
    //! never a bare bool.
    //!
    //! # 1390 — updatable without changing the core router
    //!
    //! [`super::session::capability_fit`] contains no `match` on a resource's
    //! identity and no capability values of its own; it only asks
    //! [`ResourceCapabilities::axis`] a question and applies a fixed scoring
    //! formula. To add a resource, correct an axis, or add a new model-level
    //! fact, construct or edit a [`ResourceFacts`] value — nothing in
    //! `session.rs` changes. `Destination::with_resource_facts` (`super::session`)
    //! is where a caller attaches one; the harness half comes from the adapter
    //! [`crate::harness::adapter_for`] already returns.

### `routing/classify.rs` — module doc

    //! Lightweight task classification — Phase 35.
    //!
    //! # What "lightweight" rules out
    //!
    //! The map's own preamble frames this as the thing Glasshouse asks *before*
    //! spending premium agent capacity — [`super::disposable::JobKind::Classification`]
    //! is already the name for that job in the disposable-routing policy class.
    //! A classifier that had to make a network call for every request would not
    //! be lightweight and could not "run on a cheap, free, or local model" in any
    //! meaningful sense, so this module makes none: [`classify_heuristically`] is
    //! a pure, deterministic function of the request text, and [`classify`]'s
    //! model path takes an *already-produced* [`TaskClassification`] as an
    //! argument rather than calling anything itself — the same discipline
    //! [`mod@super`]'s own doc comment states for the two routing-policy classes,
    //! extended to this one.
    //!
    //! # Nothing here decides which model does the classifying
    //!
    //! `crate::config::RoutingModelChoice` and `RoutingModelResolution` (Phase
    //! 2C) already record *whether* a routing model is configured and resolve it
    //! against the providers that exist. This module is downstream of that
    //! decision, not a duplicate of it: whatever calls a routing model is
    //! expected to turn its reply into a [`TaskClassification`] and hand it to
    //! [`classify`], and whatever finds no routing model configured falls
    //! through to [`classify_heuristically`] instead. Neither path is wired to a
    //! caller yet — see the module-level "no production caller" note in this
    //! phase's evidence entry.
    //!
    //! # Confidence is an escalation lever, not a report card
    //!
    //! Phase 35's line about escalating "uncertain tier assignments... conservatively"
    //! is answered by [`TaskClassification::conservative_workload_tier`] and
    //! [`TaskClassification::conservative_safe_for_disposable_model`], which never
    //! read better than the raw fields and only ever move in the direction of
    //! *more* capability or *less* trust — the same fail-closed shape
    //! [`super::Cost::Metered`] already uses as its default.

### `routing/classify.rs` — `WorkloadTier` doc

    /// The coarse workload tier a task requires — Phase 35's "assign a required
    /// workload tier to the task", widened to the map's five-tier system
    /// (capability map lines 1395-1400 and 1404).
    ///
    /// Ordered, so a policy may escalate by moving one step up
    /// ([`WorkloadTier::escalate`]) without a `match` of its own. This is
    /// deliberately not the same type as any future Phase 34F model-capability
    /// ceiling: a task's *requirement* and a model's *ceiling* are compared by a
    /// router, not merged into one enum, for the reason
    /// [`super::AssignedModel`]'s doc comment gives for keeping "no model" and "a
    /// named model" apart — collapsing a requirement and a capability into one
    /// scale would let a router compare a task's tier against its own tier and
    /// believe that proved something.
    ///
    /// [`Self::Deterministic`] (Tier 0) and [`Self::Frontier`] (Tier 4) have no
    /// producer yet: nothing in this module or its callers currently classifies
    /// a task into either. That is deliberate — this project adds a variant when
    /// its producer lands, never in advance (`src/evaluation/mod.rs:89` states
    /// the same rule for its own enum) — and every consumer of this type must
    /// stay exhaustive over all five so that the day a producer does exist, a
    /// missed call site is a compile error rather than a silent wrong decision.

### `routing/classify.rs` — `fn parse_classification`

    /// Read one model reply as a [`TaskClassification`] attributed to `label`.
    ///
    /// # Every classifying field is required, and nothing has a default
    ///
    /// A model that omits `workload_tier` has not classified the request, and a
    /// classification assembled around a default would be a fabrication wearing
    /// [`ClassificationSource::Model`] — indistinguishable, at every consumer
    /// downstream, from a tier the model actually chose. So this returns an error
    /// and the caller falls back to [`classify_heuristically`], which is honest
    /// about being a heuristic. That is the same direction
    /// [`crate::memory::extract::TokenUsage`]'s fields take for an unreported count.
    ///
    /// # The two recommendation fields are the exception, and why that is not a default
    ///
    /// `expected_duration` and `execution_shape` (map lines 1457 and 1458) are
    /// read when present and **`None` when absent or unrecognised** — never an
    /// error, and never a value stored as if the model had said it. `None` is
    /// its own fact ("the producer did not state one"), and every reader goes
    /// through [`TaskClassification::expected_duration`] and
    /// [`TaskClassification::expected_execution_shape`], which derive the answer
    /// from the ten fields the model *did* state. A reply from a model that
    /// predates the two keys therefore parses exactly as it always did, and a
    /// model that invents a fourth shape is read as having recommended nothing
    /// rather than as having failed to classify.
    ///
    /// `label` names the resource that answered, for
    /// [`ClassificationSource::Model`]. It is the caller's own description of a
    /// model it configured — a provider name, a model name and a route — and
    /// never anything the reply said.

### `routing/disposable/classification.rs` — `fn classification_verdict`

    /// Decide whether `candidate` may be asked to classify — the four filters
    /// capability map lines 1427, 1436, 1432 and 1435 name, in that order.
    ///
    /// # The honesty rule, and the one place it is deliberately inverted
    ///
    /// Reliability and latency are **measurements**, and a measurement that has
    /// not been taken never excludes: a candidate with no record, or with fewer
    /// than [`CLASSIFICATION_RELIABILITY_MIN_OBSERVATIONS`] outcomes, or with no
    /// median yet, is admitted with a note saying the requirement was inert —
    /// the same rule [`has_no_known_headroom`] applies to capacity, because
    /// turning "nothing measured" into "fails the bar" is a fabrication. Price
    /// is the same shape once more: a metered candidate with no entry in
    /// `pricing.toml` is *unpriced*, never excluded by the ceiling, exactly
    /// like an unmeasured latency.
    ///
    /// Locality is **not a measurement**: it is a fact the provider registry
    /// states for every provider name, and a caller that attaches none has
    /// declined to say rather than failed to measure. Under
    /// [`ClassificationPolicy::local_only`] that fails **closed** — a candidate
    /// nobody could say is local is not sent anything — because this is a
    /// privacy constraint, and a privacy constraint that admits on silence
    /// would send a request off the machine on the strength of nobody having
    /// said where the model runs.

### `routing/disposable/classification.rs` — `fn time_price_preference`

    /// Map line 1439 — `design-decisions.md`'s *"Preferring a cheap metered
    /// classifier over an unreliable free one"*, amended 2026-09-02: prefer
    /// `metered` over `free` when `free`'s expected wasted retry time — `(1 -
    /// parsed_fraction) * median_ms` over its own classification record, above
    /// the reliability sample floor — **exceeds `metered`'s own median
    /// classification latency**, also above the floor and read from `metered`'s
    /// own [`ClassificationRecord`], and `metered`'s estimated call cost is at
    /// or below `policy`'s marginal-cost ceiling. `[routing] max_router_latency`
    /// plays no part here — that knob stays 1435's alone. No exchange rate
    /// between milliseconds and micro-dollars exists here or anywhere else in
    /// this build: the comparison this rule actually makes is between two
    /// *times*, and the cost half is still checked only against a ceiling the
    /// user stated in their own currency.
    ///
    /// # Why the first version of this rule (comparing against `max_router_latency`) was withdrawn
    ///
    /// `free`'s expected wasted time is at most `median_ms`, for any parsed
    /// fraction in `[0, 1]`. Comparing that wasted time against the same
    /// `max_router_latency` [`classification_verdict`]'s 1435 gate excludes on
    /// meant this rule could only ever fire on a candidate 1435 (and, below the
    /// 80% floor, 1432) had already excluded from
    /// [`DisposableRouting::choose_for_automatic_classification`]'s admitted
    /// list — an account of an exclusion, never a preference that could change a
    /// choice. Comparing `free`'s wasted time against `metered`'s **own**
    /// measured latency has no such relationship to either gate: both times come
    /// from candidates that pass 1432/1435/1436 on their own terms, so this rule
    /// can fire on a candidate the router was genuinely about to choose.

### `routing/disposable/mod.rs` — module doc

    //! Routing for bounded internal jobs — the second policy class (Phase 9I).
    //!
    //! # What a disposable job is, and why it does not share a router
    //!
    //! A disposable job is a bounded, non-conversational request Glasshouse makes
    //! for its own purposes: classifying a request before spending premium agent
    //! capacity, extracting memories from a finished session, reranking search
    //! results. Phase 9I line 530 names those three.
    //!
    //! Nothing about them resembles a live coding session. They have no
    //! conversation prefix worth keeping warm, no tools, no user watching a
    //! cursor, and no cost to being served by a different model than the last one
    //! was. Line 533 therefore asks that they be routed by a **separate policy
    //! class**, and the module header of [`mod@super`] lists the three
    //! independent ways that separation is made structural here.
    //!
    //! The practical content of the separation is one sentence: this policy
    //! **prefers free capacity and re-decides every time**, and the interactive
    //! policy **keeps what it has and re-decides only after a real failure**.
    //!
    //! # Glasshouse's own test and evaluation runs
    //!
    //! Phase 9I line 539 — *"allow Glasshouse's own automated evaluation and test
    //! runs to use configured zero-cost models, and never a metered resource
    //! without an explicit opt-in"* — is an acceptance condition, not a
    //! preference. A test run that silently spends the user's money is the worst
    //! outcome this module can produce, and it is worse than a failing test.
    //!
    //! It is enforced by construction rather than by a check a caller might
    //! forget: a routing policy is built with a [`MeteredUse`], the value that
    //! Glasshouse's own runs are built with is [`MeteredUse::Withheld`], and a
    //! [`DisposableChoice`] on a metered resource cannot be produced from a
    //! policy holding it. There is no second door — [`DisposableChoice`]'s fields
    //! are private and nothing else in the crate constructs one.

### `routing/domain.rs` — module doc

    //! Two domains a failover candidate belongs to, and why they are two types —
    //! Phase 33C line 1371.
    //!
    //! # A quota domain needs no new type
    //!
    //! "Two keys for one router are two allowances" is [`super::free`]'s own
    //! module header, and its pool is already keyed by [`super::CredentialId`].
    //! A quota domain **is** a [`super::CredentialId`]: two different
    //! credentials are two different quota domains by that type's own
    //! `PartialEq`, and nothing here wraps it in a second type that would only
    //! ever compare the same way a `CredentialId` already does.
    //!
    //! # A failure domain is a different question, with a different shape
    //!
    //! [`super::Backend`] carries no base URL — see that type's own doc comment
    //! on why it is deliberately narrower than [`crate::provider::Provider`] —
    //! so the only honest signal available for "does this request land on the
    //! same infrastructure" is the provider name. That signal answers "yes" with
    //! certainty and answers "no" with **no certainty at all**: a different
    //! provider is not evidence of a different failure domain, only the absence
    //! of evidence that it is the same one. [`FailureDomain`] is three states
    //! rather than a bool for exactly that reason — line 1371's "represent...
    //! separately" and line 1378's "prevent absent evidence from being
    //! interpreted as independence".

### `routing/evidence/joins.rs` — `fn estimate_subscription_headroom`

    /// Map line 1245's estimator, and lines 1244/1246/1250/1251/1254 with it —
    /// see [`SubscriptionHeadroomEstimate`] and [`HeadroomBand`] for the type's
    /// own honesty rules. No new table, no migration, no persisted estimator
    /// state: every call re-derives the estimate from rows the caller already
    /// holds, the same "today's history IS the ledger's own rows in window"
    /// premise every other reader in this module keeps.
    ///
    /// Reads five things, none of them queried here:
    ///
    /// - **accepted-request counts** and **throttle events with their
    ///   recency**, from `observations` — this provider's own informative rows
    ///   (`outcome.is_some()`, excluding [`CORRELATION_PURPOSE`] rows, the same
    ///   filter [`classify_throttle_scope`] applies), narrowed to
    ///   `credential_label` by the widen-when-unsure rule
    ///   [`recent_credential_throttles`] and [`recent_credential_spend`] already
    ///   apply: only when **every** informative row names its account does the
    ///   count narrow, and one contextless row widens the whole estimate to
    ///   provider scope rather than silently dropping it (map line 1246);
    /// - **token usage where rows carry it** — never turned into a figure, only
    ///   recorded on the returned value's [`HeadroomBasis`] (line 1251);
    /// - **reset behavior**, as `seconds_until_reset` — the caller's own
    ///   gateway-quota-cache reading, already computed for the provider-wide
    ///   capacity facet and handed in rather than re-read. Map line 1248: when
    ///   this is `None`, a fallback is learned from `scoped`'s own
    ///   throttle→success recoveries rather than left unused — see
    ///   [`ResetBasis`]. The learned value never displaces a real reading;
    ///   [`SubscriptionHeadroomEstimate::reset_basis`] says which one applied;
    /// - **historical sessions**, as `recent_session_count` — this project's own
    ///   count of sessions charged to this account (`sessions.entitlement`,
    ///   migration 22), read by the caller and handed in: this function stays a
    ///   pure read over values already fetched, the shape every other reader in
    ///   this module keeps.
    ///
    /// `None` — unknown — when nothing at all is available: no informative row,
    /// no session count, no reset reading. An account this genuinely unmeasured
    /// is not "exhausted" and not "ample"; it is unmeasured, the 32B line-1239
    /// discipline every other facet on `ResolvedEntitlement` already keeps.

### `routing/evidence/joins.rs` — `fn estimate_pairs`

    /// **Map line 1855, the token half.** Joins each
    /// [`crate::evaluation::EvaluationKind::RoutingConsumptionEstimated`]
    /// row in the window to the sum of `output_tokens` over this project's
    /// own routing rows carrying the same `session_id`, at or after the
    /// estimate row's own `observed_at` — the actual consumption the
    /// launch's estimate was a prediction *of*.
    ///
    /// # Why this reads across two tables from one connection
    ///
    /// [`crate::evaluation::EvaluationObservations`] and this ledger wrap
    /// separate [`Connection`]s onto the **same** project database file —
    /// [`Self::effort_shadow`] already reads `evaluation_observations` this
    /// way, correlating a routing row to the session's next harness verdict.
    /// This reader is that precedent's mirror image: here
    /// `evaluation_observations` (kind
    /// [`crate::evaluation::EvaluationKind::RoutingConsumptionEstimated`])
    /// is the driving table and `routing_observations` is summed per match,
    /// but it is the same one-file, one-query join, scoped by
    /// [`Self::project_id`] on both sides rather than opening a second
    /// ledger handle for a read this connection can already serve.
    ///
    /// A session with an estimate row and **no** matching routing row (or
    /// one whose `output_tokens` are all still `NULL`) has an unknown
    /// actual, never a fabricated zero — [`Self::consumption_by_purpose`]'s
    /// own rule for an absent sum. [`Self::output_estimate_accuracy`]
    /// reports it as *pending*.

### `routing/evidence/joins.rs` — `fn responsiveness_separation`

    /// Capability map line 1850: whether effective TTFC separates usable
    /// agent turns from unusable ones better than raw TTFC, TTFT or decode
    /// tokens per second — one [`SeparationMeasure`] per figure.
    ///
    /// Scoped to [`HARNESS_TURN_PURPOSE`] rows, the same restriction
    /// [`Self::translation_cache_savings`] applies for the same reason: only
    /// a translated exchange ever carries `first_tool_call_ms`,
    /// `first_token_ms` or a tool round to measure at all. The usable-turn
    /// verdict is [`Self::effort_shadow`]'s own subquery — the session's next
    /// [`crate::evaluation::EvaluationKind::TurnOutcomeObserved`] row at or
    /// after the exchange — never [`RoutingObservation::outcome`], which is a
    /// transport 2xx proxy and not a verdict (see that field's own doc
    /// comment). An exchange whose session recorded no such row is excluded
    /// from every measure here, not folded into either side.
    ///
    /// **Effective TTFC is attached per row from its own route**, not
    /// computed per exchange: a single exchange carries no failure rate of
    /// its own, so each row's contribution to that measure is its
    /// `(provider, model)`'s [`RouteResponsiveness::effective_ttfc_ms`] over
    /// this same window, computed once per route and read off for every row
    /// that route served. Raw TTFC, TTFT and decode tokens/s are each row's
    /// own figure.

### `routing/evidence/joins.rs` — `fn effort_shadow`

    /// [`EffortShadow`] — capability map line 2039's shadow measurement: per
    /// translated exchange this build recorded a turn shape and an
    /// output-token count for, whether the exchange's session's next
    /// harness-reported verdict was a completion, a failure, or nothing at
    /// all. Joined by migration 24's `session_id`, **never** by
    /// [`RoutingObservation::outcome`] — a transport 2xx proxy, not a
    /// verdict, per that field's own doc comment.
    ///
    /// **Two statements, not one.** The verdict is *the session's next
    /// [`crate::evaluation::EvaluationKind::TurnOutcomeObserved`] row at or
    /// after the exchange's `observed_at`* — a correlated "first row at or
    /// after" lookup expressed as a scalar subquery per candidate row — and
    /// this reader's median is computed in Rust from the raw sample the way
    /// every other median on this ledger is, so the classified
    /// rows are fetched flat, with the verdict subquery inline, and folded
    /// here rather than in a single `GROUP BY`. [`EffortShadow::unread`] is a
    /// second, simpler statement over the same window and purpose: a row
    /// whose `turn_shape` this reader could not decode is never folded into
    /// either turn shape, so its count comes from a query that does not
    /// filter on `output_tokens` at all — an unread row's tokens are unread
    /// for the same reason its shape is.
    ///
    /// Only [`HARNESS_TURN_PURPOSE`] rows with `output_tokens IS NOT NULL`
    /// enter a group.

### `routing/evidence/readers.rs` — `struct PurposeConsumption` doc

    /// Request and token consumption for one `(purpose, harness_recorded)`
    /// group, within a queried window — capability map line 1464's "measure
    /// routing-model token and request consumption separately from coding-agent
    /// consumption," and the absent aggregate
    /// [`EvidenceLedger::consumption_by_purpose`] builds: every other reader on
    /// this ledger requires the caller to already name an identity, and nothing
    /// before this grouped by the columns that answer *what a call was for* and
    /// *whether a harness was relaying it*.
    ///
    /// `purpose` alone is not enough to separate coding-agent consumption from
    /// everything else: `purpose` is `None` for every row no producer has
    /// stamped, and today that is **both** every gateway relay exchange (line
    /// 1464's own "coding-agent consumption", `crate::gateway::session`, which
    /// always calls [`NewObservation::with_harness`]) **and** every
    /// memory-extraction call (`crate::memory::extract::ModelCall::observation`,
    /// which never does) — see [`NewObservation::with_purpose`]'s doc comment
    /// for why extraction's rows are not back-filled with one. `harness_recorded`
    /// is what tells those two `NULL`-purpose producers apart: `true` only when
    /// every row in the group named a harness, which today means gateway rows
    /// and gateway rows alone.
    ///
    /// `sample_count` is a real `COUNT(*)`, always defined. The three token
    /// fields are not: each is `None` when every row in the group left that
    /// column `NULL`, which is a different fact from `Some(0)` and must stay
    /// one — the hazard this whole aggregate exists to avoid rendering as a
    /// number. A group that mixes counted and uncounted rows sums only what was
    /// counted, exactly as [`NewObservation::with_tokens`] asks every producer to
    /// leave absent counts absent rather than zeroed.

### `routing/evidence/readers.rs` — `struct ClassificationRecord` doc

    /// What this project's ledger holds about one `(provider, model)` **as a
    /// routing-model classifier** — capability map lines 1422/1432 (does it
    /// come back in the schema?) and 1421/1435 (how long does it take?) — read
    /// from the [`CLASSIFICATION_PURPOSE`] rows alone.
    ///
    /// Two counts and one median, each carrying its own denominator:
    ///
    /// - `outcomes_recorded` is the number of rows that carry a parse outcome
    ///   at all — [`Outcome::Succeeded`] or [`Outcome::Failed`] — and `parsed`
    ///   is how many of those succeeded. A row with no outcome (written by a
    ///   build before the producer recorded one) counts in neither: it is not
    ///   evidence of reliability in either direction.
    /// - `timed` is how many rows carry a duration, and `median_duration_ms`
    ///   is their median **only** once there are at least
    ///   [`MIN_SAMPLE_FOR_SUMMARY`] of them — the same floor every other figure
    ///   on this ledger sits behind. Below it the field is `None`, which a
    ///   consumer must read as *unmeasured*, never as fast.
    ///
    /// **Resolution is one second.** `dispatched_at` and `completed_at` are
    /// whole Unix seconds (this module's header, on line 1332's gap), so every
    /// duration here is a multiple of 1000ms, and a ceiling compared against
    /// this median is honest only to the second.
    ///
    /// Not split by [`ContextState`]: a classification call is a fresh prompt
    /// every time with nothing warm to keep, and its producer records
    /// [`ContextState::Unknown`] on every row.

### `routing/evidence/readers.rs` — `struct RoutingOverhead` doc

    /// Routing-model spend set against everything else — capability map line
    /// 1465 — as one pure reading over
    /// [`EvidenceLedger::consumption_by_purpose`]'s groups, so the arithmetic is
    /// testable without a database and is rendered with its denominators rather
    /// than as a bare ratio.
    ///
    /// "Spend" is **tokens**, input plus output as the provider reported them,
    /// because that is still the only currency this reading can rely on:
    /// `cost_micro_usd` has one producer (map line 1307,
    /// `main.rs::record_entitlement_fallback`), and it fires only on an
    /// entitlement-fallback event — coding-agent spend routed through the
    /// gateway relay, the volume this comparison exists to weigh, leaves the
    /// column `NULL` exactly as before. Cached input tokens are left out of the
    /// sum — providers disagree on whether they are already inside
    /// `input_tokens`, and a sum that might double-count is worse than one that
    /// names what it omits.
    ///
    /// A `None` token figure means *no row in that side carried a count*, the
    /// same convention [`PurposeConsumption`] keeps; a side that mixes counted
    /// and uncounted rows sums only what was counted. [`Self::fraction`] is
    /// `None` whenever either side is uncounted or the task side is zero, and
    /// [`Self::exceeds`] never fires on an unmeasured comparison.

### `routing/evidence/readers.rs` — `fn consumption_in_window`

    /// Every observation in the window ending at `now_unix`, **whether or
    /// not it carries an outcome** — the row set a *consumption* reader
    /// needs, and the one [`Self::observations_in_window`] deliberately
    /// cannot serve.
    ///
    /// # Why this is not `observations_in_window` with a flag
    ///
    /// [`Self::observations_in_window`] filters `outcome IS NOT NULL`
    /// because its callers classify *how exchanges went* — a throttle scope,
    /// a route correlation, a failure-class census — and a row with no
    /// recorded outcome is not evidence about that question.
    ///
    /// Capability map lines 1274 and 1276 ask a different question: how much
    /// of a resource was **consumed**. A request whose outcome nobody wrote
    /// down still consumed the request. And the one producer that carries a
    /// task class today — `main.rs::record_routing_latency`, which is the
    /// only caller holding a `crate::routing::request::RouterAnswer` — records no
    /// outcome at all, so every row line 1276 is about is invisible to the
    /// other read. Widening that read instead would silently change what
    /// four existing classifiers count, which is the opposite of what a new
    /// line is allowed to do.
    ///
    /// Ordered by `observed_at` ascending, like its sibling, because
    /// [`crate::routing::burn`] buckets by time and an idle gap is a property of
    /// consecutive rows.

### `routing/evidence/readers.rs` — `fn consumption_by_purpose`

    /// [`PurposeConsumption`] for every `(purpose, harness_recorded)` group
    /// this ledger holds a row for, within one window — capability map line
    /// 1464, and the aggregate this module's own header says nothing
    /// computes yet.
    ///
    /// Grouped by `purpose` first, so a routing model's own spend (`purpose
    /// = "classification"` today) never gets folded into anyone else's
    /// total; and, within the `NULL`-purpose rows every other producer
    /// leaves, split again by whether a harness was recorded, because that
    /// is what actually separates coding-agent consumption
    /// (`crate::gateway::session` always names a harness) from every other
    /// `NULL`-purpose producer (`crate::memory::extract` never does) — a
    /// distinction `purpose` alone cannot make. See [`PurposeConsumption`]'s
    /// own doc comment for why grouping on `purpose` alone would still fold
    /// two different producers together.
    ///
    /// `SUM(input_tokens)`, and its two siblings, are what SQLite's own
    /// aggregate already does correctly: it skips `NULL` inputs and answers
    /// `NULL` only when a group carried none at all, never `0` for an absent
    /// count. The row reader reads that straight into the `Option<i64>`
    /// [`PurposeConsumption`] declares, with no manual accumulate-and-default
    /// in between for a mutation to weaken.
    ///
    /// `first_byte_sample_count` is a genuine `COUNT(first_byte_at)`, so it
    /// is honestly `0` — not absent — for a group nothing timed, and
    /// `first_byte_ms_sample_count` is the same count over migration 25's
    /// measured offset. `mean_time_to_first_byte_ms` **prefers the offset**:
    /// each row contributes its own `first_byte_ms` when it has one and its
    /// `first_byte_at - dispatched_at` difference in milliseconds when it
    /// does not, so a window spanning the migration produces one mean over
    /// every timed row rather than two incomparable ones. It is `NULL`
    /// (`None`) exactly when no row offered either — SQLite's `AVG` over an
    /// empty set is already `NULL`, so there is no manual zero-guard here.
    /// `first_token_*`/`first_tool_call_*` are the identical triple.
    ///
    /// `decode_output_tokens` and `decode_ms` are line 1349's matched pair,
    /// summed over exactly the rows carrying `output_tokens`,
    /// `first_token_ms` and `completed_ms` with a non-negative gap — the one
    /// figure here with **no** seconds fallback, because at one-second
    /// resolution its denominator is routinely `0`. See
    /// [`PurposeConsumption::decode_tokens_per_second`].
    ///
    /// Scoped to this ledger's own `project_id`, like [`Self::observed_identities`]
    /// next door and for the same belt-and-suspenders reason: this reads
    /// across every row in the table rather than one already-named identity.

### `routing/evidence/signals.rs` — `struct RouteCorrelation` doc

    /// What this project's ledger has observed about whether two routes fail
    /// together — capability map lines 1370, 1373, 1374 and 1376, as one value.
    ///
    /// # What is counted
    ///
    /// An **informative failure event** is a correlatable failure
    /// ([`FailureClass::is_correlatable`]) on one route during which the other
    /// route was *observed at all* — had an exchange with a recorded outcome
    /// whose window overlaps the failure's within
    /// [`CORRELATION_OVERLAP_TOLERANCE_SECONDS`]. A failure while the other
    /// route was idle says nothing about the pair and is counted nowhere: line
    /// 1370's "measured, never assumed" cuts both ways, and treating an
    /// unobserved route as having survived would manufacture independence.
    ///
    /// Of the informative events, `overlaps` are those where the other route
    /// failed with the **same class** inside the tolerance, and `lone` are those
    /// where it was observed and did not. Each failure event is matched at most
    /// once, so a burst of five on each side is ten events and not twenty-five
    /// pairs.
    ///
    /// # Why the confidence moves both ways (line 1374)
    ///
    /// [`Self::confidence`] is `overlaps / (overlaps + lone)`. A new overlapping
    /// failure raises it; a new failure the other route sat out lowers it.
    /// Nothing here is a stored flag: the value is recomputed from the rows on
    /// every read and never persisted, because the rows are the claim and the
    /// rows keep arriving.

### `routing/evidence/signals.rs` — `enum ThrottleScope` doc

    /// Capability map line 1317: whether a throttle on one route reads as this
    /// provider's own cadence limiter firing everywhere, or as one model's own
    /// limit — computed, never stored, from the same rows and the same overlap
    /// [`correlate_routes`] measures, restricted to [`FailureClass::Throttle`]
    /// and to one provider's own models rather than every route in the ledger.
    ///
    /// # One of the map line's four scopes is still not here
    ///
    /// Line 1317 names four: provider-wide, model-specific, account-specific,
    /// request-pool-specific. Three now have a producer in this build.
    /// **Account-specific** gained its key with Phase 56A: every gateway
    /// exchange row carries the serving credential's label in
    /// [`RoutingObservation::quota_context`]
    /// (`crate::gateway::session` stamps `credential().label()` on every
    /// observation), so a second account of one provider is now something the
    /// rows can tell apart — the earlier note here that *"no row carries an
    /// account identity"* described the build before that column had its
    /// producer. The variant is still emitted only when the evidence permits:
    /// rows without a `quota_context` contribute nothing to it, and a ledger
    /// with one account's rows classifies exactly as it always did.
    /// **Request-pool-specific** still has neither a producer nor a consumer:
    /// `routing::free::is_request_pool` has no production caller, and the one
    /// production allowance read asks only `is_exhausted`, which a pooled and a
    /// token-priced credential both answer the same way (refusal register, row
    /// 531). Fabricating it would be exactly the invention line 1317's own
    /// "when evidence permits" refuses.

### `routing/evidence/signals.rs` — `struct CredentialSpend` doc

    /// Token spend recorded against one account inside a queried window — map
    /// line 1971's *"spend ceilings"* half, read from the rows this ledger
    /// actually holds.
    ///
    /// # Why tokens, and why that is not this reader's own decision
    ///
    /// `routing_observations.cost_micro_usd` has one producer now — map line
    /// 1307, `main.rs::record_entitlement_fallback`, carrying
    /// [`crate::routing::session::Routed::cost`] — but it writes only on an
    /// entitlement-fallback event, at [`CostConfidence::Estimated`] built from
    /// the user's own `pricing.toml` rather than a provider-reported figure. The
    /// overwhelming majority of rows still leave the column `NULL`, so a reader
    /// here that answered in money would answer `None` for nearly every window,
    /// and a ceiling that can almost never be reached is a rule the broker can
    /// almost never be held to. Map line 1465's reader already settled the same
    /// question the same way, in production, in [`RoutingOverhead`]'s own
    /// words: *"'Spend' is tokens, input plus output as the provider reported
    /// them, because that is the only currency this ledger holds."* This reader
    /// is that sentence applied per account. Cached input tokens are excluded
    /// for line 1465's reason too: providers disagree on whether they are
    /// already inside `input_tokens`, and a sum that might double-count is worse
    /// than one that names what it omits.

### `routing/free.rs` — module doc

    //! The free pool: which zero-cost resources exist, what is left of each, and
    //! which of them is currently able to serve.
    //!
    //! # Health comes from work, never from a probe
    //!
    //! Phase 9I line 534 asks Glasshouse to *"avoid consuming scarce free
    //! requests on health probes when actual workload can provide health
    //! signals"*. A health checker that burns the quota it is protecting is a
    //! defect with a passing test, so this module is built so that one cannot be
    //! written here: [`FreePool::observe`] is the **only** thing that changes a
    //! resource's health, it takes a [`WorkloadOutcome`] that a real exchange
    //! produced, and there is no client, no socket and no timer anywhere in this
    //! file — `routing::tests::no_routing_policy_can_make_a_request` scans for
    //! that.
    //!
    //! The production feed is the gateway's own request path: every exchange it
    //! completes already knows the credential it used, the status the provider
    //! returned and whether it reached the provider at all.
    //!
    //! # A request pool is not a token budget
    //!
    //! Phase 9I line 528 — *"track request-pool limits separately from
    //! token-priced limits"*. [`Allowance`] has one variant for each and no
    //! shared arithmetic, because the failure mode of collapsing them is
    //! specific and quiet: a token budget decremented by one per request reads as
    //! healthy for a very long time and then is not.
    //!
    //! What a request pool holds is what a **real response said** — a limit, a
    //! remaining count and a reset instant, each `None` until a provider actually
    //! stated it. Glasshouse defines no window of its own. A guessed window is
    //! how a router talks itself into believing a pool has refilled.
    //!
    //! # Per credential, because two keys are two allowances
    //!
    //! Phase 9I lines 537 and 538. Allowance state is keyed by [`CredentialId`]
    //! and health by credential **and** model, so exhausting one key says nothing
    //! about the other key, and exhausting one model says nothing about the
    //! others behind the same key. Keying either of these by provider is the
    //! mistake the two lines exist to name; `crates/glasshouse/tests/` carries
    //! the test that fails when it is made.

### `routing/free.rs` — `fn fail`

    /// One rate-limit or capacity failure — the two outcomes Phase 9I line
    /// 535 names — and the cooldown that follows.
    ///
    /// **A cooldown a provider declared and one Glasshouse invented are not
    /// the same kind of fact.** Capability map line 1319 makes the provider's
    /// own answer *authoritative* for a temporary scheduling block, not
    /// merely preferred, so the two take different paths here:
    ///
    /// - **A declared `retry_after` applies as given, and immediately.**
    ///   [`FAILURES_BEFORE_COOLDOWN`] exists because *inventing* a cooldown
    ///   out of one ordinary `429` would empty a pool of perfectly good
    ///   resources; nothing is invented when the provider stated the wait
    ///   itself, and scheduling work against a resource that just told us to
    ///   hold is exactly the block line 1319 forbids. [`MAX_COOLDOWN`] does
    ///   not apply either — it bounds what Glasshouse imposes *by itself*
    ///   (see its own doc), never what a provider declared. Clamping a stated
    ///   one-hour wait down to fifteen minutes is overriding the provider,
    ///   which is the whole of what this line rules out.
    /// - **Without one, the bounded doubling from [`BASE_COOLDOWN`] applies**,
    ///   and only once there have been [`FAILURES_BEFORE_COOLDOWN`] of them.
    ///   [`Self::backoff`] applies [`MAX_COOLDOWN`] itself, so the ceiling on
    ///   the invented path is unchanged by the split.
    ///
    /// A declared wait that is *shorter* than a cooldown already in place
    /// shortens it, for the same reason: authoritative means authoritative in
    /// both directions, and it is the same rule that lets
    /// [`WorkloadOutcome::Served`] clear a cooldown outright.

### `routing/free.rs` — `fn adopt_observed`

    /// Adopt a health state **another process** already observed about
    /// `resource` — capability map line 1599's bridge, and the only entry
    /// point that does not learn.
    ///
    /// # Why this is not [`FreePool::observe`]
    ///
    /// `observe` takes one outcome and derives the rest: it counts the
    /// failure, and it computes the cooldown itself from `BASE_COOLDOWN` or
    /// the provider's stated `retry_after`. There is no outcome here to
    /// derive anything from. A caller holding a persisted reading knows the
    /// failure count and the deadline as *facts already established*, and
    /// replaying them through `observe` would manufacture a cooldown length
    /// this pool invented rather than the one the gateway actually granted.

### `routing/interactive/mod.rs` — module doc

    //! Sticky routing for one live harness-backed gateway session (Phase 9H).
    //!
    //! # The session owns the assignment; the assignment is not a session
    //!
    //! Phase 9H line 507 asks Glasshouse to *"treat the gateway assignment as
    //! backend state belonging to the harness-backed session rather than as an
    //! independent agent session"*. [`Assignment`] is therefore a value with no
    //! identity of its own: no session id, no lifecycle, no start or end, nothing
    //! that could be listed beside the user's real sessions. It is held by the
    //! gateway a session started and dies with it.
    //!
    //! That is structural, not promised. Nothing in this file names
    //! `crate::session`, and `tests::the_assignment_is_not_a_session_of_its_own`
    //! scans for it — the same move `gateway::mod` already makes for the same
    //! reason, and for the same product principle: the harness stays the harness.
    //!
    //! # Sticky means *nothing on a normal turn asks the question*
    //!
    //! Lines 508 and 509 are two halves of one behaviour, and the second is the
    //! one that is easy to lose. It is not enough that a normal turn happens to
    //! keep the same backend; a normal turn must keep it **even when a cheaper
    //! free model is sitting right there**. So [`InteractiveRouting::next_turn`]
    //! takes the alternatives as an argument. A version of this function that
    //! could not see them would satisfy the line by accident, and the first
    //! optimisation someone added would break it silently.
    //!
    //! # A failover is not a migration, and the difference is decidable today
    //!
    //! Lines 513 and 514 ask for same-family failover to be preferred and a
    //! *material* model-family change to be treated as a migration decision.
    //! Rather than invent a taxonomy by pattern-matching model names, this module
    //! uses the conservative rule the available facts support:
    //!
    //! - **the same model identifier served by a different provider** is a
    //!   same-family move — it is literally the same model, which is the common
    //!   real case (one model offered by two routers) — so it is a
    //!   [`FailureResponse::FailOver`];
    //! - **any different model identifier** is treated as material, so it is
    //!   offered as a migration and never taken transparently.
    //!
    //! Erring this way costs an automatic recovery that a family table would have
    //! allowed. Erring the other way silently changes the model under a live
    //! coding session, which is exactly what line 514 forbids.
    //!
    //! # Phase 9J and Phase 33A rank the survivors; they do not choose the group
    //!
    //! [`InteractiveRouting::on_provider_failure`] used to take the first
    //! candidate in each group (same-model, then different-model) and return
    //! immediately — the ordering above is unaffected by anything below this
    //! paragraph. What changed is *which* candidate wins **within** a group,
    //! once more than one survives `compatible`: `score_candidate` classifies
    //! each one against the harness the failing session was serving
    //! ([`pairing::classify`], Phase 9J) and weighs it against local observed
    //! evidence for that exact `(provider, model, route, harness)` combination
    //! (`crate::routing::evidence::ObservedEvidenceSource`, Phase 33A), and
    //! `best` picks the highest-scoring survivor, the caller's own order
    //! breaking a tie. A candidate can never be *excluded* this way — only
    //! `compatible` refuses one — so this is design decision 1's "additive,
    //! never a filter" made literal for this policy's own decision.
    //!
    //! Every candidate also carries a **failure-domain diversity** contribution
    //! (Phase 33C, `failure_domain_contribution`): a candidate sharing the
    //! failed backend's provider is penalised, because `Backend` carries no base
    //! URL and the provider is the only honest proxy this build has for "lands
    //! on the same infrastructure" (see [`super::domain::FailureDomain`]). A
    //! different provider scores `0.0` rather than a bonus — line 1378 forbids
    //! rewarding a candidate for independence nothing has established.

### `routing/interactive/mod.rs` — `fn start`

    /// Map lines 566 and 569: which of several eligible backends a **fresh**
    /// session starts on, and the full explanation of why.
    ///
    /// [`Self::assign`] above is the older, narrower entry point: the caller
    /// had already chosen, and `assign` recorded the choice. This one is the
    /// caller *asking*. It exists because line 566 asks for a positive
    /// **initial** routing prior — "initial" is a moment, and until this
    /// function there was no moment in Glasshouse where a starting session
    /// compared two backends. `crate::gateway::session::SessionRouting::bind`
    /// took `Upstream::serving()`, the first configured backend, and its own
    /// doc said so: *"Nothing here chooses; the choice was made when the
    /// upstream was built."*
    ///
    /// # What is weighed, and in what order
    ///
    /// 1. **Hard constraints first** (line 568), through
    ///    [`apply_hard_constraints`] and therefore structurally rather than
    ///    by convention: a session pin is a
    ///    [`HardConstraint::UserConstraint`] and removes a candidate outright.
    ///    Unlike `score_candidate`'s own receipt-shaped call, this check can
    ///    actually reject, so the [`EligibleCandidate`](crate::routing::EligibleCandidate)s
    ///    below are a filter's output and not a formality.
    /// 2. **The native-pairing prior** (line 566) and **local observed
    ///    evidence** (Phase 33A), from `score_candidate` — the same function,
    ///    unchanged, that [`Self::on_provider_failure`] already scores
    ///    failover survivors with.
    /// 3. **Session continuity** (line 569), from
    ///    [`session_continuity_contribution`] — bounded, never negative, and
    ///    on the prior's own scale so that `best` can weigh the two against
    ///    each other by simple sum. That is what "commensurable" has to mean
    ///    here: not that a warm session is compared to a prior by a special
    ///    rule, but that neither term knows the other exists and
    ///    [`RoutingExplanation::total`] adds them up.
    ///
    /// A candidate is never *excluded* by any of steps 2 or 3 — only step 1
    /// can remove one, and only for a constraint the user or the protocol
    /// imposed. That is design decision 1 ("additive, never a filter") at the
    /// one caller where a prior could most easily have been written as
    /// `if native { return it }`.
    ///
    /// # What a build with nothing configured does
    ///
    /// Exactly what it did before this function existed. With
    /// [`NoObservations`](crate::config::pairing::NoObservations),
    /// [`NoWarmSessions`](crate::config::pairing::NoWarmSessions) and no
    /// vendor-native candidate, every contribution is `0.0`, every candidate
    /// ties, and `best` keeps the first — which is `Upstream::backends()`'
    /// own order, which is the user's configuration order.
    ///
    /// # What this function cannot decide, and it is not a gap here
    ///
    /// **The native-pairing prior is constant across every candidate set the
    /// shipped binary can build**, so at this caller it contributes a real
    /// number to every explanation and separates nothing.
    /// [`pairing::classify`] reads `query.route` exactly once, and only to
    /// compute `Pairing::protocol_fit` — a field
    /// `native_pairing_prior_contribution` never looks at — so
    /// [`pairing::PairingClass`] is a function of the harness, the model and
    /// the user's corrections alone. A session start's candidates are
    /// `crate::gateway::upstream::Upstream::backends`, which carry a
    /// provider, a credential and a cost and **no model**: the one model
    /// comes from the launch profile and is the same for all of them. Same
    /// harness plus same model means same class means same prior.
    ///
    /// `tests::the_native_pairing_prior_is_constant_across_a_real_session_start_candidate_set`
    /// holds that as an executable fact rather than a comment. What *does*
    /// separate candidates here is local evidence and session continuity,
    /// both of which are keyed by [`pairing::EvidenceKey`] and therefore vary
    /// with the route.
    ///
    /// `None` only when `candidates` is empty: there is nothing to start on,
    /// and `best` may not be called with nothing.

### `routing/interactive/mod.rs` — `fn on_provider_failure`

    /// Lines 512, 513, 514, 517 and 518: what a real provider failure does.
    ///
    /// `candidates` are the other backends configured for this session's
    /// protocol. The order is the caller's — the user's own ordering is the
    /// tiebreaker, exactly as it is in the free pool — but every candidate
    /// that survives `compatible` is now scored by Phase 9J's native-pairing
    /// prior and Phase 33A's local evidence (`score_candidate`), and the
    /// best-scoring one wins rather than simply the first one found. With no
    /// evidence at all (`evidence` answers `None` for everything, as
    /// [`crate::config::pairing::NoObservations`] always does) every
    /// candidate scores `0.0` and this reproduces "first compatible
    /// candidate" exactly, which is what every test in this module that
    /// passes `NoObservations` is checking.
    ///
    /// `evidence` is [`crate::config::pairing::ObservationSource`] rather
    /// than a concrete store for the same reason
    /// `native_pairing_prior_contribution` itself takes one: this function
    /// stays a pure function of its arguments (see this module's own header)
    /// with no knowledge of `crate::routing::evidence::EvidenceLedger` or how
    /// its caller reached it.
    ///
    /// `preference` and `overrides` are Phase 9J line 576's own patch: the
    /// user's configured native-pairing preference and corrections, resolved
    /// once from configuration by `crate::profile`'s gateway path and carried
    /// here by `crate::gateway::session::SessionRouting`, which is why this
    /// method takes them as arguments rather than storing them on `self` —
    /// `self.pin` is session *policy* state that a pin or an unpin replaces
    /// wholesale, while a resolved preference must survive that replacement
    /// unchanged.
    ///
    /// `correlations` is Phase 33C lines 1370–1376's answer to *do these
    /// two front doors fail together* — read off the same ledger as
    /// `evidence`, by the same caller, and passed beside it for the same
    /// reason: this function stays pure. [`RouteCorrelations::default`]
    /// (every pair unmeasured) reproduces the ranking exactly as it was
    /// before that package, which is what every test here that passes it
    /// checks.

### `routing/interactive/mod.rs` — `fn route_correlation_contribution`

    /// Capability map lines 1370, 1373, 1374 and 1376 at the one place a
    /// correlation changes a decision: what the ledger has **measured** about
    /// `candidate` failing at the same moments as the backend that just failed.
    ///
    /// # A sibling of `failure_domain_contribution`, not a change to it
    ///
    /// [`FailureDomain::between`] is a certainty about provider identity and
    /// stays exactly what it was; this term is evidence about behaviour, and it
    /// is only ever consulted for a pair that identity calls
    /// [`FailureDomain::Unknown`]. A same-provider candidate gets [`None`] here:
    /// the provider term already carries the whole penalty, and a second term
    /// for the same fact would count it twice. Keeping the two terms apart is
    /// also what keeps line 1851's count meaning what `glasshouse route` prints
    /// beside it — *steered off a candidate sharing the failed backend's
    /// provider* — while line 1852 is derived from this term alone.
    ///
    /// # What the magnitude is
    ///
    /// [`RouteCorrelation::confidence`] scaled by [`SHARED_FAILURE_DOMAIN_PENALTY`]:
    /// a pair observed failing together every time is penalised exactly as a
    /// shared provider is, a pair that never did is penalised nothing, and a
    /// pair between moves between — line 1374's "confidence-weighted", with the
    /// weight recomputed from the rows on every failover rather than stored.
    ///
    /// # Line 1376
    ///
    /// Below [`super::evidence::MIN_CORRELATION_SAMPLE`] events the term is
    /// `0.0` — indistinguishable in the ranking from no correlation at all —
    /// and its detail says how many of how many, so the explanation `glasshouse
    /// route` prints names the sample size before anything reads as meaningful.

### `routing/interactive/mod.rs` — `struct FailureDomainEffect` doc

    /// What the failure-domain term did to one ranking — **capability map line
    /// 1851**, derived rather than decided.
    ///
    /// # Why this is a derivation and not a rejection
    ///
    /// Design decision 1 makes failure-domain diversity *additive, never a
    /// filter*: `failure_domain_contribution` is a `-1.0` term inside an
    /// explanation, and nothing anywhere refuses a candidate for sharing the
    /// failed backend's provider. So no production code path *decides* that a
    /// failover was prevented, and inventing one would change the policy in
    /// order to measure it.
    ///
    /// What can be established honestly is a comparison: rank the survivors
    /// once as production does, and once with that one term's magnitude removed.
    /// If the winners differ, the term is what moved the decision.
    ///
    /// # The displaced candidate always shares the failed provider
    ///
    /// This is a property of the arithmetic rather than a claim. Every
    /// candidate's score differs between the two rankings by its own
    /// failure-domain magnitude, which is `0.0` for every candidate except one
    /// on the failed backend's own provider, where it is
    /// `SHARED_FAILURE_DOMAIN_PENALTY`. A candidate scoring `0.0` for that
    /// term therefore has the same total in both rankings, while every other
    /// candidate's total can only be lower in the production ranking — so a
    /// `0.0` winner of the term-free ranking still wins the production one, with
    /// `best`'s first-seen tie-break unchanged because both rankings walk the
    /// same order. A winner that *changes* is therefore always a candidate that
    /// shared the upstream, which is exactly the map line's *"failover onto the
    /// same unhealthy upstream"*.

### `routing/mod.rs` — module doc

    //! Routing policy: which backend serves which work, and why.
    //!
    //! [`classify`] is a third, independent thing: not a policy that picks a
    //! backend, but the lightweight, model-optional classification of a request
    //! (Phase 35) that a future policy — Phase 34F/35B, neither built yet — would
    //! read before picking one. Nothing in this module or its siblings consumes
    //! a [`classify::TaskClassification`] today; see that module's doc comment.
    //!
    //! # Two policy classes, and the reason they are two types
    //!
    //! Phase 9I line 533 asks Glasshouse to *"keep interactive harness routing
    //! and disposable-support-job routing as separate policy classes"*. That
    //! sentence is easy to satisfy on paper and easy to lose: one router with a
    //! `disposable: bool` parameter would read as compliance and would be one
    //! careless call site away from routing a live coding session the way a
    //! throwaway classification job is routed.
    //!
    //! So the separation here is **structural**, in three independent ways:
    //!
    //! 1. [`interactive::InteractiveRouting`] and
    //!    [`disposable::DisposableRouting`] are distinct types with distinct
    //!    result types — [`interactive::Assignment`] and
    //!    [`disposable::DisposableChoice`]. Neither result converts into the
    //!    other: there is no `From`, no `Into`, no shared trait, and no public
    //!    field on either, so a caller holding one cannot produce the other
    //!    without going through the policy that mints it.
    //! 2. Neither module names the other. `interactive.rs` contains no mention
    //!    of `disposable`, and `disposable.rs` none of `interactive`;
    //!    `tests::the_two_policy_classes_do_not_name_each_other` scans both
    //!    sources to keep it that way, the same move
    //!    `gateway::mod`'s import scan already makes.
    //! 3. They **decide differently on identical input**, which is the part that
    //!    matters: given one catalogue in which a free model and a paid model
    //!    both serve, the disposable class picks the free one and the
    //!    interactive class keeps the backend the session started on. A test
    //!    that only checked the type separation would pass for a router that had
    //!    quietly become one policy.
    //!
    //! # What this module refuses to do
    //!
    //! Nothing here opens a socket, resolves a credential, or reads the clock.
    //! Every function is a pure function of values the caller supplies —
    //! including `now`, which is a parameter and never [`std::time::Instant::now`]
    //! called inside a policy. That is not tidiness:
    //!
    //! - a policy that could probe would eventually probe, and Phase 9I line 534
    //!   says free requests must not be spent on health probes (see
    //!   [`free::FreePool`], whose only mutator is fed by real workload);
    //! - a policy that read its own clock could not be tested for a cooldown
    //!   boundary without waiting for one.
    //!
    //! # Credentials appear here only as names
    //!
    //! [`CredentialId`] holds a [`SecretRef`] — an environment variable name, or
    //! a store service and account. Never a value. Phase 9I lines 537 and 538
    //! require quota state to be tracked *per credential*, which means a
    //! credential has to be a map key, and a map key is a thing that gets
    //! printed. `SecretRef` is already the one shape in Glasshouse that is safe
    //! to write into a tracked configuration file, so it is the one used here.

### `routing/mod.rs` — `enum CacheLocality` doc

    /// Whether a change of backend leaves provider-side prompt caching usable.
    ///
    /// # Pinning down "likely", because Phase 9H line 516 uses that word
    ///
    /// The line is *"warn when failover is likely to invalidate provider-side
    /// prompt caching"*, and a capability whose trigger is a feeling is not a
    /// capability. So the rule is written down once, here, and every warning in
    /// Glasshouse comes from it:
    ///
    /// - **Different provider.** The cache is held by the provider. A request
    ///   that goes to a different service reaches a cache that never saw this
    ///   conversation. Certain, so [`CacheLocality::Lost`].
    /// - **Different model.** A provider-side cache is keyed by the model as well
    ///   as the prefix — a cached prefix for one model is not a cached prefix for
    ///   another. Certain, so [`CacheLocality::Lost`].
    /// - **Same provider and model, different credential.** Provider-side caches
    ///   are commonly scoped to the account a key belongs to, and Glasshouse has
    ///   established that for **no** configured provider — every template in
    ///   [`crate::provider::templates`] declares its capabilities `Unverified`.
    ///   So this is the case the map's "likely" is actually about, and it is
    ///   [`CacheLocality::LikelyLost`]: warned, and said as a likelihood rather
    ///   than as a fact.
    /// - **Nothing moved.** [`CacheLocality::Preserved`].
    ///
    /// The consequence worth stating: rotating a credential (Phase 9I line 537)
    /// is a cache event too, which is not obvious and is why the rule is a
    /// function rather than a comment.

### `routing/mod.rs` — `fn same_capability_tier`

    /// The seam map line 1970's tier-preserving fallback consumes: are these two
    /// destinations' models of the same user-assigned capability tier?
    ///
    /// # The axis, now landed
    ///
    /// `classify::WorkloadTier` ranks *how hard the task is*, and
    /// `capability::CapabilityAxis` answers *can it do this at all* — neither is
    /// "how capable is this model, relative to others". Phase 34F's answer to
    /// that question is the resolved ceiling a user assigns a model (an
    /// override, or a capability record's own `ceiling`): the same `WorkloadTier`
    /// vocabulary, read as *the tier this model is trusted to serve*. The caller
    /// resolves that once per destination — `main.rs::destination_tier_ceiling`,
    /// beside where it attaches [`session::Destination::with_tier_ceiling`] — and
    /// attaches it via [`session::Destination::with_capability_tier`]; this
    /// function compares the two attached values and reads no configuration of
    /// its own, matching every other free function in this module.
    ///
    /// [`TierRelation::Unknown`] is not [`TierRelation::Same`]: a destination
    /// whose model nobody assigned a tier answers unknown, and the fallback's
    /// tier steps never fire on it — the ruling's own direction, *"You can't put
    /// a fable 5 task and switch it to a nemotron v3"*, and a fallback that
    /// silently downgrades the model *"is worse than a refusal, because the work
    /// continues and looks fine"*.

### `routing/pressure.rs` — module doc

    //! Phase 35D — routing under subscription pressure: what a destination's
    //! capacity **band** and the nearness of its reset do to the session
    //! router's ranking, and the reserve policy the scope of work runs under.
    //!
    //! # What this module decides, and what it deliberately reuses
    //!
    //! Capability map lines 1570–1577, and 1606/1612 as Phase 38 restates them.
    //! Two contributions, one public function each so a mutation can zero exactly
    //! one of them (the same shape as `super::session`'s seven):
    //!
    //! - [`capacity_band_pressure`] — lines 1570, 1571, 1573, 1574 and 1577. A
    //!   premium destination in the **tight** band is penalised, and less so the
    //!   nearer its reset; one in the **reserve** band is put to the
    //!   protected-reserve policy this build already has,
    //!   [`crate::provider::quota::evaluate_reserve_spend`], under the policy the
    //!   caller's scope selects.
    //! - [`low_tier_spend`] — line 1575. A premium destination already under
    //!   pressure is not spent on low-tier work while a healthy zero-cost
    //!   alternative adequate for that work is among the candidates.
    //!
    //! Everything else here is *read*, not re-decided: the band comes from
    //! [`crate::provider::quota::RemainingCapacityScore::band`] against the
    //! thresholds the user configures (line 1270) and the provider's own protected
    //! reserve percentage (line 1288); the reset comes from
    //! [`crate::provider::quota::CapacityState::seconds_until_reset`]; the task's
    //! tier is Phase 35's [`WorkloadTier`]; and the precedence a reserve-band
    //! decision follows is Phase 32F's own function. Inventing a second copy of
    //! any of those would be two scales for one question, which is the mistake
    //! `ReserveDecisionInputs::tier`'s doc comment already refuses.
    //!
    //! # The rule every term obeys — a term must be able to separate a pair
    //!
    //! `docs/product/evidence/phase-9j.md`'s last entry: a signal constant across
    //! the candidate set cannot change a ranking. Every contribution below has a
    //! test in `tests/subscription_pressure.rs` holding two destinations that
    //! differ **only** in its axis and resolving differently, and every case in
    //! which a term cannot separate anything — no reading, unknown tier, a
    //! zero-cost destination — contributes exactly `0.0` and says in its evidence
    //! that it is inert and why. That is not "assume healthy" and not "assume
    //! exhausted": an unread resource is neither preferred nor withheld, the
    //! stance `super::session::quota_pressure` already takes for the same
    //! reading.
    //!
    //! # "Premium" is one fact, and it is [`super::Cost`]
    //!
    //! The lines say *premium subscription*. The fact that decides it here is
    //! whether the destination costs the user anything at the margin —
    //! [`super::Backend::cost`], which is [`super::Cost::Metered`] for everything
    //! nobody has marked free (fail-closed, per that type's own doc) and
    //! [`super::Cost::Free`] for a model the user named in a provider's
    //! `free_models`. Nothing about the *shape* of the quota (a rolling window, a
    //! balance) is consulted: a metered key in its tight band is spent as
    //! carefully as a subscription in its tight band, and a reset time is what
    //! separates the two shapes when one exists, not a second flag.
    //!
    //! # Purity
    //!
    //! No clock, no store, no socket, no name. `seconds_until_reset` is a value
    //! the caller computed against its own clock, and the set-level facts in
    //! [`Alternatives`] are computed by the router from the candidate set it
    //! holds. **No provider, model or harness is named in this file** — the
    //! policy is tunable through configuration (`routing.reserve.*`,
    //! `routing.capacity_band_thresholds`, a provider's `reserve_percent`) and
    //! never through a hierarchy written here; line 1612 is enforced by a test
    //! that scans this source.

### `routing/pressure.rs` — `fn exhaustion_forecast_pressure`

    /// Line 1280: what a forecast of exhaustion before the next reset
    /// contributes.
    ///
    /// # It is inert unless a forecast exists, and it says so
    ///
    /// `super::burn::forecast` answers `None` for a resource with too few rows,
    /// no measured request-unit remaining amount, no known reset, or a zero burn
    /// rate — the four ways line 1278's *"sufficiently known"* is not met. Every
    /// one of them reaches this term as `inputs.forecast == None`, and every one
    /// of them contributes exactly `0.0`. That is what keeps a ranking on a
    /// build with no forecast identical to what it was before this module
    /// existed, which is a property with its own test
    /// (`a_destination_with_no_forecast_ranks_exactly_as_it_did`).
    ///
    /// # "Well before", and why it is not "before"
    ///
    /// [`super::burn::WELL_BEFORE_RESET_FRACTION`] carries the reasoning: a
    /// forecast landing just short of the reset is inside the estimator's own
    /// tolerance, and reacting to it would be line 1281's overreaction wearing a
    /// different hat. A resource forecast to exhaust *after* the reset, or in
    /// the last half of the window before it, contributes `0.0` and names the
    /// figures it was comparing — an informational line, not a silent one.
    ///
    /// # The words are hedged here too
    ///
    /// The evidence text says *estimated* and *at the current rate*, never
    /// *will*. The explanation this contributes to is read by a person through
    /// `glasshouse route`, so line 1283's restraint applies to it exactly as it
    /// applies to `crate::shell`'s capacity line.

### `routing/pressure.rs` — `fn reserve_verdict`

    /// Whether a reserve-band spend is justified — Phase 32F's own
    /// [`evaluate_reserve_spend`] when the task's tier is established, and its
    /// precedence with every tier-dependent branch taken **conservatively** when
    /// it is not.
    ///
    /// # The unknown tier, decided against line 1459
    ///
    /// Line 1459 says a low-confidence classification is a reason for a
    /// conservative rule. An absent one is the limit of low confidence. The
    /// reserve exists *for* high-tier work (line 1571), so a task not established
    /// to be high-tier is not admitted on the tier branch — the same outcome the
    /// lowest tier would get, with a reason that says the tier was unknown rather
    /// than one claiming the task "does not require the heavy tier". Every other
    /// branch — the user's override, an imminent reset, the absence of any
    /// cheaper adequate resource — is the same as Phase 32F's and admits the
    /// spend regardless of tier. A test in this module holds the unknown-tier
    /// verdict equal, on `is_allowed`, to the lowest tier's across every input
    /// combination, so the copy cannot drift from the original.
    ///
    /// # Line 1610, and the one thing that may make `task_nearly_complete` true
    ///
    /// Line 1610 (*"avoid migrating a nearly completed task solely to preserve a
    /// small amount of quota"*) is line 1294's guard seen from Phase 38, and
    /// both turn on the word **solely**: the guard stops a threshold being the
    /// whole reason work moves. A second reason may only come from the party
    /// that knows, so `task_nearly_complete` below is a **declaration** —
    /// somebody said so, on purpose, about this session, recently — carried in
    /// by the caller through `PressureInputs::task_nearly_complete` and scoped
    /// there.
    ///
    /// This module still infers nothing, and that half of
    /// `docs/product/design-decisions.md`'s *"A task is never 'nearly
    /// complete'"* is untouched: a proxy from turn counts or elapsed time would
    /// report "almost complete" for work that had merely been running a while,
    /// inverting the protection at exactly the moment it exists for. The value
    /// arrives from a caller or it is `false`; nothing here derives it.

### `routing/request.rs` — module doc

    //! Phase 34D — the router request schema, the router answer, and the one
    //! economy rule (Phase 34E, line 1467) that decides when the answer can be
    //! reused without asking again.
    //!
    //! # What reaches a routing model, and what structurally cannot
    //!
    //! [`RouterRequest`] is the whole of what a routing model is shown about a
    //! decision, and it is built from **values the caller already holds at the
    //! moment it decides**: the task a person typed, which harness they named,
    //! whether a warm session exists among the candidates, the capacity *band*
    //! of each candidate provider, and the constraints they stated. Every field
    //! is a typed, bounded value. There is no field of type "file", "transcript",
    //! "environment" or "credential", and no constructor takes one — so map lines
    //! 1425, 1426, 1455 and 1456 hold by the shape of the type rather than by a
    //! filter that could be bypassed. The one free-text field, the task, is
    //! bounded by [`TASK_TEXT_CEILING_BYTES`] and is the half
    //! [`crate::memory::extract::Prompt::for_request`] scrubs before anything
    //! leaves the process.
    //!
    //! # Bands, never numbers
    //!
    //! Line 1449: a provider's remaining quota reaches the model as one of five
    //! words ([`crate::provider::quota::CapacityBand`]) and never as a remaining
    //! count, a limit, a reset time or a spend. The router needs to know whether
    //! a provider is tight; it does not need the billing figure that says so.
    //!
    //! # Purity
    //!
    //! The same rule as the rest of `routing`: no socket, no file, no clock. The
    //! sticky record ([`StickyClassification`]) is a value the caller persists and
    //! reloads; [`StickyClassification::reuse_for`] is a pure function of it and
    //! of what the caller can see right now.

### `routing/session/discovery.rs` — `fn hard_constraint`

    /// The gate step 2 runs. Five constraints and no others, for the same
    /// reason [`crate::routing::interactive`]'s `compatible` has two: each is a
    /// fact about whether the destination *can* serve, not a preference about
    /// whether it *should*.
    ///
    /// Two of the five — map lines 1517 and 1518 — are asked on both passes,
    /// like tool semantics and protocol: whether a destination lacks a required
    /// hard capability, or whether its provider has refused the credential or
    /// declared a still-active cooldown, does not depend on which tier the
    /// movement settled. Both follow the same "established, not merely unread"
    /// rule as the others: an unverified capability axis and an *invented*
    /// cooldown are not "cannot," so neither excludes — see [`is_adequate`] and
    /// [`provider_unavailable_cause`].
    ///
    /// The fifth — map line 1516 — fires only on an **established** ceiling
    /// strictly below the required tier. A destination with no ceiling stated
    /// passes, because "nobody has said" is not "cannot"; the same rule the
    /// other constraints already follow for `Unverified` tool semantics and an
    /// unknown protocol.
    ///
    /// `minimum_tier` is the tier the gate reads — [`TierMovement::gate_tier`]
    /// once the movement is decided, and `None` for the pass that decides it
    /// (the two capability constraints only). It is an argument rather than
    /// `inputs.requirements.minimum_tier` so a downgrade (line 1562) can admit
    /// a resource the classified tier would have refused, in exactly one place.

### `routing/session/mod.rs` — module doc

    //! Phase 37 — the basic session-aware router: which *destination* a piece of
    //! work goes to, and why.
    //!
    //! # What makes this a different router from the two beside it
    //!
    //! [`super::interactive`] answers "which backend serves this session", and
    //! [`super::disposable`] answers "which resource serves this throwaway job".
    //! Both rank **backends**. This module ranks **destinations**, and a
    //! destination is a strictly larger thing: an existing session that could be
    //! continued, or a fresh session that would have to be started. Map lines
    //! 1593 and 1594 are that comparison and nothing else — *"prefer an existing
    //! relevant session"* against *"prefer a fresh session"* — and neither can
    //! be expressed by a policy whose candidates are all backends.
    //!
    //! That difference is also what makes the six `Consider X` lines (1595–1600)
    //! answerable here when their equivalents were not answerable one layer down.
    //! `docs/product/evidence/phase-9j.md`'s last entry records why: a signal
    //! that is **constant across the candidate set** cannot change a ranking, and
    //! every candidate set `crate::gateway::upstream::Upstream` can build varies
    //! only by route, because [`Backend`] carries a provider, a credential and a
    //! cost and no harness and no model of its own. A [`Destination`] carries its
    //! own [`IntegrationId`] and its own [`Continuation`], so a candidate set here
    //! genuinely varies along harness, warmth, cache locality, credential and
    //! bootstrap cost — the six axes the six lines name. Every contribution below
    //! has a test that holds two destinations differing **only** in that axis and
    //! asserts they resolve differently; a contribution that could not do that
    //! would be dead weight, and saying so is the finding rather than the failure.
    //!
    //! # The native-pairing prior, and why it belongs here and not one layer down
    //!
    //! `docs/product/evidence/phase-9j.md`'s 2026-09-02 entry corrects the
    //! sentence this paragraph used to carry: the constancy proof it cites is
    //! scoped to [`super::interactive`]'s `UpstreamBackend`, which has no model
    //! field of its own and takes one model for the whole candidate set at
    //! `SessionRouting::bind`. A [`Destination`]'s [`Backend`] carries a model
    //! resolved **per launch profile** (`main.rs::destination_backend` →
    //! `session_pairing`), so a candidate set built from two enabled profiles of
    //! one harness genuinely varies in `PairingClass` — a fact
    //! `docs/product/evidence/phase-56.md`'s "The question the orchestrator
    //! added" section establishes from current production code. [`pairing_prior`]
    //! reads `classify`'s *vendor* axis for exactly that reason, beside
    //! [`harness_capability_fit`], which reads its *capability* axes (protocol
    //! fit, model-behaviour fit, tool semantics) and does not.
    //!
    //! # Purity
    //!
    //! Same rule as the rest of `routing`: no socket, no credential resolution,
    //! no clock. `now` is an argument. Warmth, capacity and checkpoint quality
    //! are values the **caller looked up** — this module names neither
    //! `crate::session` nor `crate::checkpoint`, for the reason
    //! [`crate::config::pairing::ContinuitySource`] gives.

### `routing/session/reserve.rs` — entitlement-pool terms block

    // ---------------------------------------------------------------------------
    // Phase 56A step 3, lines 1953 and 1966–1969 — the entitlement pool's own
    // terms: the pool enters the candidate set (main.rs widens it, one candidate
    // per entitlement allowed to serve the harness), and the score chooses.
    //
    // Five factors, map line 1966's own list: available capacity (band), time
    // until reset, recent throttling, session affinity, and model availability.
    // The affinity factor is deliberately NOT a new term — it **is**
    // [`session_affinity`], because the entitlement holding a warm session's
    // context scores exactly what that session's warmth already says, and a
    // second number for the same fact would be the double-count this module
    // refuses everywhere. Stickiness (line 1968) is therefore the affinity
    // term's weight, not a second mechanism, and
    // [`entitlement_stickiness_note`] says so in the explanation.
    //
    // Two rules every term below obeys:
    //
    // - **an unknown facet contributes NOTHING and says so** — never a guessed
    //   number, the same stance `quota_pressure` takes for an unread quota;
    // - **the terms are live only when the candidate set actually offers a
    //   choice of configured entitlements** ([`EntitlementPoolView`], two or
    //   more distinct configured names). A user with zero or one configured
    //   entitlement has no pool for a score to choose across, and their ranking
    //   must stay byte-for-byte what it was — the packet's own preservation
    //   clause, enforced structurally rather than hoped for.
    // ---------------------------------------------------------------------------

### `routing/session/reserve.rs` — `fn entitlement_fallback`

    /// Map line 1970's reselection: given the ranking and the index it settled
    /// on, the index the work should move to instead and the record of why.
    ///
    /// `ranked` is the already-ranked, already-gated list `Routed` keeps as
    /// [`Routed::considered`], best first — so every candidate this function can
    /// reach has passed every hard constraint, **map line 1971's rules
    /// included**. That is the whole of
    /// how *"an exhausted pool does not license exceeding a rule"* is enforced:
    /// there is no path from here to a candidate the gate removed, because the
    /// gate ran first and this function never sees its rejections.
    ///
    /// `None` — no fallback — whenever any of these holds, and each is a
    /// deliberate narrowing rather than an omission:
    ///
    /// - the candidate set carries fewer than two configured entitlements, so
    ///   there is no pool to fall back across (the gate every pool term checks);
    /// - the chosen candidate carries no entitlement, or its entitlement is
    ///   neither exhausted nor throttled — the untriggered case, which must stay
    ///   byte-identical to today's decision;
    /// - no step of [`FallbackStep::ORDER`] matched a **healthy** candidate on a
    ///   **different** account. A sibling in the same state is not a refuge, and
    ///   a second candidate on the *same* account is the same account.

### `routing/session/scoring.rs` — `fn capability_fit`

    /// Map line 1382, joined to a task's hard capability requirements —
    /// `GH-ROUTING-CAPABILITY`'s package, and `capability::axis_for`'s own
    /// comparison function is what makes this ruling-1-safe: this function never
    /// compares a task's tier to a resource's tier, only a resource's registry
    /// entry to the specific axis a requirement names.
    ///
    /// This is one of `TaskClassification::hard_capabilities`' two production
    /// consumers — the other is `is_adequate`, which `session::hard_constraint`
    /// asks the same question of to raise `HardConstraint::Capability` (map line
    /// 1517). `requirements.hard_capabilities` is where a caller of
    /// [`SessionRouter::choose`] attaches it: `main.rs`'s `launch_session` and
    /// `route_recommendation` both build it from `classified.answer.requirements()`
    /// on every classified launch; a caller with no task in hand still passes
    /// `TaskRequirements::default()`, an empty list that contributes `0.0` here
    /// and excludes nothing at the gate.
    ///
    /// Reads `destination.harness()` the same way [`harness_capability_fit`]
    /// does — the identity is already in hand at the point this term is
    /// computed — and combines it with [`Destination::resource_facts`] through
    /// [`capability::ResourceCapabilities::describe`]. No capability value and no
    /// resource identity is matched here: this function only asks the registry a
    /// question and applies the three named constants above, which is 1390's
    /// answer — a new resource, a new harness, or a corrected axis changes
    /// nothing in this function's body.

### `routing/session/scoring.rs` — `fn cost_preference`

    /// Map line 1558: *"prefer the cheapest healthy candidate that satisfies the
    /// required workload tier and hard capabilities."*
    ///
    /// # What this term is, and what the three words before it already decide
    ///
    /// The line names four properties, and three of them are already decided
    /// before this function is reached, which is why it prices only the fourth:
    ///
    /// - *satisfies the required workload tier* — `hard_constraint` has already
    ///   **removed** a destination whose established ceiling is below the
    ///   requirement, and [`workload_tier_fit`] prices the fit of what is left;
    /// - *satisfies the hard capabilities* — [`capability_fit`] prices an
    ///   established-absent axis at `CAPABILITY_ESTABLISHED_ABSENT`, four times
    ///   this term's magnitude, so a cheap resource established to lack what the
    ///   task needs can never win on price;
    /// - *healthy* — [`provider_health`] prices a refused credential and a
    ///   cooling-down provider, and its penalties are larger again.
    ///
    /// So what is left for this term is the comparison the line is actually
    /// about: two candidates the terms above could not separate, one of which
    /// spends the user's money. It is `METERED_COST_PREFERENCE` for a metered
    /// destination and `0.0` for a free one — a preference for the free
    /// resource expressed as a cost on the paid one, so that a project with no
    /// free resource configured is not scored as though every destination it has
    /// were somehow deficient.
    ///
    /// # Why it is only pushed when a tier was established
    ///
    /// `score` pushes this term exactly where it pushes [`workload_tier_fit`]:
    /// under `if let Some(required)`. The line's own subject is *"a candidate
    /// that satisfies the required workload tier"*, and there is no required tier
    /// until a task has been classified — so a launch or a `glasshouse route`
    /// that states no task renders precisely the explanation it rendered before
    /// this term existed, byte for byte. The same rule, and the same reason, as
    /// the tier term beside it.
    ///
    /// `Cost` is [`super::Backend::cost`], which `main.rs::destination_backend`
    /// resolves through `ProviderConfig::cost_of` — the user's own `free_models`
    /// list. Nothing here infers a price from a model's name.

### `routing/session/scoring.rs` — `fn expected_marginal_cost`

    /// Map line 1538: *"include expected marginal cost in candidate scoring."*
    ///
    /// `Cost` — [`super::Cost`]'s own doc calls it *"whether using a model costs
    /// the user anything at the margin"* — is still the only reading that can
    /// ever make this term `0.0`: [`Cost::is_free`] returning `true` is a
    /// **known** zero, never an unknown, and stays priced exactly as it always
    /// was. Phase 32G's `PriceTable` (`crate::provider::pricing`) answers the
    /// other half for a metered destination — a known per-million price, or an
    /// honest unknown — but it changes only the **evidence**, never the
    /// magnitude: there is still no per-call token estimate at this call site
    /// (`SessionContextFacts` carries none), so a known price cannot yet be
    /// converted into an actual expected dollar figure without inventing one.
    /// Reporting the known rate is honest; reporting a dollar estimate from it
    /// is map line 1298's job, once a size producer exists. A destination whose
    /// price is unknown is priced identically to one whose price is known but
    /// unconvertible — both metered, neither free — and the difference between
    /// them is only ever textual, the same way [`AffinityFacet`]'s `known` and
    /// `unknown` constructors both start every unattached facet as `0.0`.
    ///
    /// **Pushed unconditionally**, unlike [`cost_preference`], because line 1538
    /// names no workload-tier precondition the way line 1558 does. That is also
    /// why it must stay inert exactly where [`cost_preference`] is active: once a
    /// tier is established, [`cost_preference`] already prices the same `Cost`
    /// reading as its own deliberately small tie-break (line 1558's own doc).
    /// Pricing it again here would score the identical fact in the identical
    /// direction a second time — the double-count this term exists to avoid, not
    /// to add — so the two conditions partition rather than overlap: exactly one
    /// of them ever prices a given candidate.

### `routing/session/scoring.rs` — `fn request_pool_cost`

    /// Line 1302: what a request pool's own scarcity is worth, read from
    /// [`Allowance`]'s remaining count and [`Destination::burn_forecast`]'s
    /// persisted rate — never recomputed from ledger rows, and never folded into
    /// `expected_marginal_cost`'s magnitude: a reader sees two terms, one for
    /// money and one for a scarce unit money does not price.
    ///
    /// # Its own axis, never 1280's twice
    ///
    /// [`super::pressure::exhaustion_forecast_pressure`] already prices the case
    /// where a resource will not make it to its reset. This term is inert
    /// whenever [`crate::routing::burn::ExhaustionForecast::exhausts_well_before_reset`]
    /// says that term is already carrying the penalty for this destination's
    /// resource — `phase-32g.md`'s 1302 entry: one forecast, priced once. What is
    /// left for this term is the case beside it: a pool that will make its reset
    /// but is being spent fast enough to be worth naming.
    ///
    /// # Inert, and says so, in three cases
    ///
    /// - the allowance is [`Allowance::TokenPriced`] — "how many requests are
    ///   left" has no answer for a resource priced per token, and pricing it
    ///   anyway is exactly the conflation `free.rs`'s own module doc warns
    ///   against;
    /// - the pool's remaining count is not yet known, or the destination carries
    ///   no burn forecast at all (too few rows, no measured remaining amount, or
    ///   a non-positive rate — see [`crate::routing::burn::forecast`]);
    /// - the forecast already exhausts well before the reset, which is the case
    ///   above.

### `routing/session/scoring.rs` — `fn estimated_cost`

    /// Map line 1307: the marginal input cost this decision actually used, as a
    /// monetary reading with its required confidence — never recomputed once
    /// carried. [`SessionRouter::choose`] calls this exactly once, for the
    /// destination it settled on, and the result travels on [`Routed`] to
    /// whatever records it (`main.rs::record_entitlement_fallback`), rather than
    /// being derived a second time at the writer from a `PriceTable` that may
    /// have changed on disk since the decision was made.
    ///
    /// Free is a known zero, regardless of size — nothing is spent whatever the
    /// input turns out to be, the same certainty [`expected_marginal_cost`]'s
    /// free branch reads. A metered destination needs **both** a known price
    /// and a known size; either half missing answers `None` — never a
    /// fabricated zero, matching map line 1307's own rule that unknown size or
    /// unknown price means no cost row at all.
    ///
    /// [`CostConfidence::Estimated`], always — including the cached-input split
    /// below. `CostConfidence` distinguishes *provenance* (a provider-reported
    /// invoice figure versus Glasshouse's own arithmetic versus nothing at all),
    /// not how many of Glasshouse's own readings that arithmetic combines. A
    /// split estimate is built from two of this build's own measurements — the
    /// user's declared `cached_input_per_million_usd` and this route's own
    /// observed `cache_read_ratio` — rather than one, but neither reading is a
    /// provider-stated figure, so it has no more claim to [`CostConfidence::Exact`]
    /// than the flat estimate above it did; migration 11's `CHECK` requires a
    /// label to be chosen, and this is the one that says so.

### `routing/session/scoring.rs` — `fn measured_cache_temperature`

    /// Map lines 1535/1545: this destination's own measured prompt-cache read
    /// history — [`Destination::route_responsiveness`]'s attached
    /// [`RouteResponsiveness::cache_read_ratio`], read the same way
    /// [`observed_pairing_reliability`] and [`tool_round_rate`] already read that
    /// reading's other fields, so a caller that attaches none, or a route too
    /// thin to summarize, leaves this term exactly as inert as those two already
    /// are.
    ///
    /// This is a **different** signal from [`prompt_cache_state`], right below:
    /// that term answers whether *this specific move* would preserve a cached
    /// prefix (a locality fact); this one answers how often *this route in
    /// general* has actually shown a cache read, over its own recorded history.
    /// The two are pushed side by side deliberately, and see
    /// [`MEASURED_CACHE_TEMPERATURE_MAGNITUDE_CEILING`]'s own doc for why this
    /// one is bounded strictly below both of that term's magnitudes.
    ///
    /// `0.0`, saying so, when: no responsiveness reading is attached to this
    /// destination; or the reading's ratio is `None` — fewer than
    /// [`MIN_SAMPLE_FOR_SUMMARY`] rows carried a known input-token count for
    /// this route. Otherwise the magnitude is linear in the ratio, centred on
    /// `0.5`: a route with no measured warmth advantage either way scores
    /// `0.0`, a perfectly warm observed history scores
    /// `+MEASURED_CACHE_TEMPERATURE_MAGNITUDE_CEILING`, and a perfectly cold one
    /// scores the negative of that. The `clamp` is defensive — the ratio's own
    /// domain (`[0.0, 1.0]`) never reaches it, the same recorded shape as
    /// [`observed_pairing_reliability`]'s own clamp.
    ///
    /// # `cooling_down_until` is the caller's conversion, and that is
    /// deliberate
    ///
    /// [`Instant`] has no epoch, so a deadline that crossed a process
    /// boundary as a wall-clock second can only be placed on this process's
    /// monotonic clock by something holding **both clocks read at the same
    /// moment**. This pool holds neither.
    /// [`crate::provider::telemetry::GatewayHealthReading::cooling_down_until`]
    /// is that conversion and states the rule this method depends on: a
    /// deadline that has already elapsed arrives as `None` — *not cooling
    /// down* — never as an `Instant` in the past manufactured for the sake of
    /// carrying a value.
    ///
    /// Last write wins, exactly like `observe`: a resource this is called for
    /// twice holds what the second call said.
    ///
    /// # `cooldown_cause` crosses honestly, never as a guess
    ///
    /// `GatewayHealthReading` persists `cooldown_cause` as an optional field,
    /// serde-defaulted so a cache file written before it existed still
    /// deserializes — as `None`, never a guess. The caller hands this method
    /// exactly what that reading said: a genuinely recorded
    /// [`CooldownCause::Declared`] or [`CooldownCause::Invented`] crosses as
    /// itself, and an absent cause — no key in the file, or a resource that
    /// simply is not cooling down — adopts as `None`, which
    /// [`ResourceHealth::declared_wait_remaining`] reports as inert rather
    /// than as a guess in either direction.

### `routing/free.rs` — `fn withhold_in_flight`

    /// Net the requests other dispatches already hold out of what this
    /// credential's pool is known to have left — capability map line 1367,
    /// on the reading side.
    ///
    /// `known_remaining` is what a real response actually stated is left, as
    /// a caller read it back off disk; `in_flight` is how many of those a
    /// concurrent process has already claimed and has not yet spent. The
    /// difference is what a dispatcher deciding *now* may actually use, and
    /// it is that difference this records, so
    /// [`Allowance::is_exhausted`] — and therefore
    /// [`FreePool::is_available`], which is the one gate
    /// `crate::routing::disposable::DisposableRouting::choose` puts every
    /// free candidate through — sees a pool that is empty when every
    /// remaining request is spoken for.
    ///
    /// # Why this is not [`FreePool::record_pool`]
    ///
    /// `record_pool` carries *"what the provider claims is left"*, and no
    /// provider claimed this. The subtraction is Glasshouse's own bookkeeping
    /// about work it is itself about to do, and giving it its own name keeps
    /// a reader of the allowance from mistaking a local claim for a
    /// statement on the wire. It also keeps the two mutable in different
    /// ways: a later real reading overwrites `remaining` outright, which is
    /// correct, because a response is authoritative about the pool and a
    /// reservation never was.
    ///
    /// A credential this pool has been told is [`Allowance::TokenPriced`] is
    /// left exactly as it is: there is no request count to net anything out
    /// of, and inventing one would be the conflation line 528 forbids.
    /// Nothing here is a rule change — the rule that a pool with no requests
    /// left cannot serve is [`Allowance::is_exhausted`]'s, unchanged, and
    /// this only feeds it a truthful number.

## Trims: commands module docs — history moved out of comments by `GH-TRIM-COMMANDS-DOCS`, 2026-09-05

### `commands/context_firewall.rs` — `record_file_touches`

    /// Map line 1139's producer: one `file_touched` lifecycle event per distinct
    /// path a **writing** tool named, for the Glasshouse session this hook was
    /// registered for.
    ///
    /// # Why the hook's response can never depend on this
    ///
    /// It returns `()`. There is no error for the caller to see, no value for it
    /// to branch on, and every failure below ends in a `tracing::warn!` and a
    /// `return`. That is not caution about a rare case — the whole tool call is
    /// downstream of this function, and a bookkeeping write that could fail a
    /// user's `Edit` would be a far worse defect than never learning which file
    /// it touched. `the_hook_response_is_identical_whether_or_not_recording_works`
    /// is the proof rather than this paragraph.
    ///
    /// # The four gates a path passes, in order
    ///
    /// 1. **A session**, or nothing is recorded. See
    ///    `cli::ContextFirewallCommand::Hook`'s `--session` for why absent is a
    ///    supported state and why the payload's own `session_id` is not a
    ///    substitute.
    /// 2. **A writing tool** — `firewall::eligibility::is_writing_tool`, which is
    ///    the block list read the other way round. `Read`, `Grep` and `Glob`
    ///    carry paths and are deliberately not recorded: *touched* means the
    ///    session changed the file.
    /// 3. **Under the project root.** An absolute path inside the root is made
    ///    relative to it; a path outside it is **dropped and never stored**, which
    ///    is the isolation invariant rather than a tidiness rule — a memory must
    ///    not be able to name a file in another project, or in the user's home.
    /// 4. **Normalisable**, through the one function `memory_files.path` already
    ///    goes through, so the two producers spell a path identically or the
    ///    association never matches.
    ///
    /// Distinct paths only: `MultiEdit` names the same file once per edit, and
    /// sixty rows saying one file was edited is sixty times the storage for the
    /// same fact.

### `commands/context_firewall.rs` — `project_relative_path`

    /// `raw` as a path under `root`, in `memory_files.path`'s spelling, or
    /// `None`.
    ///
    /// Claude Code hands the hook an **absolute** path, and on Windows it hands
    /// one with `\` separators. So: fold the separators first — before any
    /// prefix test, because `C:\proj\src\a.rs` does not start with
    /// `C:/proj/src` until it has been folded — then strip the root, then put
    /// what is left through
    /// [`glasshouse::memory::store::normalize_observed_path`], which is the
    /// function the other writer of this column uses and the only definition of
    /// the spelling.
    ///
    /// Both sides are reduced to one spelling before any prefix test, and the
    /// separator fold is only half of that: on Windows the root is
    /// `fs::canonicalize`'s output and therefore **verbatim** (`\\?\C:\proj`),
    /// while a tool input or a shell argument is not, so the two would fail to
    /// match for the same reason `\` and `/` did. See
    /// [`folded_ordinary_spelling`].
    ///
    /// `None` for a path outside the root, and that is the isolation invariant:
    /// nothing outside the project is stored, not even to be filtered out later.
    /// A relative path is accepted as already being relative to the root, which
    /// is what a relative path in a tool input means.
    ///
    /// `pub(crate)` for `commands::sessions::claimed_path`, which needs the same
    /// answer for the same reason: `file_claims.path` and `memory_files.path`
    /// hold the same spelling, and a second implementation of "inside this
    /// project, spelled this way" is how the two would come to disagree.

### `commands/gateway.rs` — `gateway_pairs_report`

Orphaned from `entitlements_report`'s doc comment (that function moved to
`commands/entitlements.rs`, leaving this text stranded above the next
function in the file); moved here rather than kept, since it does not
describe `gateway_pairs_report`.

    /// `glasshouse entitlements` — map line 1972's inspectable view of the pool.
    ///
    /// A pure function returning a `String`, like [`status_report`] and
    /// `resources_report`: what it prints is testable without a terminal, which
    /// is the only reason a view of this kind can be asserted at all.
    ///
    /// # Every configured entitlement, including the ones nothing measured
    ///
    /// The rows come from the **configuration**, not from the telemetry and not
    /// from the sessions table, so an account no reading describes still gets a
    /// row and reads `unknown` on the facets it has no reading for. An
    /// entitlement missing from the view because nothing had measured it is the
    /// exact failure 56A step 2's Cluster E discipline exists to prevent: unknown
    /// is a rendered word, never full, never empty, never a number.
    ///
    /// # Why `served` is *not* one of those unknowns
    ///
    /// The four telemetry facets are `unknown` when nobody looked. `served` is
    /// different in kind: this function **does** look, at every session row this
    /// project recorded, and an account with no rows has a *measured* zero. That
    /// is `SessionRecord::observed_compactions`' distinction, and rendering
    /// "nothing recorded" where the sessions table is empty rather than `unknown`
    /// is what keeps the two apart.
    ///
    /// # Names, never credentials
    ///
    /// An entitlement is named by its `[entitlements.<name>]` key and described
    /// by its kind and vendor. Its `credential` is a `config::SecretRef` and this
    /// function never touches it — nothing here opens a secret store, and there
    /// is no branch on which this view could print a value.

### `commands/launch.rs` — `routed_cost_class`

    /// The cost class of the destination a launch actually routed to — map line
    /// 1835's *"low-cost or free route"* versus *"the premium route it
    /// displaced"*, as a fact rather than a guess.
    ///
    /// # Why this is not `destination.backend().cost()`
    ///
    /// [`destination_backend`] hardcodes `Cost::Metered` for every destination it
    /// builds, and says so: the session router reads a backend's provider,
    /// credential, model and tool semantics and never its cost, so the field is
    /// the fail-closed constant rather than a measurement. Recording *that* as a
    /// route's class would give line 1835 one bucket for ever and report a
    /// tautology.
    ///
    /// So the class is read where the fact actually lives:
    /// [`ProviderConfig::cost_of`], the same one lookup `disposable_candidates`
    /// and `gateway_upstream` use, applied to the destination's own provider and
    /// model with the project layer winning over the user layer. `glasshouse::
    /// profile` and `glasshouse::routing` may not import `glasshouse::config`, so
    /// main.rs is where this can be answered at all.
    ///
    /// # `None` is the third answer, and it is honest
    ///
    /// A destination on a harness's own sign-in names no configured provider, and
    /// a gateway-backed one assigns its model when the session starts. Neither
    /// has a marked cost, and Glasshouse does not know what a subscription costs
    /// at the margin. That is recorded as
    /// [`glasshouse::evaluation::UNKNOWN_COST_CLASS`] and counted in its own
    /// bucket — never folded into `metered`, which would be a number nobody
    /// measured.

### `commands/launch.rs` — `routing_evidence_for`

    /// Whether the pool this launch handed the router held any observed health
    /// reading for the destination it chose — map line 1854's *sparse* half.
    ///
    /// The key is built exactly as [`observed_provider_health`] builds it, from
    /// the destination's own credential and model label, so a hit here means the
    /// same resource and not a resource that merely renders the same.
    ///
    /// **Two of line 1854's three words now, not one.** `routing::evidence`'s
    /// `Confidence` belongs to the gateway's aggregate ledger, which
    /// `SessionRouter` never reads, and a
    /// [`glasshouse::routing::free::FreePool`] health entry carries no
    /// observation time — but the cache the pool was filled from does, per
    /// provider file, and [`ObservedHealth`] carries it here. So *sparse* is
    /// answered by whether the pool held this destination and *stale* by how old
    /// the file that supplied it was, against
    /// [`glasshouse::evaluation::HEALTH_EVIDENCE_HORIZON_SECONDS`].
    ///
    /// *Incorrectly segmented*, line 1854's third, still has no producer
    /// anywhere on this path and is not invented: nothing in this build compares
    /// a health reading's segmentation against the resource it was attributed
    /// to, and the line stays open on that word alone.

### `commands/launch.rs` — `record_entitlement_fallback`

    /// Capability map line 1970: one ledger row per pool fallback the launch
    /// path acted on. The same open-write-drop shape as
    /// [`record_tier_movement`], for the same reasons — and **a decision that
    /// made no fallback writes nothing**, because "the broker stayed put" is
    /// the row's absence, exactly as a held tier is.
    ///
    /// The row carries the fallback whole **without a migration**: `purpose` is
    /// the trigger, `quota_context` is the account the work LEFT (so the
    /// entitlements view's own per-account reader finds it), and the account
    /// the work went TO is the `sessions.entitlement` column migration 22
    /// added, written by this same launch from this same decision. `provider`
    /// and `model` are the chosen destination's.
    ///
    /// Map line 1307's own producer: `cost`, when given, is
    /// [`glasshouse::routing::session::Routed::cost`] — the value **that
    /// decision itself computed**, carried in rather than recomputed here from a
    /// `PriceTable` that may since have changed on disk. This is the only launch
    /// writer with a `Destination` in scope
    /// (`record_tier_movement`'s `TierMovement` carries none), so it is the only
    /// production caller `cost_micro_usd` has today; most rows still leave it
    /// `NULL`, on every decision that made no fallback at all.

### `commands/launch.rs` — `record_routing_latency`

    /// Map line 1849: record what routing added to this launch, from the start
    /// of the decision (`started`) to its end — the point after which profile
    /// resolution, the gateway and the process spawn happen identically whether
    /// or not a task was stated, and are therefore the launch's own cost rather
    /// than routing's.
    ///
    /// Called only when a classification ran, so a launch that states no task
    /// opens no ledger (practice §65) and leaves no row: the row's absence is
    /// the honest reading of "nothing was added". Opened, written and dropped
    /// here, before any gateway holds its own handle.
    ///
    /// The ledger's timing columns are unix **seconds** (migration 11), so a
    /// sub-second decision reads back as `0` through `duration_ms()`; the
    /// millisecond figure goes to the log beside it. A finer column is a schema
    /// decision this package does not take.
    ///
    /// **This row carries no session id** — `glasshouse::database` migration
    /// 24's `session_id` stays `NULL` here, deliberately and permanently. The
    /// decision this row measures is taken *before* `store.create` mints a
    /// session, so there is no id to write; and the row is about the routing
    /// decision rather than about an exchange some session was served, which is
    /// the only thing that column is for. Filling it from a session recorded
    /// later would make "the launch decided this before any session existed"
    /// indistinguishable from "this exchange belonged to that session", which is
    /// the distinction the nullable column exists to keep.

### `commands/launch.rs` — `install_edit_intent_hook`

Orphaned from `briefing_announcement`'s doc comment (`briefing_announcement`
is defined later in the file at its own, now-undocumented, `fn`); moved here
rather than kept, since it does not describe `install_edit_intent_hook`.

    /// The `briefed with ...` line both delivery rungs print, once, on a
    /// successful delivery — never composed twice so the wording cannot drift
    /// between rungs.
    /// Map lines 2402-2405: register Phase 60's edit-intent `PreToolUse` hook
    /// for a Claude Code session, unless a configuration layer turned
    /// coordination off.
    ///
    /// **Never a second `--settings` flag**, for the reason
    /// [`crate::commands::resume::install_context_firewall_hook`] states at
    /// length: Claude Code keeps only the last one, so the only safe way to add
    /// a hook is to merge it into the document `install_session_document`
    /// already wrote. This reads that file back, adds one `PreToolUse` key, and
    /// writes it in place; `args` is never touched.
    ///
    /// **`mode = "off"` installs nothing at all** — line 2405's own words, and
    /// the reason this returns before reading the executable path or the session
    /// directory. Not installed-and-inert: an inert hook would still spawn a
    /// process for every `Edit` the session makes.
    ///
    /// Best effort, matching every other registration on this path: a failure
    /// here is a session that starts without coordination rather than one that
    /// fails to start, and it is logged rather than propagated. There is no
    /// version floor and no probe — unlike the firewall's `updatedToolOutput`,
    /// nothing this hook returns needs a Claude Code newer than the one that
    /// first accepted a `PreToolUse` entry, and the worst a build that ignores
    /// the entry can do is not run it.

### `commands/memory_extraction.rs` — `run_extraction`

    /// Run memory extraction over what this session has done — Phase 29's
    /// **memory commit**, whatever started it.
    ///
    /// # One operation, four triggers, and no second pipeline
    ///
    /// Map line 1147 asks for *"a lightweight memory commit operation that
    /// extracts durable project knowledge from recently completed work"* and
    /// lines 1148-1151 ask for four ways to start one. This function is that
    /// operation, and `trigger` is the whole of the difference between them:
    /// `Manual` from `glasshouse memory commit`, `TaskCompleted` and `GitCommit`
    /// from the `TurnEnded` arm of [`report_hook_with`], `BeforeCompaction` from
    /// its `PreCompact` arm. A second extraction path for any of them would be a
    /// second answer to what is worth remembering, a second credential screen and
    /// a second duplicate check.
    ///
    /// # The outcome is returned, and the hook path still ignores it
    ///
    /// `Option<ExtractionOutcome>` rather than `()` so `glasshouse memory commit`
    /// can print what its run actually did. It is not an error channel and does
    /// not become one: `None` means the *preparation* failed or the bound expired
    /// — both already logged here — and every failure of the extraction itself is
    /// a field on the outcome, never a `Result`. The hook path discards it, which
    /// is why nothing about its posture changes.
    ///
    /// # Nothing here can hurt the session, and that is the design
    ///
    /// Phase 21: *"keep memory-extraction failure non-fatal to the coding
    /// session."* Four different failures are absorbed here and none of them
    /// reaches [`report_hook`]:
    ///
    /// - the project database will not open, or the event log will not read —
    ///   logged, and the function returns;
    /// - the model is unavailable, refuses, or answers rubbish —
    ///   [`glasshouse::memory::Extractor::run`] has no error channel at all and
    ///   describes it on the outcome;
    /// - the model **panics** — caught inside `run`, reported as an outcome;
    /// - the model **hangs** — the work is on its own thread and this waits
    ///   [`EXTRACTION_BOUND`], then leaves it behind. The thread dies when the
    ///   process exits moments later, having written nothing: the store is only
    ///   touched after the model answers.
    ///
    /// # Why a thread and not just a call
    ///
    /// The only thing that buys is the bound, and the bound is the whole point.
    /// This codebase has no async runtime and [`glasshouse::memory::ExtractionModel`]
    /// is deliberately synchronous, so a thread is the mechanism; `ExtractionModel`
    /// is `Send + Sync` for precisely this reason.
    ///
    /// Everything cheap happens before the thread starts — opening the database,
    /// reading a bounded window of the log, scrubbing and bounding the chunk — so
    /// what is on the far side of the bound is the model call and the insert, and
    /// a timeout means the model, not Glasshouse.

### `commands/memory_extraction.rs` — `hook_extraction`

    /// [`run_extraction`] on a hook's path, where a lost memory has to be said
    /// out loud.
    ///
    /// # Why this exists at all, when `run_extraction` already logs every failure
    ///
    /// Because on this path nothing reads the log. `logging::LogConfig::resolve`
    /// answers [`glasshouse::logging::LogSink::Disabled`] unless `GLASSHOUSE_LOG`
    /// is set or a `--log-*` flag is given, and a harness spawning
    /// `glasshouse hook` gives neither — so `run_extraction`'s
    /// `"memory extraction produced nothing"` and its bound-expiry `warn!` are
    /// both written to a subscriber that was never installed. Measured
    /// 2026-08-31: a `PreCompact` hook whose model call failed exited **0**, with
    /// **empty stderr**, having recorded nothing.
    ///
    /// That is the precise thing capability map line 1174 is about. *"Record
    /// enough pre-compaction durable memory that important project decisions do
    /// not depend solely on a lossy native compact summary"* is not satisfied by
    /// a trigger that fires, fails, and says nothing: the person then believes
    /// their decisions were captured and goes on to compact, which is worse than
    /// knowing they were not.
    ///
    /// # Why stderr, and why one line
    ///
    /// `main.rs`'s own [`run`] already draws this distinction for the overridden
    /// safety refusal, three lines into the program and for exactly this reason:
    /// *"logging is off by default, so a `tracing::warn!` there can go completely
    /// unseen … it always gets a line on stderr, log or no log."* A memory the
    /// compaction trigger was supposed to record and did not is user-facing in
    /// the same sense.
    ///
    /// Stderr and not stdout, and never a non-zero exit: Claude Code reads a
    /// hook's exit code as a gate on the turn, and Phase 21's *"keep
    /// memory-extraction failure non-fatal to the coding session"* is unchanged
    /// by this. The hook still exits zero whatever extraction did.
    ///
    /// Not used by `glasshouse memory commit`: that trigger is
    /// [`glasshouse::memory::ExtractionTrigger::Manual`], it runs in front of a
    /// person who is watching, and it prints its own report. This is the wrapper
    /// for the triggers that run inside somebody's session with nobody watching.

### `commands/memory_extraction.rs` — `lost_extraction_notice`

    /// What to tell the person about an extraction that recorded nothing, or
    /// [`None`] when nothing was lost.
    ///
    /// Separated from [`hook_extraction`] so the decision can be tested without a
    /// process: what this returns is the whole of the difference between a silent
    /// loss and an observable one.
    ///
    /// # The four cases, and why two of them are silent
    ///
    /// - **no outcome at all.** [`run_extraction`] answers `None` for its two
    ///   preparation failures and for [`EXTRACTION_BOUND`] expiring. All three
    ///   are losses — a boundary went by and nothing was written — and the reason
    ///   is in a log that, on this path, does not exist.
    /// - **a failure.** The model was unavailable, refused, timed out, panicked,
    ///   answered something the contract could not read, or the store could not
    ///   be read for duplicate detection. Each is a memory that should exist and
    ///   does not, and [`glasshouse::memory::extract::ExtractionFailure`]'s `Display` is a
    ///   fixed phrase by construction — no provider body reaches this line.
    /// - **[`glasshouse::memory::extract::ExtractionFailure::NothingToExtract`] is
    ///   deliberately silent.** There was no session activity to extract from, so
    ///   there is no memory to have lost. A warning here would fire on every
    ///   compaction of a session that had not done anything yet, and a warning
    ///   that cries wolf is how the real one gets ignored.
    /// - **rejections without a failure.** The model answered and some of what it
    ///   proposed did not survive the contract. Said out loud when *nothing*
    ///   survived, and silent when something did: a run that stored two memories
    ///   and rejected a third lost nothing a person needs to act on, and
    ///   duplicates and speculative drops are the mechanism working rather than
    ///   failing.

### `commands/memory_extraction.rs` — `record_extraction_observation`

    /// What the extraction model reported the call cost, into this project's
    /// routing evidence ledger.
    ///
    /// # This is the first thing in this build that counts tokens
    ///
    /// `routing_observations` has carried `input_tokens`, `output_tokens` and
    /// `cached_input_tokens` since migration 11 and nothing has ever written
    /// one: `crate::gateway::ingress` relays a response body it is designed
    /// never to parse, so the gateway producer leaves all three `NULL` and says
    /// so in its own module header. Memory extraction is the other path —
    /// Glasshouse builds the request itself and already deserializes the whole
    /// reply — so the counts come from a document that was parsed anyway. See
    /// [`glasshouse::memory::extract::ModelCall::observation`] for exactly what
    /// one row carries and what it deliberately leaves empty.
    ///
    /// # Why the ledger is opened here and not beside the event log
    ///
    /// The same finding [`evidence_ledger`] carries, one path over.
    /// [`glasshouse::routing::evidence::EvidenceLedger`] holds `Mutex<Connection>`
    /// — an open SQLite handle for its whole lifetime — and a handle opened on a
    /// path that turns out to have nothing to write blocks a later writer under
    /// Windows' mandatory `LockFileEx` while being invisible under POSIX advisory
    /// locks. So nothing is opened until `observation()` has already said there
    /// is a row: that is [`None`] for every run that reached no provider, which
    /// is every run under the default configuration, where extraction chooses a
    /// resource and calls nothing at all.
    ///
    /// # A failure here is one log line
    ///
    /// [`run_extraction`]'s own posture, for its own reason: this is a hook
    /// process running inside somebody's coding session, and Glasshouse's
    /// bookkeeping is never more important than the session it keeps books
    /// about. There is no error channel out of this function because no caller
    /// should have one.

### `commands/memory_extraction.rs` — `record_observed_files`

    /// Which files were being worked on when these memories were learned, into
    /// this project's `memory_files` — migration 17.
    ///
    /// # This records an observation and not a reference, deliberately
    ///
    /// `paths` is what the git index said differed from the working tree when
    /// extraction began. It says *"this was learned while that file was being
    /// worked on"*, which is a fact about the **session**: three memories out of a
    /// session that dirtied twenty files get all sixty pairs, and each pair is
    /// true. It is emphatically not capability-map line 1139's *"the files a
    /// memory explicitly references"* — on this path the model's input carries no
    /// prose at all, so a model asked to name files here would be fabricating from
    /// an empty input, and line 1294's rule is that a fabricated value inverts the
    /// policy rather than degrading it. Every row therefore carries
    /// [`glasshouse::memory::FileAssociation::Observed`].
    ///
    /// # Why the store is opened here and not beside the event log
    ///
    /// [`record_extraction_observation`]'s finding, one function over, for the
    /// same reason: an open SQLite handle on a path that turns out to have
    /// nothing to write blocks a later writer under Windows' mandatory
    /// `LockFileEx` while being invisible under POSIX advisory locks (practice
    /// §65). So the guard comes first and nothing is opened at all when there is
    /// no row — which is every extraction that stored nothing, and every one run
    /// against a clean tree.
    ///
    /// This deliberately runs on the calling thread rather than inside the
    /// extraction thread: the thread outlives its bound, and a write started
    /// there after the process has already decided to move on would be a second
    /// writable handle appearing at an unpredictable moment.
    ///
    /// # A failure here is one log line
    ///
    /// [`run_extraction`]'s posture, and the path is not named in it: a file path
    /// is the user's own data, so the log says how many associations were lost
    /// and never which files they were about.

### `commands/memory.rs` — `memory_path_report`

    /// `glasshouse memory search --path <p> [--for-edit]` — the CLI half of
    /// capability map lines 1143, 1141 and 1142, and the flag the 1143 evidence
    /// entry recorded as missing.
    ///
    /// Answers from [`glasshouse::memory::MemoryStore::for_path`], the same
    /// reader the socket door and the briefing's file section use, so the three
    /// surfaces cannot disagree about what a file is associated with. Not through
    /// [`memory_search_grouped`]: that helper records every retrieval through it
    /// as a *search* in the evaluation ledger, and a path lookup runs no query —
    /// recording it as one would misreport what was asked, which is the same
    /// reasoning `api::unix::query_memory_for_path` gives for opening the store
    /// directly.
    ///
    /// # What each row says beyond the memory itself
    ///
    /// `assoc=` is read per row (line 1139's second provenance), `freshness=` is
    /// line 1142's commit-order label, and the advisory line above the results is
    /// line 1142's own sentence: **the source at the path is the evidence**. A
    /// `stale` row is printed in its rank like any other — the label never
    /// withholds, reorders or rescores.
    ///
    /// `for_edit` is line 1141: within each authority rung, constraints,
    /// decisions and failed approaches sort ahead of features, findings and
    /// todos. Off, the order is byte-for-byte what a `Lookup` gives.
    ///
    /// One `git log` for the whole report, since every row is about one file.

### `commands/memory.rs` — `memory_challenge`

    /// `glasshouse memory challenge <id> <reason>` — Phase 21F lines 937/938:
    /// let the receiving agent say, explicitly, that current evidence
    /// contradicts a memory, rather than silently distrusting it in a way
    /// nothing records.
    ///
    /// Reuses Phase 21C's `mark_for_review` and its six reasons rather than
    /// inventing a seventh state: a challenge *is* "something changed that may
    /// invalidate this; a person or a stronger agent has to look" — the review
    /// mechanism already built for that. The retrieval half of 937/938 is true
    /// the moment this returns: `SearchScope::Current` only ever returns
    /// `Active` memories (see `memory/search.rs`'s own documentation), so the
    /// challenged memory drops out of every default search immediately and
    /// stays reachable only as history — `glasshouse memory search --history`.
    ///
    /// 938's "before further automatic injection into the same task" has no
    /// consumer in this build: Phase 27 (automatic injection) does not exist, so
    /// there is nothing that injects a memory for this to gate. Closed on the
    /// retrieval half only — see the packet's own reasoning, echoing §33's rule
    /// of asking the capability as a question a user would ask: *can Glasshouse
    /// stop presenting a challenged memory as settled?* Yes. *Can it stop an
    /// automatic injection from using it?* There is no automatic injection to
    /// stop.

### `commands/memory.rs` — `memory_commit`

Orphaned lead-in from `memory_extract`'s doc comment (`memory_extract` is
defined later in the file at its own, now-undocumented, `fn`); moved here
rather than kept, since it does not describe `memory_commit`.

    /// `glasshouse memory extract` — Phase 21's manual run, for debugging and
    /// evaluating extraction itself.
    ///
    /// Everything except the model call is the production path: the chunk is
    /// bounded and scrubbed by `SessionChunk::build`, the reply goes through the
    /// same contract validation, credential screen, conservative classification
    /// and duplicate check, and what survives is written to the project's real
    /// memory store.

    /// # It is the same operation the harness triggers, not a hand-written twin
    ///
    /// This calls [`run_extraction`] with
    /// [`glasshouse::memory::ExtractionTrigger::Manual`], which is the same
    /// function the `TurnEnded` and `PreCompact` arms of [`report_hook_with`]
    /// call. Everything a person could get wrong by hand — the event window, the
    /// credential screen, the duplicate check, the bound, the working-tree
    /// observation, the routing observation — is therefore identical by
    /// construction rather than by two implementations agreeing.
    ///
    /// It is deliberately *not* [`memory_extract`]. That command exists to
    /// evaluate the contract without a provider, takes its reply from a file, and
    /// says so on every run; this one asks the model the user configured, which
    /// is what makes it a memory commit rather than a harness.
    ///
    /// # Defaulting to the most recently active session
    ///
    /// `SessionStore::list` is ordered `last_activity_at DESC`, which is the
    /// project's own answer to *"what was I just working on"* and the same order
    /// `glasshouse sessions` prints. A project with no sessions is an error
    /// naming the flag rather than a silent success: there is no honest
    /// "recently completed work" to commit, and reporting *stored 0* would be
    /// indistinguishable from a model that looked and found nothing.
    ///
    /// # One database handle at a time
    ///
    /// The session lookup is scoped so `ProjectSessions` is closed before
    /// [`run_extraction`] opens the event log and the memory store. That is
    /// practice §65's rule taken seriously on a path that has the choice: a
    /// handle held across work that does not need it is free on this developer's
    /// machine and billed under Windows' mandatory locks.

### `commands/resources.rs` — `resources_report`

    /// Render `glasshouse resources` — Phase 32B's production caller, and the
    /// reason its boxes are closeable at all.
    ///
    /// # What this function is for, beyond printing
    ///
    /// Phase 32 recorded that `provider::registry::registry()` had no production
    /// caller, and Phase 32A recorded that the launch path reads exactly one
    /// projection out of `CapacityState` — its quota *shape* — with every pool,
    /// window and rate ceiling below that proven only by tests. Both ledgers
    /// named the same missing piece: something in the shipped binary that reads
    /// the model. This is that, and every telemetry reader Phase 32B builds is
    /// reached from here and from nowhere else in the binary.
    ///
    /// # The order of reads, and why the cheap one is not optional
    ///
    /// Harness status first, because it is a local process invocation of about a
    /// quarter of a second that spends no quota and needs no credential — so the
    /// bare command still takes a real reading, and a user who runs
    /// `glasshouse resources` with no flags is not shown a screen of `unknown`
    /// that Glasshouse could have filled in for free. Network probes are opt-in,
    /// matching `glasshouse pairing` and `glasshouse response`, which is the
    /// shape this command was modelled on.
    ///
    /// # It cannot fail on telemetry
    ///
    /// The `Result` here is for reading the user's own configuration files, which
    /// is the same failure every other command in this file can have. No
    /// telemetry read below can produce an `Err`: capability map line 1238 is
    /// enforced in `provider::telemetry` and `provider::resources` by there being
    /// no fallible signature to propagate.

### `commands/resources.rs` — `render_routing_model`

    /// Capability map line 1443 — *"show the currently selected routing model in
    /// resource diagnostics"* — as the last block of `glasshouse resources`.
    ///
    /// # Why this surface, and why it is not the settings screen
    ///
    /// The Settings overlay already renders the configured
    /// [`glasshouse::config::RoutingModelChoice`], and `docs/product/evidence/phase-34c.md`
    /// ruled that showing a value on the screen where you set it is
    /// configuration, not diagnosis. This is the diagnostic surface: the routing
    /// model is named next to the capacity, health and quota of the very
    /// resources it would be chosen from, which is where the question *"why did
    /// routing behave that way"* is actually asked.
    ///
    /// # The honesty constraint, and it is the point of the block
    ///
    /// `Automatic` is an intent — the word the Settings overlay shows — and
    /// naming only that would answer a different question than a person reading
    /// `glasshouse resources` is asking. So the block runs the real decision
    /// ([`automatic_classification_choice`], the same function `glasshouse
    /// classify` calls) and names the resource it picked.
    ///
    /// **And it says `would`, in every arm.** Nothing in this build classifies
    /// anything on its own: `routing::classify::classify`'s only production
    /// caller is the `glasshouse classify` diagnostic, and nothing else asks a
    /// routing model a question. Rendering a "currently selected routing model"
    /// beside live capacity numbers with no signal that it classifies nothing is
    /// the spectacle Phase 47 exists to prevent, so the `in use` row says so in
    /// as many words and is not conditional on anything.
    ///
    /// # No credential, ever
    ///
    /// [`glasshouse::routing::disposable::DisposableChoice`] carries a
    /// [`glasshouse::routing::CredentialId`], and nothing below reads it. A
    /// provider name, a model name and the policy's own explanation are what this
    /// block prints — the same rule `memory::extract::model`'s header states for
    /// the label a classification is attributed to.

### `commands/response.rs` — `response_request`

Orphaned lead-in describing a harness-session launch path, not
`response_request` (the file's only function); moved here rather than kept.

    /// Open a harness session attached to this terminal.
    ///
    /// This is the production consumer of the sanctioned launch path: the harness
    /// is chosen and its executable resolved from configuration (project level
    /// overriding user level), the requested launch profile is resolved against
    /// its adapter (Phase 9A/9F — see [`glasshouse::profile`]), and only then is
    /// anything started through [`HarnessLaunch`] — the only route that exists,
    /// and the one that derives the child's working directory from the active
    /// project rather than from whatever directory Glasshouse happened to be run
    /// in.
    ///
    /// Setup is deliberately not triggered here. A user who has named a harness
    /// has already said what they want; interrupting that with a first-run wizard
    /// would be answering a question they did not ask.

### `commands/resume.rs` — `run_headless`

    /// Run a harness session that never takes this terminal — Phase 4's headless
    /// presentation mode.
    ///
    /// The mirror image of [`session::attach`]. The harness gets a real
    /// pseudo-terminal in the project root exactly as it always does, but this
    /// process's own terminal is never claimed: no raw mode, no alternate screen,
    /// no output relayed to standard output. What the harness prints goes into
    /// the session's own bounded scrollback, which is where an embedded session's
    /// output goes too. That is the whole of "a PTY continues running without
    /// occupying the visible session viewport" from the launch side; the shell
    /// side is `shell::run`, which never makes a headless session the viewport's.
    ///
    /// Glasshouse stays in the foreground for the session's whole life on
    /// purpose. Returning early would drop the [`SessionRuntime`], and with it
    /// the pseudo-terminal the harness is writing to — a detached session needs a
    /// supervisor process, which is a different capability from this one.
    ///
    /// **The terminal queries have to be answered here.** A headless session has
    /// no emulator on the other end: on Windows nothing gets past ConPTY's
    /// startup handshake without a reply, and on any platform a harness asking
    /// `ESC[6n` waits forever for one. [`SessionRuntime`] knows how to answer but
    /// cannot do it from its reader thread, so whoever owns the runtime must — in
    /// the shell that is the tick, and here it is this loop.
    ///
    /// # A signal here is a forced exit, and that is why the cleanup exists
    ///
    /// [`shutdown::install_signal_handler`] ends the process immediately when the
    /// terminal is not engaged, on the reasonable premise that a Glasshouse with
    /// nothing to restore has nothing to wind down. **This path breaks that
    /// premise**: it engages no terminal — that is what makes it headless — and
    /// it owns a child process that stops receiving a hangup the moment Glasshouse
    /// dies. Forced exit calls [`std::process::exit`], which runs no destructor,
    /// so without the registration below a Ctrl-C would leave the harness running
    /// with nothing left able to reach it.
    ///
    /// Found by sending a real `SIGINT` to a real headless launch and looking for
    /// the child afterwards; it was still there. `shutdown`'s own documentation
    /// had already named this as the thing a second caller would have to get
    /// right, which is exactly what this is.
    ///
    /// Deliberately **not** solved by claiming the terminal is engaged. That flag
    /// means "raw mode and the alternate screen are on", and `restore_terminal`
    /// acts on it — setting it here would write escape sequences to a terminal
    /// Glasshouse never touched.

### `commands/resume.rs` — `close_before_forced_exit`

    /// Close `id` on the way out of a forced exit, retrying briefly rather than
    /// once.
    ///
    /// [`glasshouse::shutdown`]'s rule is that a forced-exit callback must never
    /// wait indefinitely: failing to clean up is survivable, failing to exit is
    /// not. A **single** `try_lock` honours the letter of that rule and still
    /// gets the wrong answer. The headless poll loop takes this same lock every
    /// `POLL`, so one attempt is a coin flip, and losing it orphans a real
    /// harness permanently with no second chance — there is no retry anywhere
    /// above this.
    ///
    /// That is not theoretical. It was **measured at 1 orphan in 100 runs under
    /// 3x CPU load**, and it turned up first as an intermittent red
    /// `test (macos-latest)` that passed on rerun against the identical commit.
    ///
    /// A bound keeps the guarantee that actually matters — this returns, always,
    /// and quickly — while removing the coin flip. Poisoning is treated as
    /// ownership rather than as a reason to give up, for the same reason
    /// [`lock`] does: a panicked thread must not strand a live child, and a
    /// poisoned mutex would otherwise make `try_lock` fail for as long as we were
    /// willing to retry.
    ///
    /// Returns whether the runtime was reached.

### `commands/resume.rs` — `evidence_ledger`

Preceded by an orphaned doc for `resolve_resume_overlay` (defined later in
the file, at its own plain `//` comment, undocumented as a doc comment);
moved here rather than kept, since it does not describe `evidence_ledger`.

    /// Re-resolve `profile_name`'s overlay for a resumed session — Phase 9A line
    /// 368's resume half, production caller of `resume_session`.
    ///
    /// Exactly [`launch_session`]'s own resolution: the same lookup, the same
    /// secret store, the same gateway start. A resumed session's overlay is not a
    /// smaller thing than a fresh one's, so there is no separate, weaker path
    /// here for it to take.
    ///
    /// # Errors here are never fatal to the resume
    ///
    /// The caller treats any `Err` as "resume without the overlay, and say why" —
    /// never as a reason to refuse the resume outright. `open_for_resume` has
    /// already proven this session is safe to continue; a bypass acknowledgement
    /// withdrawn since the original launch, or a provider since removed from
    /// configuration, is a reason to fall back to a plain native resume, not a
    /// reason to make an otherwise-healthy session unresumable.

    /// The routing evidence ledger for this project — **only when a gateway will
    /// actually be started** — or `None` with a warning.
    ///
    /// # Why the gate, and what it cost to learn
    ///
    /// The first version opened the ledger unconditionally, before
    /// `start_if_required_with_telemetry` decided whether a gateway was needed at
    /// all. On macOS and Linux that was merely wasted work. On Windows it **hung
    /// six memory-extraction tests indefinitely** — a 37-minute stall with no
    /// output, on a tree whose local gate was 13/13 green.
    ///
    /// [`crate::routing::evidence::EvidenceLedger`] holds `Mutex<Connection>`: an
    /// open SQLite handle for its whole lifetime. SQLite locks with advisory
    /// POSIX locks on Unix and with mandatory `LockFileEx` on Windows, so a handle
    /// this function opened on a launch that never needed it blocks a later writer
    /// on Windows and is invisible on Unix. **Opening a database you may not use is
    /// not free, and the platform that charges for it is not the one this project
    /// develops on.**
    ///
    /// Gating on [`glasshouse::gateway::gateway_is_required`] makes the open happen
    /// exactly when the gateway that consumes it is started, which is also what
    /// `start_if_required_with_telemetry` would have decided a moment later.
    ///
    /// Phase 33A records an observation per forwarded gateway exchange. Opening
    /// its store touches the project database, and both callers evaluate this
    /// **before** `start_if_required_with_telemetry` decides whether a gateway is
    /// needed at all — so this runs on every launch and every resume.
    ///
    /// It therefore must not fail the caller. A launch that refused to start
    /// because a telemetry table could not be opened would trade the user's whole
    /// session for a row nobody is waiting on, and this project's own product
    /// invariant is that Glasshouse orchestrates real harnesses rather than
    /// standing between the user and one. The warning is `tracing::warn!` for the
    /// same reason `set_lifecycle`'s is: it belongs in the log, not on the
    /// terminal the harness is about to take over.

### `commands/resume.rs` — `install_context_firewall_hook`

    /// Map lines 1991-1996: register the context firewall's `PostToolUse` hook
    /// for a Claude Code session, when the effective configuration enables it.
    ///
    /// **Never a second `--settings` flag.** Claude Code 2.1.247 silently
    /// discards every `--settings` but the last (verified in
    /// `session::HarnessSelection::install_session_document`'s own doc), so the
    /// only safe way to add a hook is to merge it into the SAME document
    /// [`install_session_document`] already wrote — this function reads that
    /// file back, adds one `PostToolUse` key, and writes it in place. `args`
    /// itself is never touched, which is what makes `mode = "off"` byte-identical
    /// to a session built before this phase existed: this function returns
    /// before touching anything when the harness is not Claude Code or the
    /// effective mode is `off`.
    ///
    /// Best effort, matching [`install_session_document`]'s own policy: any
    /// failure here is a session that starts without the firewall bridge rather
    /// than one that fails to start, and is logged rather than propagated.
    ///
    /// Map lines 2023/2024: `entitlement` and `backend` are read only to
    /// *classify* the reduction policy (subscription, metered or local) and to
    /// resolve its thresholds through `effective`'s new accessors — never baked
    /// into the registered command line themselves. The firewall core and the
    /// hook subprocess this command line invokes stay entitlement-blind, exactly
    /// as before this package: only numbers and a mode word ever reach them.

### `commands/resume.rs` — `EventRecorder`

    /// Records lifecycle events durably from a command that is about to exit.
    ///
    /// # Why this is not the sink the shell uses
    ///
    /// [`glasshouse::events::EventLogSink`] queues behind a writer thread,
    /// because the shell publishes from a thread that is sometimes draining a
    /// pseudo-terminal and must never wait. None of that applies here: a
    /// `glasshouse hook` process lives for a few milliseconds and then exits, and
    /// queueing behind a thread it is about to drop would lose the event it was
    /// run to record. So this writes synchronously.
    ///
    /// # Why there is a bus at all
    ///
    /// [`glasshouse::events::RecordedEvent`] cannot be built without a session
    /// identifier and a timestamp — that is a property of the type rather than a
    /// habit of its callers, and [`EventBus::publish`] is what stamps both. Using
    /// it as the minting authority is what keeps "record every translated
    /// lifecycle event with session ID and timestamp" true on this path as well
    /// as in the interactive one. No sink is attached to it, so nothing is
    /// written twice.
    ///
    /// # Every failure is swallowed into the log, deliberately
    ///
    /// This runs inside the user's own session — see [`report_hook`], which may
    /// never fail — and it is also on the launch path, where a bookkeeping
    /// failure must not turn into what looks like a harness failure. A project
    /// whose database cannot be opened loses event history and keeps its session.

    /// # Why the log is behind a `Mutex`
    ///
    /// [`EventLog`] owns a `rusqlite::Connection`, which is `Send` and **not**
    /// `Sync`. Since [`DegradeRelay`], a recorder is no longer touched only by
    /// the thread that built it: the gateway's own connection thread reports a
    /// failed upstream through it, so `&EventRecorder` crosses a thread boundary
    /// and the type has to be `Sync` to be shared at all. The lock is what makes
    /// it so, and it is uncontended in practice — the two writers are a launch
    /// path making one bookkeeping call at a time and a gateway thread that only
    /// speaks when its upstream has just failed.

### `commands/resume.rs` — `EventRecorder::degrade`

    /// Record that one backend resource stopped serving — map line 1735's
    /// durable half, on the path the shipped binary actually takes.
    ///
    /// # Why `degrade_resource` is called rather than reimplemented
    ///
    /// Which sessions a failing resource affects is one rule, and it lives in
    /// [`glasshouse::events::degrade_resource`]: *a session is affected if,
    /// and only if, its own record says it resolved to this backend
    /// resource.* Selecting the sessions here instead would be a second copy
    /// of that rule, and it would leave `degrade_resource` with no production
    /// caller again — the exact state the evidence ledger refused this line
    /// in.
    ///
    /// # Why it publishes on a bus that keeps nothing
    ///
    /// `degrade_resource` publishes each `GatewayUnhealthy` on the bus it is
    /// given, and the durable write on this path is [`Self::append`], which
    /// publishes on *this* recorder's bus to mint the record. Handing it
    /// `self.bus` would mint every event twice. A history of zero makes the
    /// bus purely the question-asking apparatus: nothing is kept, nothing is
    /// dropped, and the returned [`glasshouse::events::Degradation`] is the
    /// answer this method acts on.

### `commands/resume.rs` — `DegradeRelay`

    /// Where a gateway failure is recorded, given that the recorder does not
    /// exist yet when the gateway starts.
    ///
    /// # The ownership problem, stated exactly
    ///
    /// [`glasshouse::gateway::DegradeSink`] has to be handed to the gateway at
    /// `start_if_required_with_degrade_sink`, and **both** of this binary's
    /// gateway starts happen before anything the sink needs exists:
    /// `launch_session` starts the gateway 184 lines before it opens its
    /// [`EventRecorder`], and it has no `SessionRecord` at all until the store
    /// has created one. So the sink cannot close over a bus and a session list;
    /// there is nothing to close over. This is the handle it closes over
    /// instead, created before the gateway and filled by [`Self::install`] once
    /// both halves exist.
    ///
    /// # Why the session records are a snapshot, and whose sessions they are
    ///
    /// [`glasshouse::events::degrade_resource`] takes the records it should
    /// consider. This relay is given **the sessions this process owns** — one, on
    /// either path — and not a fresh read of the project's whole session table.
    /// Two reasons, and the second is the load-bearing one:
    ///
    /// - reading fresh would mean a `SessionStore` on the gateway's thread, which
    ///   means a second open connection held for the life of the session for a
    ///   read that fires only when an upstream has failed. §65's Windows hang was
    ///   exactly that shape;
    /// - and a gateway is **per instance**. Another Glasshouse process's session
    ///   is served by *its* gateway, which does its own detecting. Degrading it
    ///   from here would report a failure this process never observed on that
    ///   session's behalf. The narrower snapshot is the honest claim.
    ///
    /// # Lifetime
    ///
    /// The sink holds an `Arc<DegradeRelay>` and the relay holds an
    /// `Arc<EventRecorder>`; neither points back, so there is no cycle to leak.
    /// No thread is started here and none is kept alive: the relay is inert
    /// between calls, and the gateway's own guard is what stops the threads that
    /// call it.

### `commands/resume.rs` — `checkpoint_before_moving`

    /// Check point the session this work is leaving, before it moves —
    /// capability map line 1716.
    ///
    /// `moving_to` is where the work is going: a session identifier when this
    /// launch or resume is continuing one, and `None` when it is starting a new
    /// session. The session being **left** is whichever this project was most
    /// recently active in, which is the same `active_session` rule
    /// `glasshouse checkpoint save` and `Request::TakeCheckpoint` use for "the
    /// current session".
    ///
    /// # Three of the four cases are a no-op, and each says which
    ///
    /// Nothing is being left when this project has no recorded session, when the
    /// launch is starting a fresh one, or when the destination *is* the session
    /// already in hand. Writing a checkpoint for any of those would produce a
    /// handoff describing a migration that did not happen. The flag says so
    /// instead of passing silently: a person who asked for a checkpoint and did
    /// not get one needs to know which of the two occurred, and a silent no-op is
    /// indistinguishable from a checkpoint that was taken (practice §68's shape).
    ///
    /// # It invents nothing, and it fails loudly
    ///
    /// The handoff records only what Glasshouse knows: which session was left,
    /// where the work went, the Git position and this project's binding memories,
    /// all through the same [`Checkpoint::capture`] the two existing checkpoint
    /// paths use. It does not read the session's terminal for an objective —
    /// `checkpoint_command`'s own doc says why that would be a confident fiction.
    ///
    /// A failure here **stops the launch**. The person asked for a checkpoint
    /// before the move; moving anyway would lose exactly what they asked to keep.

### `commands/resume.rs` — `report_task_boundary_routing`

Preceded by an orphaned doc for `resume_session` (defined later in the
file, now undocumented); moved here rather than kept, since it does not
describe `report_task_boundary_routing`.

    /// Reopen a recorded session in its own harness.
    ///
    /// The order here is the safety property. The store decides whether this
    /// session may be resumed *at all* — it belongs to this project, it is not
    /// still running, and it has a native identifier to resume to — before any
    /// harness is selected and long before any process exists. A refusal costs
    /// nothing; a session opened against the wrong project would be a breach of
    /// the isolation the whole product rests on.
    ///
    /// The harness is then whichever one the record names, not whichever one is
    /// configured now: resuming a Codex conversation in Claude Code would be
    /// nonsense, so a record's own harness is what gets selected.

    /// Line 1592's task-boundary caller, and line 1601's explanation on it.
    ///
    /// Prints where the router would have sent this work and what the named
    /// session displaced. Never changes the destination — see `RouteOnResume`.
    /// Everything it needs can fail (the session store, a deleted profile, a quota
    /// cache that will not open), and none of those may cost a person their
    /// resume, so the whole thing is best effort and silent when it has nothing to
    /// say.
    /// **It explains; it does not move the work.** The session was named on the
    /// command line, and a router that answered "somewhere else" would overrule
    /// the most explicit statement a person can make — so the named session goes
    /// in as `RoutingOverride::to`, which is what line 1602 calls a user override,
    /// and the ranking it displaced is printed beside it. Stated as a limit rather
    /// than left to be discovered: **line 1593 is earned on the launch path**,
    /// where the choice is genuinely open, and not here.

## Trims: the remaining module docs — history moved out of comments by `GH-TRIM-REST-DOCS`, 2026-09-05

### `checkpoint/git.rs` — module doc

    //! Where the repository is standing, read cheaply.
    //!
    //! The map asks a checkpoint to *include the current Git branch and commit
    //! when available*, and "when available" is doing real work: a project need
    //! not be a Git repository at all, and Glasshouse must still be able to take
    //! a checkpoint.
    //!
    //! # No subprocess
    //!
    //! This opens two or three small files and parses them. It does not run
    //! `git`, and that is deliberate rather than incidental:
    //!
    //! - a checkpoint can be taken at a task boundary, on a thread that is also
    //!   serving a terminal, and spawning a process there is a latency nobody
    //!   asked for;
    //! - `git` need not be installed for a `.git` directory to exist and be
    //!   readable — a repository cloned onto a machine whose Git was uninstalled
    //!   is still a repository;
    //! - a subprocess inherits an environment, and `GIT_DIR` in that environment
    //!   would silently point this at another repository.
    //!
    //! # The deliberate exceptions, and what they are scoped to
    //!
    //! [`last_change_commit`], [`is_ancestor`] and [`changed_paths`] **do** run
    //! `git`, and the objections above are answered rather than waived. None is
    //! on the checkpoint path: nothing takes a checkpoint through them, and no
    //! thread serving a terminal calls them. `last_change_commit` and
    //! `is_ancestor`'s caller is memory retrieval (`crate::memory::inject`'s file
    //! section and `glasshouse memory search --path`), which is already several
    //! database reads deep and is bounded at one `git log` per path and one
    //! `merge-base` per memory. `changed_paths`'s caller is the guardrail door's
    //! transition handler, bounded to one call per rollback-or-refutation
    //! transition — an assumption ledger write, not a terminal-serving path
    //! either. A machine with no `git`, or a project that is no repository,
    //! makes every one of the three answer `None`, which their consumers render
    //! as *unknown* rather than assuming a clean tree or fresh memory. And the
    //! environment objection is met head-on: all three clear `GIT_DIR`,
    //! `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_COMMON_DIR` from the child
    //! rather than trusting the caller's, so an inherited `GIT_DIR` cannot
    //! silently point them at another repository.
    //!
    //! `changed_paths` does not reuse [`WorkingTreeStatus::detect`] — the index
    //! reader already on this path — because that reader is deliberately bounded
    //! to `MAX_CHANGED_FILES` tracked entries and never reports an untracked
    //! file at all; a preserve set that silently omitted a new, unclaimed file
    //! would be the one wrong direction line 1044 forbids.
    //!
    //! There is no file-reading version of *"which commit last changed this
    //! path"*: answering it means walking the commit graph and diffing trees out
    //! of packfiles, which is a decompressor and a delta resolver, not two small
    //! files. Map line 1142's freshness is worth one bounded subprocess and is
    //! not worth that.
    //!
    //! # Worktrees, which is the case that actually bites
    //!
    //! In a linked worktree `.git` is a **file** holding `gitdir: <path>`, that
    //! directory has its own `HEAD` and its own `commondir`, and the refs live in
    //! the *common* directory rather than beside the HEAD. Glasshouse's own
    //! development happens in linked worktrees, so a reader that only handled the
    //! `.git`-is-a-directory case would have reported nothing in exactly the
    //! situation this project runs in every day. Both shapes are handled, and
    //! both are tested against real fixtures.

### `checkpoint/git.rs` — `fn git_output`

    /// Run one `git` subcommand in `root` and return its trimmed stdout, or
    /// `None`.
    ///
    /// The single place this module spawns a process, so the environment scrub
    /// the module documentation promises is made once rather than remembered
    /// twice.
    ///
    /// - **`current_dir(root)` and no `-C`, no `--git-dir`.** The repository is
    ///   named by the working directory and by nothing a caller can smuggle in.
    /// - **Four variables removed.** `GIT_DIR`, `GIT_WORK_TREE`,
    ///   `GIT_COMMON_DIR` and `GIT_INDEX_FILE` each override the working
    ///   directory, and Glasshouse's own development runs inside linked
    ///   worktrees where at least one of them is routinely set. Inheriting them
    ///   would answer about whichever repository the parent happened to be
    ///   pointed at — silently, and with a real commit.
    /// - **No shell, ever.** `args` are argv elements, so a path is a literal
    ///   however it is spelled; the caller puts a `--` in the list before any
    ///   path so a file named `-n` cannot become a flag.
    /// - **`stdin(null)`.** `git` must never block waiting for input on a path
    ///   whose whole purpose is to answer a label quickly.
    ///
    /// `None` for every way of not getting an answer — `git` absent, not a
    /// repository, a nonzero exit, output that is not UTF-8, an empty answer —
    /// because the one consumer renders all of them as *unknown* and a caller
    /// that could tell them apart would still do nothing different.

### `checkpoint/git.rs` — `fn changed_paths`

    /// Every repo-relative, `/`-separated path the working tree reports as
    /// changed against the index — tracked or not — for the guardrail door's
    /// preserve set (`crate::guardrails::preserve_set`, capability map line
    /// 1044; see `docs/product/design-decisions.md`, *Rollback preserves what is
    /// not yours*).
    ///
    /// `git status --porcelain=v1 -z --untracked-files=all`: `-z` gives NUL-
    /// terminated, unquoted records, which is the only spelling that survives a
    /// path with a space or a non-ASCII byte in it undamaged; `--untracked-files
    /// =all` is what makes a brand-new file the transitioning session never
    /// staged show up at all, which the index-only [`WorkingTreeStatus`] cannot
    /// do. A rename or copy prints two `-z` records — the old path with the
    /// status, then the bare new path — and this reports the new path, which is
    /// what the working tree currently holds at.
    ///
    /// **Not through `git_output`**: that helper answers `None` for empty
    /// stdout, which is exactly what a clean tree prints, and collapsing *clean*
    /// into *unknown* is the one confusion line 1044 forbids — a caller reading
    /// `None` as "nothing to preserve" on an unreadable tree would preserve
    /// nothing when it should preserve everything. So this reads the process
    /// output itself: `None` for every way of not getting an answer (`git`
    /// absent, not a repository, a nonzero exit, output that is not UTF-8), and
    /// `Some(vec![])` only for a clean tree.

## Trims: api, events, harness and config module docs — history moved out of comments by `GH-TRIM-API-EVENTS-HARNESS-CONFIG-DOCS`, 2026-09-05

### `api/mcp.rs` — module doc

    //! - **Project scope** (capability map line 1702). The server binds to the
    //!   [`Runtime`] it was started in and offers no tool argument that names a
    //!   project, a path, a database, or a socket. A session identifier from
    //!   another project's database is refused by `SessionApi::resolve`, exactly
    //!   as it is refused on the socket, because the request reaches that seam
    //!   through the same `dispatch`. This file opens no store of its own —
    //!   `tests/mcp_project_scope.rs` greps it to make sure that stays true.
    //! - **Dangerous operations are explicit** (line 1703). Spawning a session,
    //!   sending it a message, and interrupting it are three separately named
    //!   tools whose descriptions say what they do to a process, never one
    //!   `glasshouse_control` with an `action` field. A harness's own permission
    //!   controls can therefore allow the five read-only tools and ask about the
    //!   three that are not; the MCP tool annotations (`readOnlyHint` and its
    //!   siblings) say the same thing in the form a harness reads mechanically.
    //! - **The caller is a program.** Every message and interrupt this door
    //!   delivers is recorded with `MessageOrigin::Machine`, and no tool accepts
    //!   an `origin` argument that could say otherwise: the field exists on the
    //!   wire for `glasshouse api send`, which knows a person ran it, and an MCP
    //!   client is never that.
    //!
    //! # Hand-rolled on `serde_json`, deliberately
    //!
    //! The handshake and the two tool methods are a few hundred lines; a
    //! dependency that pulled an async runtime into a binary that has none is
    //! the thing this project has refused every time. What is implemented is the
    //! 2025-06-18 revision's stdio transport: newline-delimited frames, no
    //! embedded newlines, protocol on stdout and nothing else on it, diagnostics
    //! on stderr. JSON-RPC batches — removed in that revision — are refused as
    //! an invalid request rather than half-supported. Where the specification
    //! leaves a choice, the conservative reading is taken and stated at the site.
    //!
    //! # What happens when the client goes away
    //!
    //! EOF on stdin ends the read loop and the server returns cleanly. Nothing
    //! here interrupts, stops, closes, or marks the sessions it spawned on the
    //! way out: Glasshouse orchestrates real harnesses, and a client
    //! disconnecting is not an instruction to reap the workers it started — an
    //! orchestrator that wants a worker stopped calls `glasshouse_interrupt_session`
    //! while it is still connected.
    //!
    //! That is a statement about what this module does, not a promise about
    //! what the harness experiences, and the two differ. The sessions' pseudo-
    //! terminals are held by this process, so when it exits the kernel closes
    //! them and each harness receives `SIGHUP` on its controlling terminal.
    //! Measured on macOS with a shell harness: one that handles the hangup kept
    //! running, reparented to init, and saw EOF on its stdin; one that had only
    //! just been spawned died before it ran a line. A harness that takes the
    //! default action on `SIGHUP` — most do — ends with the server. This is the
    //! same fate a `glasshouse api serve` that is killed hands its sessions, and
    //! nothing here can promise more than "not killed by Glasshouse".
    //!
    //! Nothing from a session's output or a memory's body is ever written to
    //! stderr or to a log line by this module; those travel only inside a tool
    //! result.

## pane's screen as a notebook — the user's idea, 2026-09-05 23:20

**The idea, in the user's words:** *since code snippets and objects are its distinctive feature, it
could be looking like a Jupyter notebook — blocks, each turn.* Recorded as a design direction for the
61C/61E screen, not yet a package.

**Why it fits without a new concept.** `runtime-contract.md` §1 already names a turn's program a
**cell**; §3 gives every result a type-directed, size-capped preview and §2 a handle the model named;
§4 records programs and previews in the rollout and never objects. A notebook view renders exactly
those four things in the order they already exist: the cell's program as the input block, its
previews and handles as the outputs beneath it, a thrown result (§5) as the error output of that
cell, and a terminal `return` (the direct-verified-completion delta, `phase-61.md` §61E) as the last
cell's rendered value. The conversation column of line 2449 becomes a column of cells; the telemetry
sidebar is unchanged.

**What it decides and what it does not.** It decides the *shape* of the conversation column — a
sequence of input/output blocks keyed by cell, scrollable, with a cell's outputs collapsible to the
preview line. It does not decide execution (a notebook re-runs cells; pane never re-runs a turn, §5),
does not add a second serialization of any result (2465 — the outputs are the previews the model
already saw), and does not put the model's own text anywhere but the cell that produced it.
**Successor:** `GH-PANE-NOTEBOOK-VIEW` (Amber, pane-lead's lane), after `GH-PANE-61C-FIXUPS` lands the
real `CrosstermBackend` renderer — the fixups worker should shape that renderer as a column of cells
from the start rather than a chat transcript to be re-laid later.
