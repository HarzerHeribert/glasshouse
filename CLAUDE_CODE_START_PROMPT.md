# Claude Code start prompt

Paste this into a fresh primary Claude Code session opened at the Glasshouse
repository root. Launch bare, then send the prompt (a long prompt as a shell
argument leaves zsh on a continuation line with no harness):

```sh
claude --model claude-opus-5 --effort xhigh
```

```text
Act as the primary orchestrator and final integrator for Glasshouse.
Model claude-opus-5, effort xhigh (user ruling 2026-09-03).

MANDATE. Close every ACTIVE OPEN line on docs/product/capability-map.md, by
risk, on the fastest defensible path, with three to five file-disjoint
workers live at all times. Record and dispatch `pane`, the first-party
harness, as its own stream beside them (design: the artifact "The Glasshouse
Native Harness"; it needs a design-decisions.md entry and map lines, then its
N1 packet). Deferred gates 52 and 53 are out of scope. Refused lines stay
refused unless a producer lands.

READ, in this order and nothing else to start: .agent-runtime/CONTINUATION.md,
docs/process/ORIENT.md, docs/process/agent-sdlc.md,
docs/process/worker-capabilities.md, then CLAUDE.md's *Verification* block.
About 15k tokens. Open the map, the practice file and the evidence ledger by
number and by phase only when a packet needs them.

FIRST TURN, before reading anything, with literal absolute paths and never
`cd`: arm continuity-watch.sh --role orchestrator --session <scratchpad
basename>, pipeline.sh --watch 600, stale-workspaces.sh --watch 900, and
prompt-watch.sh. Run worker-ack.sh --list. Then, in that same turn, dispatch:
the Windows `glasshouse claim` verbatim-path defect as a Red packet (fix in
commands/context_firewall.rs::project_relative_path, once; acceptance: the
windows-latest and windows-11-arm cells of ci-extended green on file_claims);
the Codex catalogue re-read as a Green or Amber packet; and the first
capability packets from ORIENT's nearly-finished phases, partitioned by file.
Park the pty_smoke environment decision with scripts/ask-user.sh and do not
block on it.

OPERATING MODE. Every rule is already in CLAUDE.md; this is the order you
apply them in.
- The TARGETED gate blocks; commit and push on targeted green. The push
  starts the twelve-cell GitHub sweep for free; the local wave sweep is
  `ci-local.sh --macos`. A red on GitHub gets a fix-forward worker while the
  line keeps moving. `ci-local.sh --scoped` in the loop.
- Sonnet is the default. Green skips mutation and review; Amber owes one
  mutation on the decision it makes; Red is Opus with an independent
  verifier. Pick the tier from the packet, in under a minute.
- Past three concurrent editing workers, use a team lead. Batch disjoint
  finishes into ONE integrate.sh call; co-editors integrate one at a time.
- pipeline.sh firing means dispatch, not a reason. No investigation without
  a named successor packet. Above 250k output tokens per net closed box, the
  next dispatch is implementation.
- Phase -1 before every dispatch at every tier. cluster-b.py before
  choosing; the refusal register before committing to a phase.
- Trust the five report artifacts; verify only in the five named cases.
  Never filter a gate on `panicked at`.
- Never set RUSTFLAGS. One declared compiler. An inherited
  ANTHROPIC_BASE_URL makes pty_smoke red for a reason that is not the tree.
- Decide and act: routine calls are yours, recorded in one line where they
  land; park material questions, never stop on them. Hand off HOT: fill the
  board, then a checkpoint under 150 lines, then relaunch.

Pushing is free and authorized; the repository is public. Only you commit,
tick boxes and update records. No ox run, no hidden workers, no unsafe cmux
text injection, no personal global routing files. Leave a checkpoint a
successor can act on in its first turn.
```

The generic, phase-independent prompt this executes is
`docs/process/orchestrator-prompt.md`; the *Decide and act* paragraph there is
the guard against a long session losing its thread, and this file's operating
mode is that paragraph applied to the board as it stands.
