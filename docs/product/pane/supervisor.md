# pane — the supervisor (61F)

Ruled by the primary on 2026-09-06 06:10 from the lead's ask
(`.agent-runtime/pane/ask-primary-61f-spec.md`); this section is the producer a
61F packet's FEASIBILITY names. Three map lines: *watch a compressed trajectory
every N turns with a cheaper model and emit one decision: intervene or not* ·
*catch a planted three-turn loop within two turns without a human* · *show
every nudge in the trajectory and in the sidebar*. Tier Amber
(`design-decisions.md`); one Sonnet worker, after `GH-PANE-61E-TERMINAL` lands,
because both touch `session.rs`.

## 1. Configuration: `.glasshouse/pane.toml`, read once at session start

Under the project's `.glasshouse/` directory — the folder Glasshouse already
owns in a managed project — not a new file at the project root. Absent means
defaults. Two tables and nothing else:

    [limits]
    cell_wall_clock_s = 30      # runtime-contract §7's four constants, moved here
    response_bytes    = 16384
    task_tokens       = 400000
    cells             = 40

    [supervisor]
    every   = 4                 # cells between looks
    model   = "<id>"            # no default: unset means the supervisor is off, said once at start
    enabled = true

A value outside a fixed range is refused at start with one sentence. Nothing in
this file can name a tool, a path or a grant — those are the sandbox's
(`sandbox-grants.md`) and stay there.

## 2. The compressed trajectory is the rollout's own cell lines

Since the last look: each cell's program head (its first line), its outcome,
and its call trajectory from `runtime-contract.md` §9.4 (tool, checked
arguments, ended). Never a preview's bytes and never a payload — programs and
outcomes only, so the supervisor sees exactly what the rollout already records
and nothing the model did not.

## 3. The look is one metered request every `every` cells

Through the same wire (`wire::send_turn`) with a fixed supervisor preamble and
a small `max_tokens`, asking for one JSON object `{"intervene": bool, "reason":
"<one line>"}`. Anything unparseable is *not intervene* and is recorded as
such. The request lands in the project's ledger with purpose `supervisor`, so
the ruler can subtract it from a task's cost and a reader can see what the
supervisor spent.

## 4. A nudge is one line at the head of the next user message

The exhausted preamble's slot, the same mechanism: `supervisor: <reason>`.
Recorded in the rollout as a `turn` line with the user role and that prefix
(`RolloutKind` stays frozen; `--resume` needs nothing new). The sidebar shows
it under the budget line. A nudge never ends a task, never changes a grant,
never runs code.

## 5. The planted loop is the acceptance test

A scripted provider answers the same program three turns running; a scripted
supervisor model says *intervene* on the trajectory that shows the repeat; the
assertion is that the nudge heads the user message of the turn after the
second repeat — *within two turns* — and that with `enabled = false` no
supervisor request is ever sent. The mutation: the look's cadence off by one.

## 6. Not decided here

Which cheaper model is the default (none; unset is off), and any supervisor
action beyond a one-line nudge. Both wait for a measured need.
