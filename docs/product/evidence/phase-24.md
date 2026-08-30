# Phase 24 — Memory reranking

**Phase state: 1 of 6.** Five lines are refused; see the refusal register.
`GH-MEMORY-PHASES-RECON` classified all six on 2026-08-30: **1089, 1091 and
1092** need a reranking stage that does not exist (`routing/classify.rs:583-586`
— *"No cheap model is wired up in this build"*); **1090** is Cluster Q, a
restraint over a capability that does not exist; **1094** needs a debug-mode
concept that returns zero hits tree-wide, and the diagnostics it would record
are computed and then discarded at `memory/search.rs:443`.

---

# Line 1093 — closed 2026-08-30, already true, and the existing test was not enough

Package `GH-INJECTION-CAP`. **No production change**: `memory/inject.rs` is
untouched, `MAX_INJECTED_MEMORIES` is still `3`, and
`.take(MAX_INJECTED_MEMORIES)` is where it was.

## Why the line was not already evidenced

`an_absurd_caller_supplied_task_still_yields_a_bounded_injection` asserted
`entries > 0 && entries <= MAX_INJECTED_MEMORIES` — an ***at most*** bound, on a
fixture of nine candidates with 4000-character bodies. **Two ceilings are live
in that fixture**: the count cap and `MAX_INJECTED_BYTES` (900 bytes). An
"at most" assertion where either could have truncated cannot say which one did.
That is §41 exactly — a weak test and a weak mutation sharing one blind
assumption — and it is why this line stayed open beside a passing test.

## The fixture is the work

`the_cap_truncates_more_than_the_cap_eligible_candidates_to_exactly_the_cap`
uses **five short, current, non-idea `Constraint` records** so the byte budget
has room for all of them and only the count cap can act. It asserts **exactly**
three, not at most three.

**The pre-cap count is measured, not assumed**: the test first calls
`store.search_grouped_for_injection("kestrel", SearchScope::Current, 40)` — the
same call `briefing` makes before its own `.take` — and asserts it returns **5**
before the server is even started. Two more eligible candidates than the cap, so
the cap demonstrably fires.

`fewer_eligible_memories_than_the_cap_are_all_injected` records two candidates
and asserts both are injected, which stops the first test passing vacuously
against a `briefing` that returned three regardless of input.

## Mutations

| mutation | result | killed by |
|---|---|---|
| delete `.take(MAX_INJECTED_MEMORIES)` | **KILLED** | `the_cap_truncates_…` — the injected block came back carrying **5** memories |
| `MAX_INJECTED_MEMORIES: 3` → `30` | **KILLED** | the same test, and only that test |
| subject-budget change (predicted SURVIVED) | **SURVIVED, as predicted** | — |

**Re-run independently by the orchestrator** after integration: the delete-cap
mutation killed again, the failure naming the five-entry block verbatim.

## The worker caught a false KILLED in its own draft — §80

Its first draft set the fixture size from the constant under test
(`MAX_INJECTED_MEMORIES + 2`). The raise-the-cap mutation returned KILLED, **but
for the wrong reason**: raising the cap to 30 also rescaled the fixture to 32
candidates, which no longer fit the 900-byte budget, so the byte ceiling was
doing the killing. It rewrote both fixture sizes as literals (`5` and `2`) so
the fixture cannot move when the mutated constant does, and re-ran.

**That is the trap this project keeps rediscovering, caught by a worker before
review.** A fixture derived from the constant under mutation cannot isolate it.

## Limits, stated rather than discovered later

- Does not prove *which* three of more-than-three tied candidates survive —
  that is line 1131's test.
- Does not exercise `MAX_INJECTED_BYTES` or the subject/body truncation budgets.
  **The subject-budget mutation SURVIVED and is disclosed as a real, separate
  gap** in what this file watches.
- `context_injection.rs` is `#![cfg(unix)]`, consistent with the rest of the
  file and with `phase-27.md`; Windows is not exercised.
