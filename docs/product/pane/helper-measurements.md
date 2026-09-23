# Helper measurements — the grounded basis (2026-09-23)

Which helper runs, when, and on what model — Jev, the Scout, the little
helpers — decided by three levers: **accuracy, speed, cost**. Every figure
below was measured on 2026-09-23; the raw attempts are kept by the runs'
`--out` directories and the offline sets are in
`crates/pane/examples/decision_eval_cases/`. **Read the limits (§5) before
acting on a number.**

## 1. Jev (the decision model), offline — Pane's own questions, two runs each

| question | cases | result | latency |
|---|---|---|---|
| kind (explore/fix/implement/question/run) | 48 labelled requests | 88 % right; 100 % the same across runs; at confidence ≥ 0.7: 97 % right on 79 % of cases; ≥ 0.85: 100 % on 75 % | median 290–300 ms |
| intent (read-only/modify/run/other) | same 48 | 96 %; one false read-only ("what is 2^20") | same request |
| field shape (log/listing/source/prose/data) | 29 real texts | 97 %; "log" never wrongly claimed | ~310 ms |
| enough (is a return enough to act on?) | 24 labelled returns | cleanly separated: noul median 0.72 when enough, 0.11 when not; best threshold ≈ 0.25 (100 %); today's 0.40: 83–88 % | ~330 ms |
| complexity (needs exploration?) | 35 cases, the same requests cold / with an AGENTS.md naming commands / as follow-ups | 23/35 with **and without** the session's context; leans to "needs exploration" regardless ("now fix it" as a follow-up: 0.97) | same request |

**Decision:** Jev decides kind, intent and field shape. It does not decide
whether exploration is needed — that is the session's count of earlier
requests and whether instructions name commands, both known exactly.

## 2. The Scout, offline — does it name the files the acting model needed?

Ground truth: files read in ≥ 3 of 6 baseline attempts (X1: 8 files, X2: 6).

| variant | X1 recall | X2 recall | time | tokens |
|---|---|---|---|---|
| tool loop (today's Scout, recorded) | 0.62 | 0.17–0.33 | ~60 s | 150–350k |
| one-shot over the file listing: opus-5-5 | 0.88 | 0.17 | ~8 s | ~17k in / 0.8k out |
| one-shot: gpt-6-luna | 0.75–0.88 | 0.00 | 10–16 s | ~10k / 0.5k |
| one-shot: gpt-5.6-luna | 0.62–0.75 | 0.00 | 13–17 s | ~10k / 0.6k |
| one-shot + files matching the request's words | ≈ same | ≤ 0.33 | same | +1k |

Names reveal the files for X1 and not for X2 (the needed files hold
"budget" and "page", words the request does not use): no variant finds
those.

## 3. End to end (the ruler), parent gpt-5.6-sol, helpers gpt-5.6-luna

Arms: **bare** — no helper, no decision; **shadow** — defaults (acceptance
list and completion check), Jev asked but not acted on; **scout** — plus the
span Scout on every request; **dissect** — Jev acts (`mode = on`), the tool-
loop dissection on explores; **oneshot** — `dissect` with the one-shot
dissection (after the fix in e5546dca that serves the files it names).

**Accuracy: every attempt that ran passed its own check** (fix F1, Rust I2
with a hidden test, Python I1, docs W1, rename E1, explore X1/X2) — the
tasks are too easy for a strong parent to show a helper's accuracy value.

| task (n) | bare | shadow | scout | dissect | oneshot |
|---|---|---|---|---|---|
| F1 fix (3) wall · parent tokens | **231 s · 510k** | 403 s · 514k | 412 s · 411k (+295k helper) | 619 s · 851k | — |
| I2 Rust (3) | **201 s · 128k** | 379 s · 317k | 294 s · 178k (+136k) | 367 s · 155k | — |
| E1 rename | 482 s · 1165k (3) | 383 s · 762k (2) | quota | quota | — |
| I1 Python (3) | 87 s · 96k | **72 s · 53k** | 83 s · 81k | 126 s · 89k | — |
| W1 docs (3) | **98 s** · 329k | 153 s · 538k | 154 s · 328k | 121 s · 378k | — |
| X1 explore (5) | **182 s** · 276k | — | — | 250 s · 262k (+228k) | 222 s · **230k** (+14k) |

What the helpers did in those runs:

- **Completion checker:** every finding it raised was on an attempt that then
  passed its own check (I2 2 of 3, W1 in `on` 3 of 3, F1 in `on` 3 of 3) —
  false alarms, each costing turns. No attempt failed, so its value on a
  real failure is unmeasured.
- **Read-only hold (`mode = on`):** 3 holds on X1, all 3 overridden by the
  model re-running the cell unchanged. Mode `on` was the slowest arm on
  every editing task.
- **Effort lowered for explores:** more cells and tokens, not fewer (A/B #1).
- **Prefetch:** Jev answered "not enough" 20/20 — correctly, per §1 — but
  prefetching three whole files each time doubled the parent's tokens
  without saving a read.
- **Dissection:** fewer cells on explores (X1 4.2 → 3.8 tool loop, 2.8 one-
  shot); the one-shot is ~16× cheaper than the loop for the same or better
  effect; both slower than bare by 40–70 s.

## 4. What this recommended — ruled and built 2026-09-23 (*Lanes, not gates*, design-decisions)

1. **Defaults for a strong parent model: no helper on the critical path.** On
   every editing task bare was fastest and cheapest at equal accuracy. The
   completion checker and the acceptance list stay available and off by
   default until a task set where the parent fails shows they catch it.
2. **Jev stays**, for what it is measured to decide: kind (≥ 0.7), intent,
   field shape. `mode = on` actions that stop the model (the read-only hold,
   the explore narrowing, lowered effort) are turned off; Jev's answers
   become advice lines, never stops.
3. **Explores get the one-shot dissection** when the kind is `explore`, the
   request is the session's first, and the project is a git repository;
   the tool-loop Scout stays opt-in.
4. **Prefetch and the effort lease are off**; `enough`'s threshold, if
   prefetch is revisited, is 0.25, and it would prefetch one file, not three.

## 4a. Where the parent's context goes (offline, the bare runs)

Weighted by how often each part is resent: the system prompt and project
instructions about 60 % (52 KB of its 82 KB is this repository's own
CLAUDE.md), tool results 25–30 % — search hits and listings the largest part,
then file text (~14 %), then command output (~7 %) — and the cells' own
source the rest. **A helper can save at most the tool-result share**, which
matches the 10–19 % parent-token drops measured where a Scout helped. Priced
at list rates (Sol $4/$20, Luna $0.20/$1.20 per M), Luna's own tokens never
mattered: 1–15 cents per task.

## 5. Limits — read before acting on a number

- **n is small (3–5 per cell) and the spread is wide:** main-model tokens on
  X1 have a standard deviation of about 35 % of the mean across 9 runs. A
  difference under ~40 % at n = 3 is not established. The consistent
  *direction* across five editing tasks is the evidence in §3, not any one
  cell.
- **One parent model** (gpt-5.6-sol) on **one repository** (this one). A
  weaker parent, an unfamiliar repository or tasks the parent fails can
  change every row; the next measurement is exactly that (Claude parent,
  harder tasks), because accuracy is the lever these tasks could not move.
- The offline labels (§1) are one person's; the complexity result shows how
  much a label can assume that the question does not carry.
- Runs after ~11:30 on the ChatGPT subscription hit its weekly limit; those
  attempts are `errored`, never counted as failures.
