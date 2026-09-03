# Dogfooding — the lane the Decompression ruling opened

> Process, not product. Capability requirements live only in `docs/product/capability-map.md`.

**Why this file exists.** Every real defect this project found was found by
running the shipped binary in a real terminal, never by a unit test
(measured, and the user's ruling of 2026-09-03 says the same: mutation tests
prove a test watches a line, not that the Claude/Codex/provider chain works).
The ruling makes it a lane: **one real session per working day**, the shipped
binary driving a real harness on a real project for at least an hour, with
the orchestrator watching memory extraction, routing, the context firewall
and the shell — and findings filed here, then packaged **by risk**.

## The protocol, short

1. Build the binary from `main` (`cargo build -p glasshouse`), run it from a
   scratch checkout of a real project (this repository is fine), in its own
   cmux pane, through the normal `glasshouse` entry point — never a test
   harness, never a fake provider.
2. Launch a real Claude Code session through it; give it an hour of ordinary
   work (a small task with edits, a search, a failing test). Watch: the
   launch briefing (file-observed and file-referenced sections), the firewall
   hook's reductions and its `file_touched` rows, the memory extraction at the
   session's end, `glasshouse memory search --path <a file it edited>`,
   `glasshouse route`, `resources`, `entitlements`, and the shell's activity
   view.
3. Record below: date, commit, harness and version, what was done, **every
   surprise** — a wrong number, a stale label, a slow step, a message that
   did not help, a crash — with the exact command and output. No prose about
   what worked beyond one line.
4. Each surprise becomes a packet sized by its mechanism and tiered by risk,
   or a register row saying why not. The ledger entry for the packet links
   back to the session here.

## Sessions

### 2026-09-03 — `2d1dc4d`, claude-code, read-only surfaces only

Scratch project: a worktree of this repository. Covered: `sessions`, `route`,
`route --help`, `resources`, `entitlements`, `doctor`, `sessions show`, and
`launch` up to the point it refuses.

**No defect found.** Three things were checked against a suspicion and the
suspicion lost each time — recorded because "I looked and it was fine" is the
result, not an absence of one:

- `route` and every `launch` print a 21-line *why* of which 19 are `+0.000`.
  It reads as noise on a first run. It is **deliberate**: `commands/route.rs`
  states that a reader must be able to tell "provider health was equal" from
  "provider health was never read", so an inert term is named rather than
  dropped. Settled decision, no contradictory evidence — **curiosity, not a
  package.**
- A launch that fails records `state failed` and `sessions show` gives no
  reason; `SessionLifecycle::Failed` carries no payload. But the CLI **does**
  say why at the time, and says it well: *"a harness session needs a terminal
  on both standard input and standard output; run this from an interactive
  terminal rather than through a pipe or a redirect."* The user is told. Later
  diagnosis from the record alone is weaker — **debt**, successor below.
- `resources` prints a raw epoch beside the useful relative age
  (`last observed unix 1788430539 (0s old; provider limit 900s)`) —
  **curiosity.**

**Successor (debt, do not schedule on its own):** if a second session's record
ever has to be diagnosed after the fact, give `SessionLifecycle::Failed` a
reason and surface it in `sessions show`. One occurrence, and the live path
already reports correctly.

**What this session did not cover, and why.** The interactive half — a real
harness doing real work, with memory extraction, the firewall hook and the
shell watched — did not run. Both recorded sessions are this orchestrator's
own non-TTY invocations, correctly refused. `glasshouse launch` needs a
terminal on both ends, and this context cannot drive an interactive TUI or
read a pane back. **The lane needs a human-attended run**, or a harness
started in a pane by hand and left to work; that is where the defects this
lane exists for actually live, and none of them can be reached from here.
