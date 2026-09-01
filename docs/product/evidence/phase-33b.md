# Phase 33B — Reliability-adjusted agent performance

**0 of 14 closed at the time of writing. This file exists because the phase had
no evidence-ledger entry at all** — `discover.py --phase 33B` reported *"no file
paths found"* — and nobody had ever established what blocks it. A read-only
census (`GH-RECON-33B`, 2026-09-02, Sonnet high) did that.

**The fourteen lines reduce to four root causes, and the causes are the package
boundaries** (practice §87). Two are refusals; two are packageable and one of
them was dispatched the same night.

---

## Cause A — the relay-path body-parsing wall. REFUSED; do not package.

**Lines: 1347, 1348, 1349, 1350, 1351, 1352, 1355, and 1354 in part.**

`gateway/ingress.rs:36` states the relay is *"structurally incapable of carrying
a body"* and `:647` declines to parse the response stream. `main.rs:~8612`
states `routing_observations`' token columns have had **no writer since
migration 11** for the identical reason. **Both re-verified word-for-word
against current source during the census** — neither has rotted.

TTFC, TTFT, decode throughput and rounds-per-minute are therefore not merely
unwired: **the producer absence is already documented and mutation-tested in
production.** `shell/state.rs:695-696` and `shell/view.rs:1017-1021` both say
verbatim that those five measures *"have no producer on this gateway at all,"*
and `shell/view.rs`'s `no_fabricated_columns_appear_in_the_route_evidence_table`
asserts none of those strings ever render. `cluster-b.py` finds no
`effective_ttfc`/`reliability_adjusted_latency`-shaped symbol — **these are not
built-and-unwired functions; the symbols do not exist**, which is a stronger
finding than Cluster B's shape.

This is the register's **Cluster L / row P1b** blocker — the same wall holding
Phase 33A's 1331-1334 and line 1263. Any package here fails Phase -1 on link 1.
**Successor: none. It needs the `ingress` design ruling first.**

**1354 is half-built and worth knowing about.** `FailureClass` already has
production-constructed, production-consumed `EmptyCompletion` and `StreamAbort`
variants (`gateway/session.rs:1178-1196`), classified from HTTP status and
framing — **no parsing wall applies to those two**. What is missing is *"unusable
tool calls"* and *"apparently successful but non-actionable turns"*, which need
body/tool-call content. The line cannot tick on two of four, but Cause A's
eventual package should reuse those two variants rather than rebuild them.

---

## Cause B — routing scoring weights are compile-time constants with no config surface. PACKAGEABLE.

**Lines: 1357 (in part), 1358.**

`QUOTA_PRESSURE_WEIGHT` (`routing/session.rs:948`),
`CLASSIFICATION_PREFERENCE_WEIGHT` (`routing/disposable.rs:404`), and
`HEALTH_FAILURE_PENALTY` / `HEALTH_PENALTY_FLOOR` / `HEALTH_UNAVAILABLE_PENALTY`
are all `const`. Nothing under `src/config` names a routing score weight —
contrast `routing.capacity_band_thresholds` (`routing/pressure.rs:63`), which
**is** user-configurable and is the pattern to copy.

**1357's term-preservation clause is already satisfied**: `Contribution` and
`RoutingExplanation` (`routing/mod.rs:444,481`) preserve every score's exact
inputs, terms and evidence string. Only the weight-configurability clause is
open. **1358** ("treat the OX gateway scoring model as evidence and a
configurable starting policy, not a universal constant") has no symbol of that
name anywhere in-tree and is the same underlying gap: treating the weights as a
starting policy requires them to be configurable first.

**Successor, named and ready:** add config fields for the routing score weights
(start with `HEALTH_FAILURE_PENALTY`/`HEALTH_PENALTY_FLOOR` and
`CLASSIFICATION_PREFERENCE_WEIGHT`, layered like `capacity_band_thresholds`),
and thread the configured value into `provider_health`
(`routing/session.rs:2267`, called at `:4634`) and the disposable scorer
(`routing/disposable.rs:1747-1828`). **The one test:** set a non-default weight
via config and assert the resulting `Contribution::magnitude()` for an otherwise
identical routing decision changes from the constant to the configured value —
proving the override reaches a real decision, not just a struct field.

---

## Cause C — already implemented in production, never ticked. PACKAGEABLE, and dispatched.

**Lines: 1353, 1359.** No production change needed; both need a test that
watches what already ships.

- **1353** — `provider_health` (`routing/session.rs:2267`, production call site
  `:4634`) already applies `consecutive_failures as f64 * HEALTH_FAILURE_PENALTY`
  floored at `HEALTH_PENALTY_FLOOR`. That is a genuine **additive per-failure**
  penalty folded into the routing decision, not a boolean.
- **1359** — the coarse fallback is not a special case, it is **the only path
  that has ever run**: Cause A means structured events never arrive.
  `main.rs:~4468-4471` already writes a `NewObservation` with
  `ROUTING_LATENCY_PURPOSE` and `with_timing`, and `gateway/session.rs:470-477`
  records dispatch/completion/outcome/failure-class/failovers/retries on every
  real exchange.

**Dispatched as `GH-COARSE-FALLBACK` (Green) on 2026-09-02**, with a mutation per
line: `failure-penalty-is-flat` (turns the additive penalty into the boolean the
line distinguishes it from) and `coarse-observation-skipped`.

---

## Cause D — restraint lines with no forbidden mechanism in the tree. REFUSED.

**Lines: 1356, 1360.** Cluster P/Q shape — *"name the code path that could do
the forbidden thing; if you cannot, the line is not closeable and saying so is
the finding."*

- **1360** (*never infer precise TTFC or token timing from terminal text*) — the
  census searched `harness/`, `shell/`, `pty/` and the gateway/routing tree for
  any terminal-text-to-timing inference and found **nothing**. `pty/mod.rs`
  exposes raw bytes; nothing parses them for timing or token boundaries anywhere
  in production or tests. The restraint passes **vacuously**, which is exactly
  the shape that un-ticked 1455/1456.
- **1356** (*do not compare TTFC across materially different tool requirements
  unnormalized*) — nothing computes or compares TTFC at all, so there is no
  comparison to be non-compliant. Filed here rather than under Cause A because
  its resolution differs: **even once TTFC exists, this line stays open until a
  comparison or normalization code path exists to test.** That is a second,
  later gap.

**Successor: none.** Re-check 1356 after Cause A's `ingress` ruling, since a
producer may introduce the first comparison path it could bind to. **1360 has no
dependency on Cause A** — closing it would require someone to *build* terminal-
text timing inference, which is the outcome the line forbids, so it is most
likely a standing restraint rather than a future package.

---

## A note on the restraint-line ruling, so it is not misapplied here

A ruling landed the same night (`phase-29.md`, line 1152) that **restraint lines
are mutation-proven by violating the restraint** — the defect a restraint line
forbids *is* an addition. That ruling closed 1152 and it is correct.

**It does not rescue 1356 or 1360.** The 1152 restraint had *both* things it
separates actually built (a memory store and a checkpoint store, each with a
live command), so the violation was a real defect a real test could catch. Cause
D's lines restrain a mechanism that does not exist, so the "violation" would be
building the forbidden feature in order to forbid it. **The distinction is
whether the restrained thing exists**, and it is worth stating because the two
shapes look identical from the map line alone.
