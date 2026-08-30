
---

# Lines 1452, 1453, 1455, 1456 — closed 2026-08-30 by proof, tests only

Package `GH-ROUTER-INPUT-PROOF`. Phase 34D was 13 open / 0 closed; a read-only
recon (`.agent-runtime/report-router-schema-recon.md`) classified every line and
found most genuinely blocked. These four needed no design decision.

## The caveat, settled

1452 and 1453 were `REACHABLE / ALREADY TRUE` **with a diagnostic-only caveat**:
the chain runs under `glasshouse route`, which ranks and prints and starts
nothing. The worker settled it: **Phase 34D's own heading is "Router request
schema"**, and every line in it is a claim about what the router's *input*
contains, not about what the router then does. Diagnostic-only is therefore not
disqualifying here — unlike map line 372, where the line itself says *"when
automatic routing is enabled"* and the same caveat **is** fatal.

## The negative lines got the harder treatment

1455 and 1456 forbid sending repository contents or session transcripts to the
router. **A negative requirement is the easiest thing in this project to close
falsely** — "we do not do X" passes trivially when nothing does anything — so
the packet required a test that could actually fail, and a mutation that widens
the router input must break it.

## What remains blocked, so nobody re-derives it

Nine of Phase 34D's lines stay open. Most need a structured router-input type
that does not exist. **1457's tier/confidence clause and 1459 are blocked on a
consumer, not a producer**: `task_requirements_from_text` computes a full
`TaskClassification` and keeps only `hard_capabilities()`, and nothing in
`SessionRouter` reads a task's tier or confidence. Closing them means either
inventing a consumer or routing into a different router — a design decision,
recorded in `docs/product/design-decisions.md`.
