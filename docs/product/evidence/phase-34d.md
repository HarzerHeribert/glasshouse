
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

---

# Lines 1455 and 1456 — RE-OPENED 2026-08-30 by adversarial audit

Package `GH-EVIDENCE-AUDIT`, an independent verifier with no stake in the
outcome, dispatched specifically to attack the four boxes `GH-ROUTER-INPUT-PROOF`
closed the day before. **1455 and 1456 are REFUTED and un-ticked. 1452 and 1453
stand, with a limit recorded below.**

## The entry above named the hazard and then closed the boxes anyway

The original entry wrote: *"A negative requirement is the easiest thing in this
project to close falsely — 'we do not do X' passes trivially when nothing does
anything."* That reasoning was correct. The ruling did not follow it.

## Why they cannot be closed today: there is no router request

**Nothing in this build constructs a request to a routing model.** Two
statements in production source, neither of them a test:

- `routing/classify.rs:23-27` — *"whatever calls a routing model is expected to
  turn its reply into a `TaskClassification` … **Neither path is wired to a
  caller yet**."*
- `routing/classify.rs:583-586` — *"**No cheap model is wired up in this
  build**, so this always calls `classify(request_text, None)`."*

`classify()`'s only production caller passes `None`. `RoutingModelChoice` and
`RoutingModelResolution` are read in exactly one place, `shell/mod.rs:1492-1560`,
and only to render a label. A restraint on a request that is never built is
unobservable.

## The entry's own proof condition could not be performed

The entry set the bar itself: *"a mutation that widens the router input must
break it."* **No such mutation is runnable.** `TaskRequirements` is two fields
over a data-free enum, so every widening is a type change failing `E0063` at
both the producer (`main.rs:1278`) and the test file. `mutate.sh` reports a
compile failure as KILLED; §80 case 4 requires discarding it. What the entry
described as its proof is not an experiment that can be performed.

## What the guarding tests actually watch

`bounded_router_input` asserts `format!("{requirements:?}").len() < 300`. For
**every possible input** that string is 67–118 bytes — 182 bytes of headroom.
The test cannot fail while the type stands, and if the type changes the test
file stops compiling. This is §41: the test and the demanded mutation share the
assumption that `Debug` length is a proxy for "content reached the router", and
it is not. The 1456 fixture is a `--task` string *shaped like* a transcript; a
real transcript never comes near this path.

## The restraint is real, and it guarantees the wrong thing

`TaskRequirements` structurally cannot hold a blob, which is stronger than a
runtime check. But it bounds the input to `SessionRouter`, an **in-process
function that sends nothing anywhere**. It says nothing about what a future
routing-model request will carry — which is what 1455 and 1456 forbid.

**The map agrees with the auditor rather than with the original entry.** Phase
34B's `1425` — *"Do not send secrets, unrelated project memory, or full
conversation histories to the routing model"* — is 1455/1456's own claim about
the routing model, and it is open. And 34D's defining line `1447` is open:
`TaskRequirements` contains no user request, no session metadata and no resource
summaries, so it is not the object 1447 describes. **Four sub-clauses of a
schema were closed while the line defining that schema stayed open**, against
the map's stated order.

## What would close them honestly

A producer that assembles a routing-model request (1447), a caller that sends
it, and a test that fails when repository contents or a transcript are put into
that request. **Until the request exists, the restraint is unobservable — do not
re-package these two ahead of 1447.**

## 1452 and 1453 stand, WEAK, with a measured limit

Not vacuous. `hard_capabilities` carries `RepositoryAccess` and
`BrowserInteraction`, derived from a real classifier, reaching
`SessionRouter::choose` and consumed by `capability_fit`. All four
always-same-value mutations were **killed, each by both halves of its
assertion**, so both states are exercised — better evidence than the original
entry claimed.

**The recorded limit, measured not argued:** of three production `RouterInputs`
construction sites, **two pass `TaskRequirements::default()`** —

| site | function | `requirements` |
|---|---|---|
| `main.rs:1171` | `route_recommendation` (diagnostic) | `task_requirements_from_text(task)` |
| `main.rs:1427` | **`launch_session` — the path that starts a session** | `TaskRequirements::default()` |
| `main.rs:3816` | `report_task_boundary_routing` | `TaskRequirements::default()` |

So the signal is included in one router input out of three and **excluded from
the one that acts**. Line 1452 carries no scoping clause either way, unlike map
line 372 — which is why this is a limit rather than a refutation. **A package
that gives `launch_session` a real classification would strengthen both boxes
and is worth doing.**

## Two mutations survived, and both are findings

- `ClassificationSource::Heuristic` → `Model { label: request_text.to_owned() }`
  — the classification then carries **the entire request text**, and 28 tests
  pass. Nothing watches that field for content.
- `task_requirements_from_text`'s `classify_heuristically(text).hard_capabilities()`
  → `Vec::new()` — gutting the sole production producer survives (3 passed, 25
  filtered). The producer is not watched by the tests that were run.

## The transferable rule

**An evidence entry that names a hazard and then rules against it should be
re-read, not trusted.** The original worker's reasoning was right and its ruling
was wrong, and no reviewer caught it because the reasoning read as diligence.
An independent verifier with no stake in the outcome caught it in one pass.
