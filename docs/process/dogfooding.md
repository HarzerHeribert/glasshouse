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

_None yet. The first is due the first working day after 2026-09-03; the
orchestrator runs it when the decomposition workers are not saturating the
machine, because a loaded host hides the latencies this lane exists to see._
