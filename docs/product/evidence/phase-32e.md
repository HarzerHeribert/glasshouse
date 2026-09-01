# Phase 32E — Burn rate and exhaustion forecasting

**0 of 10 closed at the time of writing. This file exists because the phase had
no evidence-ledger entry at all** — `discover.py --phase 32E` reported *"no file
paths found"*. A read-only census (`GH-RECON-32E`, 2026-09-02, Sonnet high)
established what blocks it, grouped by root cause per practice §87.

**The register's standing claim was that *"most of 32E"* waits on row P1b's
relay-path usage reader. That is wrong in the direction that matters: it is
*one* of ten lines.** Eight are packageable or need a ruling; only 1275 is held
by the ingress wall.

| line | cause | verdict |
|---|---|---|
| 1274 | **4** — already true when measurable | **packageable (proof-only)** |
| 1275 | **2** — ingress relay wall (P1b) | refused |
| 1276 | **1** — `task_class` computed and never persisted | **packageable** |
| 1277-1280, 1282 | **3** — no burn-rate symbol, but every input exists | **packageable** |
| 1281, 1283 | **3a** — restrain Cause 3's own deliverable | **ruled below** |

---

## Cause 1 — `task_class` is computed at every routing decision and never persisted

`RouterAnswer::task_class()` (`routing/request.rs:606`) is a real producer:
every routed request already carries a `TaskClass`. The caller that has it in
scope is `record_routing_latency` (`main.rs:4443`), which builds its
`NewObservation` from `harness` and `purpose` only — **`answer.task_class()`
sits unused in the same function.** `NewObservation` has eleven `with_*`
builders and **no `with_task_class`**; `routing_observations`
(`database.rs:1266-1301`) has no such column.

**Producer exists, caller has it in scope, propagation is the missing link.**
And crucially this is *not* a token-parsing problem: every completed request
produces a row whether or not tokens are exposed, so a moving average of
**request counts** per task class (1276) needs nothing from the ingress wall.

## Cause 2 — the ingress wall, for one line only

1275 wants token consumption per task class, and tokens are what
`gateway::ingress` is designed never to parse. Stays with register row **P1b**;
no successor until the `ingress` ruling.

## Cause 3 — no burn-rate symbol anywhere, but every input already exists

`cluster-b.py` finds no `moving_average` / `burn_rate` / `time_to_exhaustion`
symbol — **these do not exist rather than being built-and-unwired**, the
stronger finding. But remaining capacity, reset timing and per-request rows are
all present, so this is a mechanism to build on existing inputs, not a blocked
signal. Covers 1277, 1278, 1279, 1280, 1282.

## Cause 4 — 1274 already ships when measurable

`record_extraction_observation` (`main.rs:8642`) records real `with_tokens()`
usage where it is measurable, and `record_routing_latency` (`main.rs:4443`)
records the turn itself with **no fabricated value** where it is not. The
line's own hedge is *"when measurable"*, and honest silence is compliance.
**Proof-only package**, the same shape as Phase 33B's 1353/1359.

---

## Cause 3a — 1281 and 1283. RULED 2026-09-02: they are acceptance criteria of Cause 3's package.

The census laid out two defensible answers and declined to choose. **The
ruling is Answer A, with a condition that makes it safe.**

Both lines are Cluster-P-shaped, and today the restrained thing genuinely does
not exist: no rolling statistic to be made non-robust, and nothing surfaces a
forecast at all. By the letter of Phase 33B's Cause D they would be filed
REFUSED.

**They are different in kind from 33B's 1356/1360, and the difference decides
it.** Those restrain mechanisms nobody has a reason to build — one is blocked
by its own phase's wall, the other forbids the very thing that would close it.
**1281 and 1283 restrain the exact mechanism their own sibling lines
(1277-1280) are asking to be built, in the package this census just scoped.**

The 1152 ruling says restraint lines are mutation-proven by violating the
restraint, and that it *"only applies when the restrained thing exists."*
**That condition is about the moment of the tick, not the moment of the
census.** When Cause 3's package lands, the rolling statistic exists and can be
swapped for a naive mean; the forecast exists and can be reworded as a promise.
Both mutations are then real defects that real tests catch. Judging them
against "this instant, before the package" evaluates the wrong moment.

Answer B also carries a cost this project has been paying all week: it files a
refusal that is **stale the day the package lands**, and needs a follow-up
recon to undo. Six stale blockers were found in this repository in two days;
knowingly writing a seventh is not bookkeeping hygiene.

**The condition, and it is not optional.** 1281 and 1283 may tick **only in the
same commit as 1277-1280**, and **only** if each carries its own KILLED
mutation:

- **1281** — swap the robust rolling statistic for a naive mean; a single
  outlier request must then move the estimate and a test must catch it.
- **1283** — swap the estimate wording for promise-sounding text; a test must
  assert the surfaced text hedges.

If Cause 3's package lands without those two mutations killed, **both lines
stay open.** They are not free riders on a package that happens to build their
subject; they are two more mutation-proven lines inside it.

**Generalised, because this will recur:** *a restraint line whose restrained
mechanism is built by the same package is an acceptance criterion of that
package, provided it carries its own KILLED mutation and ticks in the same
commit. A restraint over a mechanism no package is building stays Cluster P.*

---

## Recommended package boundary

Cause 1 + Cause 3 + Cause 4 are **one implementer package** — migration 23,
`routing/evidence.rs`, `main.rs`, `routing/pressure.rs`, `shell/mod.rs` —
closing **1276-1280 and 1282**, proving **1274**, and carrying 1281/1283 as
acceptance criteria under the ruling above. That is **six to nine boxes, all
facets of one mechanism**, squarely in §87's 3-6 target range at the mechanism
level.

**It is Red tier: it contains a schema migration.** Follow migration 18's
`failure_class` `ALTER TABLE ... ADD COLUMN` pattern exactly
(`database.rs:1975`, `:2891`, `:3091`, `:3106`) with the matching rollback and
`columns_of` assertion, and expect the ripple into literal `version, N` pins in
test files that a migration always causes.
