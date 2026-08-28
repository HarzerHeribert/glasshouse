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
  Map line 1611 and the Product Rule at line 2201 still stand.

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
