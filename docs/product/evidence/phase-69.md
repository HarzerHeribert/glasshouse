# Phase 69 — Pane: request modes and the decision model's helpers

Map lines 2637–2646, recorded 2026-09-16 (night) from the user's rulings (`design-decisions.md`, *Request modes, and what the decision model may help with*).

## Non-negotiables

- The decision model proposes; the sandbox profile and the person decide. A mode narrows, never widens; nothing the profile denies is ever allowed by a mode or by an answer.
- Every decision is fail-open: no model, an error or a timeout leaves the session exactly as it is today.
- Certainty about a remote environment comes from the far side's account and Pane's command gate, never from the model.
- No threshold becomes a default before the off/shadow/on measurement (2646) is recorded here.

## Gate

Targeted: the changed files' own targets plus the worker's quoted tests, on the merged tree; Red packages (2637, 2638, 2640) owe the full `cargo test -p pane` and a mutation per decision. The twelve-cell GitHub sweep trails on push.

## Shadow calibration inherited from Phase 66 (2026-09-16)

Five real tasks over Claude Max (`claude-sonnet-4-6`): intents `read_only` 1.00/1.00 and `modify` 1.00/0.98 in 575–791 ms; completion noul 0.94 (done), 0.65 (ambiguous), 0.10–0.22 (answer-only, empty diff — the defect 2641/2642's packet fixes by asking over the answer). `would_hold` 0 in all five.

## Line 2637 — `explore` as a request mode

**State: OPEN** — `GH-PANE-EXPLORE-MODE` (Red, Opus) dispatched 2026-09-17 00:10.

## Line 2638 — `plan` reads

**State: OPEN** — same package.

## Line 2639 — the mode proposed from the intent

**State: OPEN** — packet after 2637 lands.

## Line 2640 — remote commands gated in `explore`

**State: OPEN** — packet after 2637 lands (Red).

## Line 2641 — diff-hygiene questions at the completion gate

**State: OPEN** — `GH-PANE-COMPLETION-JUDGE` (packet validated; dispatch after the preflight package integrates).

## Line 2642 — judge items decided when confident

**State: OPEN** — same package.

## Line 2643 — a drift question before an effectful cell

**State: OPEN** — packet after 2641/2642 land (shares `decide.rs`).

## Line 2644 — the Scout's files ranked by relevance

**State: OPEN** — `GH-PANE-HELPERS-JUDGED` (packet written 00:15).

## Line 2645 — a helper's result checked

**State: OPEN** — same package.

## Line 2646 — off / shadow / on measured

**State: OPEN** — the orchestrator's, last: `pane ruler run` over one request set × three modes.
