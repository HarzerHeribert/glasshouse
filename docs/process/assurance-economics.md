# Assurance economics — spending verification where it buys something

> This describes how Glasshouse is built, not what Glasshouse does. Nothing
> here is a product requirement. Capability requirements live only in
> `docs/product/capability-map.md`.

This document exists because two batches on 2026-08-28 made the cost structure
of this process visible for the first time, and it was not where anyone assumed.

**The objective is not fewer checks. It is the same or stronger correctness
evidence with less repeated reasoning and repeated execution.**

Any optimization that skips verification must be backed by an explicit
dependency/invalidation argument. **If that argument is not reliable, keep the
broader verification.**

## The two inefficiency classes, and only one of them is waste

**1. Wrong work dispatched.** A packet whose premise is structurally impossible
burns full worker compute and cannot close a box no matter how well the worker
executes. Measured on batch 36: **two of three packets**, roughly **$30 of worker
compute**, produced correct mechanisms wired to callers that could never use
them. No downstream assurance optimization recovers any of it.

**2. Right work over-verified.** Repeated full-suite runs, double-authored
evidence, unbounded orchestrator re-review. This is *mostly deliberate* and must
be optimized conservatively — it is the reason this project's defect record is
what it is.

**Class 1 is pure waste and is fixed first.** Class 2 is overhead with a purpose
and is trimmed carefully.

## Priority order

1. dispatch feasibility / precondition validation
2. remove per-worker full-suite duplication
3. structured evidence instead of duplicated narrative
4. instrumentation of evidence coverage / selectivity
5. semantic mutation vocabulary
6. evidence-based incremental invalidation
7. adaptive assurance depth

**The first three produce meaningful savings without requiring the evidence
ledger to become a trusted correctness mechanism.** Items 6 and 7 do require
that, which is why they come after the measurement in item 4.

---

## Phase −1 — dispatch feasibility preflight

**A packet must prove its mechanism can connect end to end in the *current*
production architecture before a worker is dispatched.**

For every claimed input, name all five links:

```
claimed input
  → producing type/symbol exists
  → caller has the required field/state
  → propagation path exists
  → target consumer can observe it
  → evidence, as a file:line or symbol in current code
```

Worked example of the shape required:

```
Input producer:
  src/foo.rs::HarnessContext
Propagation:
  HarnessContext.harness
    → RouteRequest.harness
    → score_pairing_prior(...)
Consumer:
  src/routing/score.rs::score_pairing_prior
```

**If one link cannot be demonstrated from current production code, do not
dispatch implementation work.** Mark the packet `architecture-blocked` or
`premise-invalid` and return it for reformulation. That is a finding, and it
costs one orchestrator turn instead of a whole worker.

**The two failures this rule is built from**, both from batch 36 and both
catchable with two greps:

- *"the pairing prior is ready to wire into `DisposableRouting`"* —
  `PairingQuery::harness` is a **required** `IntegrationId`; `DisposableCandidate`
  has no harness field; a disposable job is Glasshouse's own internal call and
  never runs inside a coding harness. Link 2 fails.
- *"`gateway/session.rs` sees enough of a turn to record an observation"* —
  `gateway::ingress` is **structurally incapable of carrying a body**, by its own
  module doc. Link 1 fails for every timing and token field.

Enforced by `scripts/validate_round.py`, which already runs before every
dispatch, so the check costs nothing new.

### Measured on first use, 2026-08-28

**The gate refused two packets in six commands**, before either reached a worker:

- *wire the ledger into `DisposableRouting`* — `EvidenceLedger::summarize`
  matches `harness IS ?4`; every gateway-written row carries a harness; a
  disposable candidate has none. Legal query, zero rows, forever.
- *the API door observes lifecycle events* — the only `EventBus::new()` in
  production is `main.rs:1620` / `shell/mod.rs:86`; `api/**` has no bus.

**The first of those was the "cheapest next win" the previous checkpoint had
recommended**, written by an orchestrator that had *just* spent a round learning
this exact lesson. It was still wrong. **A recommendation in a handoff is not a
feasibility argument**, and that is the whole reason this is a mechanical gate
rather than a habit.

Cost per box across the three rounds either side of it:

| batch | workers | boxes | worker cost | per box |
|---|---|---|---|---|
| 35 | 3 | 35 | ~$36.5 | $1.04 |
| 36 | 3 | 27 | ~$36.2 | $1.34 |
| 37 (first gated round) | 1 | 13 | ~$13.0 | **$1.00** |

