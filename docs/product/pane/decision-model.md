# pane — the decision model (Phase 66)

Map lines 2614–2615, `design-decisions.md`'s *Jev is a classifier for Pane
first; routing stays static*. Tier Amber; one Sonnet worker.

## 1. Configuration: `.pane/config.toml`, read once at session start

    [decisions]
    model       = "jev-latest"   # no default: unset means decisions are off, said once at start
    mode        = "shadow"       # "off" | "shadow" | "on"; default "shadow" when a model is set
    hold_above  = 0.85           # confidence at or above which a read-only intent holds; 0.5..=1.0
    scout_above = 0.85           # confidence at or above which needs_exploration adds a scout signal; 0.5..=1.0

A value outside its range is refused at start with one sentence. `model`
names no tool, path or grant, exactly as `[supervisor] model` does not.

## 2. What a decision is, and is not

A decision is a typed question over the gateway, answered with a choice, a
probability distribution and a confidence — never free text. It is asked
once per task, before the first turn: *what does this request intend?*, with
criteria `read_only`, `modify`, `run`, `other`. It never sets a tool's
`Purity`, adds or removes a sandbox grant, answers an exact-call approval, or
proves a command-lifting equivalence. A failed, slow (past 2 s) or absent
decision leaves the task exactly as it is without one — recorded, never
surfaced as a task failure.

Beside intent, the same request asks one complexity question — how much
exploration this request needs — with criteria `trivial`, `routine`,
`needs_exploration` (F2). A `needs_exploration` answer at or above
`scout_above` adds `preflight::SIGNAL_DECIDED_EXPLORATION` to
`should_scout`'s signals when `mode = "on"`: it can only add a reason to run
the preflight scout, never remove one of the four deterministic signals, and
a `trivial` answer never suppresses one either. `mode = "shadow"` records
what would have happened (`would_scout`) and changes nothing; a failed, slow
or absent decision leaves preflight exactly as it is today.

## 3. The hold

`mode = on` holds, once, the first cell or direct frame that names an
effectful capability (`bash`, `write`, `edit`, `checks`, `agent`, `mcp`)
when the intent is `read_only` at or above `hold_above`. The model reads one
`## Held (decision)` block in place of running it; the same call re-issued
runs, because the once rule (`effect_holds == 0`) has already been spent
this task. `mode = shadow` runs every cell as it would today and only counts
what would have held. Neither mode changes a grant, a `Profile`, a `Purity`,
or an approval `Decision` — a held call re-issued passes exactly the checks
it would have passed before.

## 4. Telemetry and the inspector

`--output-format json` and `stream-json`'s result carry `decisions: {model,
mode, asked, answered, failed, latency_ms_total, intent: {choice,
confidence} | null, would_hold, holds, overrides}`, `null` only when no
decision model is configured. The `/cell` inspector shows one line for the
task: `decision: read_only 0.94 · holds 1 · overrides 0`, or `decision:
none`. No new `RolloutKind` — the task-start notice
(`decision: intent read_only (0.94, 180 ms)` or `decision: no answer
(timeout after 2000 ms)`) is the durable record in the conversation.

## 5. The completion question

Map line 2616. Before a claimed completion is accepted, Pane asks the
decision model one `noul` question -- does the task's diff satisfy the
request, with nothing asked for missing and nothing unasked changed -- and
uses the answer in two places only:

    [decisions]
    completion_no_below  = 0.10   # 0.0..=0.5; noul at or below this is a finding
    completion_yes_above = 0.90   # 0.5..=1.0; noul at or above this spares the checker

A confident no (`noul <= completion_no_below`, `mode = on`) adds a
`RequestNotSatisfied` finding beside the mechanical ones: the candidate is
held once, and the same claim again finishes with the completion recorded
unverified -- never a refusal. A confident yes (`noul >=
completion_yes_above`, `mode = on`, no other finding present) skips the
fresh checker for this claim when one is configured, recorded as
`checker_skipped` in telemetry; it never removes a finding the mechanical
checks or the acceptance list already made. Between the two thresholds, or
with any other finding present, the fresh checker runs exactly as it does
today. `mode = shadow` asks and records the answer; it never adds a finding
and never skips the checker. `mode = off` or no model configured is
byte-identical to today. The question is asked once per distinct diff
claimed at the gate -- an identical second claim of the same candidate (the
hold-once case above) reuses the cached answer and asks nothing, but a diff
that changed since the cached answer (the model fixed something and claimed
again) is asked fresh, never judged against the stale answer to a tree that
no longer exists. The diff sent with it is bounded to 64 KiB, cut at a hunk
boundary.

**What a confident yes does not prove.** It is one more signal, not a
verdict: the acceptance list and the final-state contract still decide their
own items mechanically, from the tree and the trajectory, exactly as they do
when no decision model is configured.

`--output-format json` and `stream-json`'s `decisions.completion` carries
`{noul, latency_ms, truncated, finding_added, checker_skipped}`, `null` when
the question was never asked (no model, or `mode = off`).

## 6. Not decided here

Judge items, an approval line, or supervisor vocabulary — the remaining
candidates the map's Phase 66 paragraph names — are not built (the preflight
signal is; see §2). Which cheaper model is the default (none; unset is off)
and any action beyond one hold and one scout signal both wait for a measured
need, the same as the supervisor's own *Not decided here*.
