# pane — the decision model (Phase 66)

Map lines 2614–2615, `design-decisions.md`'s *Jev is a classifier for Pane
first; routing stays static*. Tier Amber; one Sonnet worker.

## 1. Configuration: `.pane/config.toml`, read once at session start

    [decisions]
    model       = "jev-latest"   # no default: unset means decisions are off, said once at start
    mode        = "shadow"       # "off" | "shadow" | "on"; default "shadow" when a model is set
    hold_above  = 0.85           # confidence at or above which a read-only intent holds; 0.5..=1.0
    scout_above = 0.85           # confidence at or above which needs_exploration adds a scout signal; 0.5..=1.0
    command_runs_above = 0.85    # confidence at or above which a command line runs unasked on `auto`; 0.5..=1.0

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

**The answer state, the hygiene questions, and judge items (2641/2642,**
Phase 66's shadow calibration found asking about a diff that does not
exist). An empty diff or a `read_only` intent asks the same question over
`{request, answer}` instead. When there is a diff, the same request also
asks five hygiene nouls (`has_tests`, `out_of_scope`, `debug_leftovers`,
`deletes_tests`, `changes_signature`), each a finding held once at
`hygiene_no_below`/`hygiene_yes_above` (0.10/0.90), and one `noul` per
acceptance `judge` item: `judge_yes_above` (0.90) satisfies it without the
checker, `judge_no_below` (0.10) is a finding held once naming the item,
between them the fresh checker still decides it. One request throughout.

**What a confident yes does not prove.** It is one more signal, not a
verdict: the acceptance list and the final-state contract still decide their
own items mechanically, from the tree and the trajectory, exactly as they do
when no decision model is configured.

`--output-format json` and `stream-json`'s `decisions.completion` carries
`{noul, latency_ms, truncated, finding_added, checker_skipped, state,
hygiene, hygiene_findings, judged}`, `null` when the question was never
asked (no model, or `mode = off`); `hygiene` is `null` when `state` is
`"answer"`.

The fresh checker's own return is judged too (2644/2645, `scout_relevance_below`/`helper_no_below`), carried alongside as `decisions.helpers: {ranked, skipped, checked, flagged, latency_ms}` -- `little-helpers.md`.

## 6. Not decided here

Supervisor vocabulary — the remaining candidate the map's Phase 66
paragraph names — is not built (the preflight signal, the completion
question's hygiene and judge extensions, and the approval hint are; see §2,
§5 and §7). Which cheaper model is the default (none; unset is off) and any
action beyond one hold and one scout signal both wait for a measured need,
the same as the supervisor's own *Not decided here*.

## 7. The approval hint (F4)

With a model configured and `mode` on or shadow, the exact-call approval seam
asks the decision model in the background, once per pending call, whether it
fits the request and does nothing beyond it. `mode = on` shows one extra line
beside the confirmation, `fits the request: 0.91 (decision, 640 ms)`; `mode =
shadow` asks and counts it (telemetry's `approval_hints`,
`approval_hint_failures`) but never shows the line. The confirmation is drawn
immediately and never waits for the answer; a failed, slow, or absent
decision leaves it exactly as it renders today. The hint is a line of text
and a counter — it never changes `Decision`.

## 7a. The command-permission question (the model half of `auto`)

The `auto` rung's static reader vouches for the lines it can place -- reads,
and the ordinary build-and-test verbs in `permissions::DEVELOPMENT_COMMANDS`
-- and puts everything else in front of the person. Measured against the 57
distinct command lines of a real 120-cell session (2026-09-17, `tlj14m-24r`)
that is 23 run and 34 asked. The 34 are quote-blind segmentation,
path-qualified programs, `VAR=x` prefixes, and genuinely mutating lines.

With a model configured and `mode = on`, each of those 34 gets one `Choice`
question, `permission`, over `{command_line}` (bounded to 4 KiB), with four
criteria: `reads_only`, `ordinary_development_work`, `needs_a_person`,
`destructive`. At or above `command_runs_above`, the first two let the line
run without asking. **The other two change nothing at all.**

Three properties, and all three are load-bearing:

1. **The static half decides first and is never overruled.** A line it can
   place costs no request and no latency; the model is reached only for the
   lines it could not.
2. **The model can vouch and can never condemn.** `needs_a_person` and
   `destructive` leave the question exactly where it was -- with the person,
   now carrying the model's word for why. A model answer that could deny
   would make `auto` stricter than `accept-edits`, where the person is asked
   and may say yes, and the ladder's one structural property is that a rung
   may only ever *remove* a question.
3. **One answer per command line per session.** A model answer is precisely
   the kind that could have been given differently, so it is remembered
   (`approval::Gate::vouched`) and a retry cannot turn a question into a run.
   A timeout is *not* an answer and is not remembered, so a slow gateway
   never bars a line for the rest of the session.

**The lines a cell already spells out are judged together, before it runs.**
A cell is a program and a program can be read: `runtime::commands` walks the
submitted source and reports every `bash({command: "…"})` whose line is a
string literal or a substitution-free template, in source order, up to 32 of
them. Those go to the decision model at once, in parallel, and the answers
land in the gate's own memory keyed by the same exact line — so the calls
that follow read them instead of asking again, and a cell with six unplaceable
lines waits once rather than six times. **Only what is certain is read out**:
a line assembled from a variable, a call or a template with a substitution in
it is not reported at all and meets the gate as it always did, because
pre-answering a question about a line that never runs is worse than asking
nothing. Nothing is granted here and nobody is asked: a confirmation belongs
beside the call that needs it. This runs on the `auto` rung alone — the rungs
that confirm every command line must not have their questions pre-answered,
and `full` asks nothing to begin with. **Limit:** the gate exists only in a
session with a terminal to ask at, so a headless `--task` run pre-judges
nothing, which is also the run where nobody would have been asked.

`shadow` asks and records but does not act, unlike the supervisor's nudge:
letting a line run without asking *is* changing what runs. `off` does not
ask. Without a model the rung is exactly its static half, which is the honest
floor.

## 8. The drift question (2643)

Once the intent hold above returns `Run` for an effectful cell or frame, and
the plan has an `Active` step (the first, if several), Pane asks one `noul`,
`drift`, over `{request, step, cell}` (`cell` bounded to 8 KiB, cut at a
line): does the cell do what the step says and nothing else. A confident no
(`noul <= drift_no_below`, default `0.10`, `0.0..=0.5`) holds the cell once,
naming the step and the answer; re-issued, it runs (`drift_holds == 0` spent,
not asked again). `mode = shadow` runs the cell and counts `would_drift`; a
failed or slow decision counts `drift_failed` and runs the cell, no second
timeout. `decisions.drift` carries `{asked, held, would_hold, failed}`.

## 9. The proposed mode (2639)

`[decisions] mode_above` (default `0.85`, `0.5..=1.0`): at or above it, a
`read_only` intent in `execute`, unpinned, with a model configured and `mode
!= off`, narrows one request to `explore` (`session.mode` unchanged) and
prints `decision: explore for this request (read_only 0.97); /mode execute
to pin`. Between `0.5` and `mode_above` it offers instead: `decision:
read_only 0.71 below mode_above; /mode explore to pin`, and the request runs
in the session's mode -- a blocking question would stall a scripted session.
`mode = shadow` counts `would_apply` and prints neither line. `/mode <m>`,
`--mode`, `--plan` and Shift-Tab pin the session's mode against a proposal;
`/mode auto` unpins (default: unpinned). `decisions.mode_proposal` carries
`{proposed, applied, would_apply, pinned}` -- `decisions.mode` already names
the off/shadow/on string.
