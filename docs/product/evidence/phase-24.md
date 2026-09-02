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

## Lines 1089, 1090, 1091, 1092 and 1094 — CLOSED 2026-09-02 (`GH-MEMORY-RERANKER`, Amber, Sonnet high): Phase 24 complete

The phase was refused whole while *no cheap model was wired up in this
build*; since batch 87 the disposable router calls what it chooses, and
`design-decisions.md` (*A reranking seat in the disposable router*) put the
reranker on that seat. This package is the ruling landed — from the packet's
own objective, because the worker's base predated the note's commit (its
first packet error, and it was right).

**Contract.** Given a briefing whose lexical search returns ordinary
candidates and a configured `[memory] rerank_model`, when Glasshouse selects
memory for a task, Glasshouse sends at most `RERANK_CANDIDATES` (8) of the
*other* group to that model once and reorders them by the ids it returns —
invariants and constraints first by authority, currency and the injection
cap applied after the reorder, omitted ids appended in lexical order (a
reranker may demote, never hide) — while preserving that with no
`rerank_model` no model is called and the selection is byte-identical, that
every failure (no resource, no credential, a refusal, a timeout, an
unparseable reply, an unknown id) is a bypass with a stated reason and never
an error the caller sees, and that `[memory] retrieval_diagnostics` writes
one JSON line per briefing only when on, which `memory search --explain`
prints for one query without writing.

**Production evidence.**
- `memory/rerank.rs` (new): `rerank`, `RerankOutcome`, `parse_reply` (strict:
  an id outside the sent set is a bypass), `RERANK_CANDIDATES`,
  `EXCERPT_BYTES`, `RERANK_PROMPT_CONTRACT` (task relevance, recency, active
  status, non-duplication — 1092), `resolve_rerank_model` (the seat: consent,
  `JobKind::Reranking`, the routed client, a dedicated 6 s timeout),
  `RetrievalTrace`, `append_diagnostics`, `explain_line`.
- `memory/inject.rs::briefing_traced` / `select_briefing_traced` — the rerank
  runs on the whole *other* bucket before its partition into failed attempts
  and the rest, so authority precedence extends one level down;
  `is_current`, the already-injected filter and `MAX_INJECTED_MEMORIES` run
  after the reorder.
- Both doors: `api/unix.rs::select_memory` (the library seat, because a
  library door cannot call the binary crate — the worker's second packet
  error, structural and right) and `main.rs::brief_launch_session`;
  `main.rs::disposable_rerank_model` is the thin wrapper the note named,
  beside `disposable_extraction_model`. `memory/extract/mod.rs::Prompt::from_text`
  scrubs the whole assembled text. `config/mod.rs`: `rerank_model`,
  `retrieval_diagnostics` (project over user, default off). `cli.rs`:
  `memory search --explain`.

**Regression evidence** (`tests/memory_reranker.rs`, shipped binary through
the control-API door and `memory search --explain`, 8):
`a_reversed_reply_reorders_ordinary_memories_with_a_constraint_still_first`,
`more_than_the_window_sends_exactly_rerank_candidates_ids`,
`a_no_rerank_model_configured_leaves_the_block_lexical_and_dials_nothing`,
`an_unknown_id_bypasses_to_lexical_order_and_diagnostics_record_the_reason`,
`a_conflicted_memory_is_never_injected_even_when_ranked_first`,
`a_fixture_that_never_answers_bypasses_within_the_seats_timeout`,
`diagnostics_off_writes_nothing_and_on_writes_one_line_per_briefing`,
`memory_search_explain_prints_the_record_and_writes_no_file`; unit
`memory::rerank` (7, including
`omitted_ids_follow_in_lexical_order_and_the_window_bounds_what_is_sent` and
`one_candidate_calls_nothing`); `context_injection` 15/15 unchanged.

**Mutations** (worker, five, all KILLED, restored byte-identical):
`rerank-without-consent` (the seat falls back to the first free model) by
`a_no_rerank_model_configured_leaves_the_block_lexical_and_dials_nothing` —
the fixture was dialled; its first draft SURVIVED on a one-candidate fixture
that `TooFew` masked, re-seeded and killed (§80); `unknown-id-accepted` by
`an_unknown_id_bypasses_to_lexical_order_and_diagnostics_record_the_reason`
— outcome *reordered* where *bypassed* was owed; `currency-delegated` (the
post-rerank `is_current` filter deleted) by
`a_conflicted_memory_is_never_injected_even_when_ranked_first`;
`window-unbounded` by `more_than_the_window_sends_exactly_rerank_candidates_ids`
— *left: 12, right: 8*; `diagnostics-always` by
`diagnostics_off_writes_nothing_and_on_writes_one_line_per_briefing`.

**Orchestrator's read before the tick:** the ruling's every rule checked
against the report and the merged `rerank.rs` — consent, the *other* group
only, 8, one call and never below two, strict ids, omitted ids appended,
currency after, the diagnostics file and flag, both doors — all present. Two
decisions the note left open and the worker made, accepted: a 6 s dedicated
timeout for a call on every routed task's critical path (a Green follow-up
may name it in config), and a capped `subject` on each diagnostics candidate
row so a person can read the record without a second query.

**Recorded limits** (the worker's): no persisted cross-process health or
request-pacing reservation for this seat (a fresh `FreePool` per call);
the seat's one candidate carries no capacity, locality or entitlement
enrichment (inert defaults); `--explain` ignores `--history`/`--limit` by
design; `memory_retrieval_diagnostics_enabled` exists once in the library and
once in the binary (the crate boundary); `main.rs::estimated_project_memory_tokens`
passes no model (a `route` estimate should not spend a call); macOS only.
Not yet wired at the time of writing: the seat's chooser did not gather
budget spend — landed later the same day by
`GH-BUDGET-SPEND-REMAINING-CALLERS` (see `phase-32d.md`, the 1263 entry's
dated note): `resolve_rerank_model` now refuses a metered candidate on an
exhausted provider through `ModelError::Declined`, and a free one still runs.

State: **COMPLETE** for 1089, 1090, 1091, 1092 and 1094. **Phase 24 stands
at 6 of 6.**
