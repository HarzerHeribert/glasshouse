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
