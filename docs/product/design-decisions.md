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

`ReserveDecisionInputs::task_nearly_complete` is `false` at its one
production construction site (`routing/pressure.rs::reserve_verdict`) and
that is a decision, not an omission.

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
