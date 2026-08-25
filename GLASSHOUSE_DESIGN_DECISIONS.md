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

