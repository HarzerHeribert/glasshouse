# Glasshouse orchestration measurements

Glasshouse's own product principle says telemetry must measure **outcomes and
evidence, not token/spend vanity metrics**. This file applies that principle to
the process that builds Glasshouse, because the same question the product asks
about routing work to models is the one this project answers every hour by
hand: *which tier, at what cost, produced what verified result?*

**This is a standing experiment, and it is inherited.** Every orchestrator adds
its batches to the ledger below and writes down what changed its mind. Do not
start a fresh measurement culture; continue this one. An entry with no verdict
is worse than no entry.

## What counts as an outcome here

Not lines of code, not tokens spent. A batch's outcome is:

- **boxes** — authoritative capability-map checkboxes closed with `COMPLETE`
  evidence. The only unit that means anything.
- **kills** — mutations run and killed. A box with vacuous tests is not a box.
- **corrections** — times the worker was right against its packet. This is a
  quality signal about the *tier*, and it has been consistently high.
- **rework** — gates that failed under the orchestrator's own re-run, or
  findings that had to be sent back.

Cost per box is the headline ratio. Wall-clock per box matters only where it
blocks the next batch.

## Method

- Worker cost, context and wall-clock are read off the harness's own status
  line in its cmux pane at the moment it reports.
- Orchestrator context is read from `CTX_PCT` in the statusline data file.
- Boxes are counted from the map before and after integration.
- Mutation verdicts come from the named test's own result line, in the target
  that runs it.
- **Leaf output is verified mechanically**, never eyeballed — diff its quotes
  against the source.

## The ledger

| Batch | Tier | Wall-clock | Worker cost | Output | Boxes | Kills | Corrections | Verdict |
|---|---|---|---|---|---|---|---|---|
| 9F direct provider | Opus specialist | ~39 min | ~$10.90 | +1757/-78, 6 files | 11 | 16/16 | 3 | PASS, CI green first push |
| 9D templates+headers | Sonnet | in flight | — | — | — | — | — | — |
| 9G gateway skeleton | Opus **team lead** (may subcontract) | in flight | — | — | — | — | — | — |
| 2C onboarding | Sonnet | in flight | — | — | — | — | — | — |
| Records audit | Gemini 3.7 Flash via `agy` | in flight | — | — | 0 (read-only) | — | — | — |

### What the first data point already says

**N = 1 for Opus, so treat these as orders of magnitude, not constants.**

- **~$1 of worker spend per capability box**, at Opus tier, including its own
  mutation testing. That is cheap against the alternative of the orchestrator
  writing it.
- **~17 boxes per worker-hour** on a well-specified packet.
- **Orchestrator review and integration cost ~10% of context and ~12 minutes**
  against the worker's 39. The review is *not* the bottleneck; worker
  wall-clock is.
- **Sonnet runs at roughly a third of Opus's cost rate** for comparable
  packets. The 9F batch was routed to Opus because it crossed a secret
  boundary, not because Sonnet could not have written the code.

### The parallelism ceiling, derived rather than guessed

Reviews are serial — the practice file's "never review two workers at once"
rule stands, and it is about attention, not throughput. Workers run
concurrently; reviews queue. So:

- **worker wall-clock ≫ review time** means concurrency pays until reviews
  start colliding — about **three editing workers**;
- the real ceiling is **orchestrator context**, at roughly 10% per batch, which
  puts a session's budget at **six to eight integrated batches** before handoff;
- a **team lead that reviews its own subcontractors** raises the first ceiling
  without touching the second, because its review cost is paid out of *its*
  context, not the orchestrator's. That is the whole reason to use one.

### The mistake this measurement exposed

For most of 2026-08-25 the orchestrator ran **one worker at a time**, believing
the work could not be partitioned. That was wrong, and the map itself proves
it: **1,266 unchecked lines across 99 phases**, with whole blocks in modules
nothing else touches. The conflicts were real only *within* the Phase 9 family,
because work was being taken in strict map order inside one family.

**Map order is a priority, not a mutex.** Partition batches by the *files they
touch*, then order those batches by the map. A packet's `FORBIDDEN FILES`
section is the scheduling primitive: it is what makes two workers safe to run
at once, and it should name the other live workers' files explicitly.

## Questions the next orchestrator should answer

Add your data; do not re-derive from scratch.

1. **Does the Sonnet tier close boxes at Opus's rate on amber work?** If cost
   per box is comparable, red-risk routing is the only reason to spend Opus.
2. **Does a team lead with subcontractors beat a lone Opus worker** on the same
   packet size — in wall-clock, in cost, and in whether its mutations still get
   done properly? Delegated test-writing is the obvious win; delegated
   *judgement* is the obvious risk.
3. **How accurate is the leaf tier on a second task?** It scored 171/171 on a
   map inventory. One score is not a capability.
4. **What is the real failure rate of concurrent worktrees?** Count merge
   conflicts and reverts, not intuitions about them.
5. **Where does an orchestrator's context actually go?** If reading diffs
   dominates, a verifier tier between worker and orchestrator pays for itself.
