# pane — the decision model (Phase 66)

Map lines 2614–2615, `design-decisions.md`'s *Jev is a classifier for Pane
first; routing stays static*. Tier Amber; one Sonnet worker.

## 1. Configuration: `.pane/config.toml`, read once at session start

    [decisions]
    model      = "jev-latest"   # no default: unset means decisions are off, said once at start
    mode       = "shadow"       # "off" | "shadow" | "on"; default "shadow" when a model is set
    hold_above = 0.85           # confidence at or above which a read-only intent holds; 0.5..=1.0

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

## 5. Not decided here

A preflight signal, judge items, an approval line, or supervisor
vocabulary — the candidates the map's Phase 66 paragraph names — are not
built. Which cheaper model is the default (none; unset is off) and any
action beyond one hold both wait for a measured need, the same as the
supervisor's own *Not decided here*.