Batch 37 is cheapest per box **and** ran one worker rather than three. The saving
is neither parallelism nor tier: it is that **no compute went into a package that
could not close its boxes.**

### A fifth link worth adding, learned the expensive way

Twice in three rounds a **type signature** decided a capability's fate:
`PairingQuery::harness` being required, and then `classify` never reading
`route` — which made the pairing prior *structurally inert* at the caller it was
finally wired to, because every same-model candidate scores identically.

The four links catch "this input cannot be produced here". They do not catch
**"this input is constant across the candidates being compared."** Consider
asking, for any input feeding a *ranking*: what does its value actually vary
with, and is that thing different between the alternatives? A prior that exists
and never differs is a caller-shaped gap wearing a working mechanism's clothes.

### Promoted to a required link, 2026-08-28 — it decided a round

The fifth link stopped being optional the first time it chose between two
packages. Batch 39 dispatched map line 1547 (failure-domain diversity in the
failover ranking) **because** the signal varies across the candidates being
compared — `Upstream::failover_candidates` returns backends that share a
provider alongside backends that do not — and declined map line 1293 for the
ordinary link-4 reason.

Ask it as a required question, not a suggested one: **for any input feeding a
ranking, what does its value vary with, and is that thing different between the
alternatives?** If the answer is "nothing that differs here", the mechanism will
be built, wired, mutation-proven, and inert — which is what three rounds and
roughly $39 of worker compute bought before anyone asked.

### Three refusals, and the pattern in who wrote them

| round | refused | why |
|---|---|---|
| 37 | ledger reader into `DisposableRouting` | a disposable candidate carries no harness; the query matches zero rows forever |
| 38 | (preflight, not a refusal) | `profile/**` must not import `crate::config` — the constraint reached the packet instead of the worker |
| 39 | map line 1293, reserve in routing explanations | `disposable_candidates` builds only `Cost::Free`, so the reserve loop is always empty |

**Every one of those was the previous checkpoint's own recommended next step,
written by an orchestrator that had just finished learning this lesson.** That
is the finding. The gate is not compensating for a careless predecessor; it is
compensating for the fact that a next step written at the *end* of a round is
written when the code is least fresh and the reasoning is most compressed. Two
greps at dispatch time beat any amount of care at writing time, which is why
this is mechanical and why `CLAUDE.md` says do not route around it.

**Metric this phase owns: compute cost per *closable* packet dispatched.** A
worker executing an impossible premise dominates efficiency statistics even when
the worker and the assurance system both behave correctly.

---

## Phase 0 — instrumentation, and no assurance reduction at all

**Nothing is skipped in this phase.** Its output is the data that later phases
need in order to skip anything safely.

### Machine-readable evidence

The ledger already contains the dependency graph — as prose. Every entry cites
`Production evidence: <file>: <symbol>` and `Regression evidence: <test name>`.
Making that machine-readable is the enabling change for phases 2, 3 and the
convergence rule.

```yaml
capability: [1329, 1330, 1337]
proves:
  - routing_evidence::a_row_is_appended_per_exchange
covers:
  - path: src/routing/evidence.rs
    symbols: [append_exchange_evidence, RoutingEvidenceRow]
    relation: production
  - path: src/gateway/mod.rs
    symbols: [handle_exchange]
    relation: caller
mutations_killed: [record-call-deleted]
verified_at: 78e5307
file_hashes:
  src/routing/evidence.rs: <sha>
```

**Start with file hashes** — simple and trustworthy — but keep the schema able to
become symbol-level later.

### The evidence-index tool

Builds the capability → tests → production map, validates every reference
resolves, identifies changed files covered by **no** entry, and measures how
broadly a change invalidates the ledger.

### Split production compute from assurance compute

Worker reports must distinguish the two. Metrics worth tracking:

- output tokens per accepted worker result
- cache-create tokens per accepted result
- API-equivalent cost per accepted result
- **assurance cost per accepted result**
- reevaluation cycles per accepted result
- mutations attempted / killed
- percentage of evidence invalidated per change
- verification avoided through still-valid evidence

**Do not optimize raw token volume.** Cache reads dominate this workload —
measured 2026-08-28: 4.94B of 4.99B tokens in one weekly window — and are not
equivalent to fresh reasoning or output. Output and cache-create are the
signals.

---

## Phase 1 — remove redundant full-suite execution

Measured: the full workspace suite ran **6–8 times per batch** for three workers
touching disjoint files. `mem-validity` touched `memory/**` and ran `pty_smoke`.

