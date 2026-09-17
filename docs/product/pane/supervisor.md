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
    cell_wall_clock_s = 30      # runtime-contract §7's runtime limits, moved here
    response_bytes    = 16384
    cells             = 40

    [supervisor]
    every   = 4                 # cells between looks
    model   = "<id>"            # no default: unset means the supervisor is off, said once at start
    enabled = true

A value outside a fixed range is refused at start with one sentence. Nothing in
this file can name a tool, a path or a grant — those are the sandbox's
(`sandbox-grants.md`) and stay there.

Token usage is cumulative telemetry, not a limit. Older files that still name
`task_tokens` are accepted for compatibility, but the value is ignored and
cannot stop a task.

## 2. The compressed trajectory is the rollout's own cell lines

Since the last look: each cell's program head (its first line), its outcome,
and its call trajectory from `runtime-contract.md` §9.4 (tool, checked
arguments, ended). Never a preview's bytes and never a payload — programs and
outcomes only, so the supervisor sees exactly what the rollout already records
and nothing the model did not.

## 3. The look is three layers, cheapest first, every `every` cells

Rewritten 2026-09-17 on the user's ruling — *"Could supervisor be jev? … Don't
hardcode Luna I meant LLM and jev"* — after a dogfooding run (session
`tlitep-13fv`: 247 turns, 120 cells, 11.8M tokens, sixty cells of reading
before the first write) in which every look was a prose request whether
anything was wrong or not. That run would have bought about thirty of them to
hear *no* twenty-nine times.

**No model id is named anywhere in this design.** Each layer is whichever
model its own configuration points at, and each may be absent.

| layer | who | what it answers | what it costs |
|---|---|---|---|
| 1 | nobody — `progress::Stall`, already counted | cells in a row that changed nothing | nothing |
| 2 | the decision model (`[decisions] model`) | one typed `Choice` over the trajectory: `making_progress`, `repeating_a_failing_call`, `looping_over_the_same_reads`, `stopped_without_returning` | a couple of hundred tokens, bounded by `decide::DECISION_TIMEOUT` (2 s), no prose |
| 3 | the supervisor model (`[supervisor] model`, falling back to `[helpers] model`) | the nudge's one line, **only after layer 2 said yes** | one small request, and only when something is wrong |

**Layer 1 is evidence, not a gate.** The stall counter goes into layer 2's
state as `cells_without_change`, and the question is asked anyway: a model
repeating a *failing* call while still writing files reads as progress to a
counter that watches the tree, so gating on it would hide the loop most worth
catching.

**Layer 2 decides; layer 3 only phrases.** A criterion other than
`making_progress` at or above `[decisions] supervision_above` (default 0.85) is
an intervention. Layer 3's preamble asks for a sentence and never for a
judgement, so it cannot quietly overturn layer 2. With no supervisor or helper
model configured the nudge still fires, carrying the criterion's own words —
a worse sentence, never a lost intervention.

**`[decisions] mode` gates whether the question may be asked, not what its
answer may do.** `shadow` exists so a decision cannot change what *runs* — a
hold, a narrowing, a refused cell. A nudge runs nothing and blocks nothing, so
`shadow` and `on` behave alike here and only `off` silences it.

**With no decision model configured, layer 3 decides on its own** through the
original single prose look — one JSON object `{"intervene": bool, "reason":
"<one line>"}`, anything unparseable being *not intervene*. That path is
unchanged, and an unanswerable question at either layer is a failed look:
recorded as such, never a nudge.

Every request lands in the project's ledger with purpose `supervisor` (and the
typed question with purpose `decision`), so the ruler can subtract them from a
task's cost and a reader can see what the supervisor spent.

## 4. A nudge is one line at the head of the next user message

The exhausted preamble's slot, the same mechanism: `supervisor: <reason>` —
at the head of the live feedback, its historical projection **and the native
`tool_result` a tool-calling model receives**. (Decorating only the text
answer dropped the nudge for every session whose model answers with
`execute_cell` as a native call; found 2026-09-17 by the test in §5.)
Recorded in the rollout as a `turn` line with the user role and that prefix
(`RolloutKind` stays frozen; `--resume` needs nothing new). The sidebar shows
it under the task-spend line. A nudge never ends a task, never changes a grant,
never runs code.

## 5. The planted loop is the acceptance test

A scripted provider answers the same program three turns running; a scripted
supervisor model says *intervene* on the trajectory that shows the repeat; the
assertion is that the nudge heads the user message of the turn after the
second repeat — *within two turns* — and that with `enabled = false` no
supervisor request is ever sent. The mutation: the look's cadence off by one.
(`tests/session.rs`, and it is the no-decision-model path of §3.)

The layering has four more, in `tests/decisions.rs`, against a fake that
answers `/v1/systemone` by question key and `/v1/messages` by path: a
confident `making_progress` buys **no** prose request; a confident loop buys
**exactly one**, and that line heads the next turn; with no supervisor model
the criterion's own words nudge instead; and a supervision question answered
with a 500 nudges nothing and buys nothing. The mutation: layer 2's threshold
comparison, killed by
`decide::tests::a_loop_below_the_threshold_is_not_decisive_and_above_it_is`.

## 6. Not decided here

Any supervisor action beyond a one-line nudge — it waits for a measured need.
Which model each layer uses is not decided here either, and deliberately: both
are named by configuration, `[supervisor] model` inheriting `[helpers] model`
when it names none.