```
worker iteration      targeted tests
worker completion     targeted regression + semantic mutations
batch integration     full workspace gate, ONCE
platform verification macOS / Ubuntu / Windows at the authoritative milestone
```

**The full authoritative gate stays.** The optimization is that three isolated
workers should not each run the same 1,750-test workspace twice before the
integrated tree is tested again anyway.

**Worker-local mutation testing stays too.** Those mutations test whether the
worker's own new regression tests detect plausible defects — that is not
duplicated by anything the orchestrator does later.

## Phase 1b — stop double-authoring evidence

Workers currently write polished 200-line narratives that the orchestrator then
rereads and rewrites. Separate the two roles:

**Worker produces structured facts:** changed files, tests executed and their
outcomes, mutations attempted with killed/survived, observed runtime behaviour,
decisive claims, unresolved questions.

**Orchestrator owns the ruling:** accepted / rejected / partial, capability
disposition, architectural interpretation, remaining gaps, required follow-up.

Decisive claims get flagged so review can be bounded rather than unconstrained:

```yaml
decisive_claims:
  - claim: "Gateway body is intentionally unavailable here"
    evidence: src/gateway/ingress.rs:123-177
    confidence: high
```

**Do not eliminate orchestrator source verification.** It caught three real
defects on 2026-08-28 — a config value that silently reported "nothing
configured", an untested over-fetch the worker itself called load-bearing, and
seven boxes proposed for ticking whose function had no production caller. **Bound
it to decisive, architectural, surprising, and correctness-critical claims**
rather than removing it.

---

## Phase 2 prerequisite — prove the map is selective before trusting it

**Do not build incremental verification until the data says it buys something.**

Classify every recent change:

- `COVERED` — all changed files map cleanly to evidence
- `BROADLY_COVERED` — mapped, but invalidates a large share of the ledger
- `UNCOVERED` — at least one changed file has no evidence mapping

Track `evidence_coverage_rate`, `mean_entries_invalidated_per_change`,
`p95_entries_invalidated_per_change`, `full_gate_fallback_rate`, and:

```
Invalidation Selectivity = 1 − (invalidated entries / total entries)
```

| selectivity | reading |
|---|---|
| ~90% | excellent candidate for evidence reuse |
| ~60–70% | probably worthwhile |
| ~20% | too broad; do not build the machinery |

**If most production changes invalidate most entries anyway, stop here.** A
sophisticated mechanism that buys little is a new surface that can silently lie.

## Phase 2 — evidence invalidation

```
changed code
  → dependency analysis
  → invalidate only affected evidence
  → re-run affected verification
  → preserve unaffected evidence
```

**Conservative fallback: a changed production file absent from the evidence map
broadens verification**, initially to a full gate. **Track every fallback
explicitly** so poor map coverage cannot hide indefinitely behind conservative
full-suite runs.

## Phase 3 — adaptive assurance depth

Reuse the Green / Amber / Red taxonomy already in
`docs/process/worker-capabilities.md`. Do not invent a second scale.

| | assurance |
|---|---|
| **Green** | targeted tests, static checks, standard worker evidence |
| **Amber** | broader regression, independent orchestrator verification, targeted semantic mutations |
| **Red** | full relevant regression, strong independent review, semantic mutation suite, capability-wide verification, platform verification where applicable |

**Uncertain impact escalates.** Do not reduce assurance because a task looks
small.

---

## Semantic mutation vocabulary

Mutation choice should follow the semantics actually changed, not be picked ad
hoc:

`invert-condition` · `remove-guard` · `remove-validation` · `alter-boundary` ·
`bypass-fallback` · `weaken-error-propagation` · `accept-stale-state` ·
`skip-state-update` · `remove-persistence-call`

**A surviving mutation is the most valuable outcome**, because it identifies a
case where apparently passing tests do not prove the claimed behaviour. Two of
the best findings on 2026-08-28 came from mutations that survived — one caught by
a worker on its own draft, one by the orchestrator on its own integration edit.
A vocabulary makes that systematic rather than lucky.

## Reevaluation convergence

Distinguish three things the current loop conflates:

1. new evidence that genuinely reopens a gate
2. a localized fix invalidating only part of prior evidence
3. already-established facts that remain valid

**A small correction must not trigger complete rediscovery of unaffected
capabilities.**

The loop converges when **all** hold:

- every piece of evidence required for the affected scope is valid
- required mutations are killed or explicitly dispositioned
- the authoritative gates for that risk class pass
- no unresolved high-confidence defect remains
