# Phase 27 — Context injection, 10 of 11 closed, 1 refused

Capability map lines 1125–1135. Package `GH-CONTEXT-INJECTION`, worktree
`.worktrees/context-injection`; report in
`.agent-runtime/report-context-injection.md`. Integrated 2026-08-29.

This phase was the memory trunk. It was at **0 of 11**, and two of Phase 21F's
lines (930, 934) were filed as Cluster D against it this same batch — they
qualify an injection that did not exist.

## A live product defect found by building this, and it is the most important
## thing in this entry

**A delivery longer than the terminal's canonical line limit wedges the
session permanently.** Measured against a real pty:

    1000 bytes -> arrives intact
    1023 bytes -> nothing arrives, ever again on that terminal

Not truncated — **discarded, and every subsequent byte written to that
terminal discarded with it.** `SessionApi::send_text` appends `` and writes
into a pty; any terminal left in canonical mode (every harness that has not put
its tty into raw mode, and every shell) has `MAX_CANON` = 1024 bytes on
macOS/BSD.

The worker's first `MAX_INJECTED_CHARS` was 4000. The affected sessions
received neither the memory **nor the task they were spawned for**, and the
failure looked exactly like *"the harness never started"*.

Two consequences, and the second is not this phase's:

- The injection ceiling is a **safety property, not a conciseness one**:
  `inject::MAX_INJECTED_BYTES = 900`, counted in **bytes** because the terminal
  counts bytes (900 chars of multi-byte text is 2700 bytes). Enforced by
  dropping whole entries so the closing marker always survives, with a
  `debug_assert!` in `render` that fires in every debug test run if a later
  change to the header breaks it.
- **`Request::SendMessage` has the same defect and always has.** A caller
  sending more than ~1022 bytes wedges that session's input permanently,
  silently, with an `ok` response. Not fixed here — it is not this phase's
  line and the fix must choose between truncating a caller's text and refusing
  it — and it **deserves its own packet**. Recorded in the handoff.

## 1130 is a trust boundary and it is enforced by construction

`Injection` has one constructor and no public field: there is no way to obtain
the text except from `briefing`, and `render` always opens with
`MEMORY_MARKER` and closes with `MEMORY_MARKER_END`.

The containment argument is one sentence and one grep: **every structural token
this module emits begins with `[`**, and `quote` rewrites `[`→`(` and `]`→`)`
in all untrusted text — so a memory body cannot forge a boundary, close the
block early, or open a second one, whatever it contains. `quote` also maps
every control character, the Unicode line and paragraph separators, and the
bidi overrides to spaces. A memory body is text extracted from earlier
sessions; treating it as untrusted is the whole point of the line.

## 1129 — refused, and the refusal is written where the wiring would go

There is no honest retrieval-confidence signal to threshold. `MemoryStore::search`
computes BM25 relevance, multiplies it by `policy::retrieval_weight`, and
**discards both** at `search.rs:318` (`.map(|(record, _)| record)`). Neither
`MemoryRecord` nor `RetrievalResult` carries a score field, and exposing one
means editing `memory/search.rs`, which the packet forbade and made a STOP
condition.

Every reachable alternative measures the wrong thing: `ladder_rung` and
`retrieval_weight` never see the query text, so a "confidence" built from them
is high for an ancient invariant regardless of what was asked; a result count
measures how much the project has written down; a second BM25 query from
`inject.rs` would be a second retrieval implementation ranking differently from
the one that chose the memories it was scoring.

What the code does instead is the honest subset — **a search that matches
nothing injects nothing** — and it does not claim that as 1129. The refusal is
recorded in `inject.rs`'s module doc and in `briefing`'s own doc comment,
where a threshold would go, not only here.

## 1126 closes with a limit that must not be lost

`sanitize_query` quotes each token and joins them with spaces, which FTS5 reads
as an implicit **AND**: every word must appear in the same memory. That is
right for a search box and wrong for a routed task — **a task written as a
sentence retrieves nothing**, so this step is inert for prose tasks and works
for keyword-shaped ones.

**The orchestrator's ruling, and why it differs from Phase 34's hold.** Phase 34
was held because `capability_fit` short-circuited and could *never* fire in
production — structurally zero reach. This is different in kind: the query runs,
is BM25-ranked against the routed task, and fires for a real subset of inputs,
proven end-to-end through the shipped binary. Narrow recall is a **quality**
limit, not absence. It closes, and the limit is named here and pinned by a test
rather than discovered later.

**The follow-on is small and specific:** give the injection path a query
semantics that works for prose (OR-with-ranking rather than implicit AND). It
needs `memory/search.rs`, which this packet forbade. Until then, injection in
production will mostly not fire.

## What did not need to change

`session/api.rs` was in EXPECTED FILES and needed **no change**:
`SessionApi::send_text` already is the seam, already `MessageOrigin::Machine`.
Ruling 1 said deliver through the existing seam and that turned out to be
literally free.

---

### Add a context-selection step before Glasshouse automatically sends a routed task to a session. (line 1125)

Contract: Given a task routed to a session, when Glasshouse is about to send it, Glasshouse first runs a context-selection step against this project's memory, while preserving the exact delivery a spawn with no task or no selected memory has today.

State: **COMPLETE**

Production evidence:
- `src/memory/inject.rs` — `briefing`
- `src/api/unix.rs` — `spawn_session`
- `src/api/unix.rs` — `select_memory`
- `src/api/unix.rs` — `deliver_memory`

Regression evidence:
- `context_injection::a_spawn_with_a_task_delivers_a_labelled_memory_block_and_the_task_distinguishably`
- `context_injection::a_spawn_into_a_project_with_no_memories_delivers_exactly_the_task_and_nothing_else`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/inject.rs: `text.push_str(MEMORY_MARKER);` -> `text.push_str("");` | `drop-the-label` | **killed** | `context_injection::a_spawn_with_a_task_delivers_a_labelled_memory_block_and_the_task_distinguishably` |

> drop-the-label observed: context_injection.rs:325 `exactly one delivery must be an injected memory block` — a real assertion on the label, not a fixture timeout (practice §80 case 5)

Recorded scope limits — stated by the worker, not discovered later:
- The step runs on both machine-delivered task paths (spawn's `task` and `Request::SendMessage`). It is not reached by a person typing into a session, which never travels this door.

---

### Query project memory for memories relevant to the routed task. (line 1126)

Contract: Given a routed task, when the context-selection step runs, Glasshouse queries this project's memory for memories relevant to that task through MemoryStore::search_grouped, while preserving that ranking rather than re-ranking.

State: **COMPLETE**

Production evidence:
- `src/memory/inject.rs` — `briefing`

Regression evidence:
- `context_injection::a_spawn_with_a_task_delivers_a_labelled_memory_block_and_the_task_distinguishably`
- `context_injection::a_task_written_as_a_sentence_retrieves_nothing_because_the_search_ands_its_terms`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/search.rs: `AND memories.project_id = ?2 \` -> `AND (?2 IS NOT NULL) \` (restored byte-identically by mutate.sh) | `drop-project-scope-on-the-injection-query` | **killed** | `context_injection::another_projects_memory_never_reaches_an_injected_block` |

> drop-project-scope-on-the-injection-query observed: context_injection.rs:629 `another project's memory must never reach an injected block` — and only that one test failed, so the predicate is watched specifically

Recorded scope limits — stated by the worker, not discovered later:
- READ THIS BEFORE TICKING. `sanitize_query` ANDs a query's terms, so a task written as a sentence retrieves NOTHING and this step is inert for prose tasks; it works for keyword-shaped ones. Proven, deliberately, by `a_task_written_as_a_sentence_retrieves_nothing_because_the_search_ands_its_terms`, which pins the limitation with a keyword-shaped control beside it and must be INVERTED rather than deleted when `memory/search.rs` grows a non-conjunctive mode. Holding this line open instead is a defensible ruling and it is yours.

---

### Inject only a bounded set of high-relevance memories into the target session. (line 1127)

Contract: Given any caller input, when memory is injected, Glasshouse injects at most 3 memories in at most 900 bytes, while preserving the property that no request field can raise either ceiling.

State: **COMPLETE**

Production evidence:
- `src/memory/inject.rs` — `MAX_INJECTED_MEMORIES`
- `src/memory/inject.rs` — `MAX_INJECTED_BYTES`
- `src/memory/inject.rs` — `MAX_QUERY_CHARS`
- `src/memory/inject.rs` — `render`

Regression evidence:
- `context_injection::an_absurd_caller_supplied_task_still_yields_a_bounded_injection`
- `memory::inject::tests::quoting_cuts_by_character_and_says_that_it_cut`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/inject.rs: `pub const MAX_INJECTED_BODY_CHARS: usize = 120;` -> `= 100000;` | `unbound-an-injected-body` | **killed** | `context_injection::an_absurd_caller_supplied_task_still_yields_a_bounded_injection` |

> unbound-an-injected-body observed: context_injection.rs:325 `exactly one delivery must be an injected memory block` — one untruncated 4000-char body exceeded the whole-block budget, so every entry was dropped and nothing was injected at all. The test was changed to wait for ONE delivery rather than two precisely so this fails on that assertion instead of on a fixture timeout (§80 case 5).

Recorded scope limits — stated by the worker, not discovered later:
- There is no injection knob in the request at all, so 'a caller cannot raise the bound' is structural rather than clamped. The only caller input on this path is the task text.
- A single memory rich in rationale and conditions can consume most of the byte budget and drop the entries behind it. Entries are dropped from the end of a list already ordered by 1131's preference, so what survives is what that line says matters most.

---

### Keep memory injection separate from native harness session history. (line 1128)

Contract: Given memory selected for a session, when Glasshouse delivers it, Glasshouse sends it as a message through SessionApi::send_text with MessageOrigin::Machine, while preserving the harness's own session files, transcript and resume state untouched.

State: **COMPLETE**

Production evidence:
- `src/api/unix.rs` — `deliver_memory`
- `src/session/api.rs` — `SessionApi::send_text`

Regression evidence:
- `context_injection::a_spawn_with_a_task_delivers_a_labelled_memory_block_and_the_task_distinguishably`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/api/unix.rs: `if let Err(err) = api.send_text(session, briefing.text()) {` -> `... api.send_text(session, "(memory omitted)") {` | `do-not-deliver-the-block-through-the-message-seam` | **killed** | `context_injection::a_spawn_with_a_task_delivers_a_labelled_memory_block_and_the_task_distinguishably` |

> do-not-deliver-the-block-through-the-message-seam observed: context_injection.rs:325, and ten other tests with it — nothing labelled reached the terminal. This is the positive half of the line: the block travels the message seam and nothing else carries it.

Recorded scope limits — stated by the worker, not discovered later:
- This line is satisfied partly by an ABSENCE — no harness session file, transcript or resume state is written — and an absence cannot be mutation-proven by deleting code. The mutation above proves the positive half; the negative half rests on `inject.rs` containing no file I/O at all, which is a source fact, checkable by grep, not a test.
- Proven by the block arriving on the harness's terminal and by `inject.rs` performing no file I/O at all. Not proven by inspecting any particular harness's session files, which differ per harness.

---

### Avoid injecting memory when retrieval confidence is low. (line 1129)

Contract: REFUSED. Glasshouse has no retrieval-confidence signal to threshold: MemoryStore::search computes BM25 relevance (search.rs:242, :272), multiplies it by policy::retrieval_weight (:310-311) and discards both at :318; neither MemoryRecord nor RetrievalResult carries a score field. Exposing one requires editing memory/search.rs, which this packet forbids and makes a STOP condition.

State: NOT STARTED — worker refused the line; see its reason

Recorded scope limits — stated by the worker, not discovered later:
- Every reachable alternative measures the wrong thing (§79's fifth-link test). `ladder_rung` and `retrieval_weight` never see the query text, so a confidence built from them is high for an ancient invariant regardless of what was asked. A result count measures how much the project has written down. A second BM25 query from inject.rs would be a second retrieval implementation ranking differently from the one that selected the memories it scored.
- What the code does instead is the honest subset — a search matching nothing injects nothing — and it is NOT claimed as this line. That is an empty result, not a threshold.
- The refusal is recorded in inject.rs's module doc and in briefing's doc comment, where the threshold would be added (§79).

---

### Clearly label injected information as Glasshouse project memory rather than user-authored instructions. (line 1130)

Contract: Given injected text, when it reaches a session, Glasshouse labels it as project memory and not as a user instruction by construction, while preserving that property against a memory body containing the marker, a terminator, a forged entry head, a carriage return or an escape sequence.

State: **COMPLETE**

Production evidence:
- `src/memory/inject.rs` — `Injection`
- `src/memory/inject.rs` — `render`
- `src/memory/inject.rs` — `quote`
- `src/memory/inject.rs` — `MEMORY_MARKER`

Regression evidence:
- `context_injection::a_hostile_memory_body_cannot_break_out_of_or_forge_an_injected_block`
- `context_injection::a_spawn_with_a_task_delivers_a_labelled_memory_block_and_the_task_distinguishably`
- `memory::inject::tests::quoted_text_can_never_contain_a_bracket`
- `memory::inject::tests::quoted_text_can_never_contain_a_control_character`
- `memory::inject::tests::quoted_text_drops_line_separators_and_bidi_overrides`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/inject.rs: `'[' => '(',` -> `'[' => '[',` | `keep-a-bodys-brackets` | **killed** | `context_injection::a_hostile_memory_body_cannot_break_out_of_or_forge_an_injected_block` |
| src/memory/inject.rs: `c if c.is_control() => ' ',` -> `c if c.is_control() && false => ' ',` | `keep-a-bodys-control-characters` | **killed** | `context_injection::a_hostile_memory_body_cannot_break_out_of_or_forge_an_injected_block` |

> keep-a-bodys-brackets observed: context_injection.rs:527 — the bracket count assertion; a body's `[` became structure

> keep-a-bodys-control-characters observed: a body's own `\r` split the injected block into extra deliveries, the second reading as a fresh user prompt — which is the attack this rule exists for

Recorded scope limits — stated by the worker, not discovered later:
- Containment rests on one invariant: quoted untrusted text can contain no `[` or `]`, and every structural token this module emits begins with `[`. That is checkable by grep rather than by a pattern list.

---

### Include active constraints and relevant failed approaches preferentially when they can prevent repeated mistakes. (line 1131)

Contract: Given a retrieval that matched more memories than fit, when Glasshouse selects, Glasshouse takes currently active invariants and constraints first and relevant failed approaches second, while preserving search's own relevance order within each group.

State: **COMPLETE**

Production evidence:
- `src/memory/inject.rs` — `briefing`

Regression evidence:
- `context_injection::active_constraints_and_failed_approaches_are_injected_in_preference_to_ordinary_matches`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/inject.rs: `.chain(failed)` -> `.chain(std::iter::empty())` | `drop-the-failed-approach-preference` | **killed** | `context_injection::active_constraints_and_failed_approaches_are_injected_in_preference_to_ordinary_matches` |

> drop-the-failed-approach-preference observed: context_injection.rs:872 `a relevant failed approach must be injected in preference to ordinary matches` — with six ordinary findings competing for three slots, the failed approach fell out entirely

Recorded scope limits — stated by the worker, not discovered later:
- A stable partition over `search_grouped`'s output, not a re-rank. The ladder, decay weighting and thin-decision demotion all ran inside `MemoryStore::search`, so an injection can never promote a memory past a rung its authority and currency did not earn.

---

### Do not inject stale ordinary decisions as binding instructions when their original assumptions have not been validated against current project state. (line 1132)

Contract: Given an ordinary decision whose assumptions have never been validated, when Glasshouse injects it, Glasshouse presents it as context rather than as a binding instruction, while preserving invariants and constraints as binding and reading only what the store already records.

State: **COMPLETE**

Production evidence:
- `src/memory/inject.rs` — `standing`
- `src/memory/inject.rs` — `may_constrain`

Regression evidence:
- `context_injection::an_unvalidated_ordinary_decision_is_injected_as_context_and_a_validated_one_as_binding`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/inject.rs: `Some(MemoryAuthority::Decision) if record.last_validated_at.is_none() => {` -> `... if record.last_validated_at.is_some() => {` | `present-an-unvalidated-decision-as-binding` | **killed** | `context_injection::an_unvalidated_ordinary_decision_is_injected_as_context_and_a_validated_one_as_binding` |

> present-an-unvalidated-decision-as-binding observed: context_injection.rs:944 — the entry head for the never-reaffirmed decision read `binding`

Recorded scope limits — stated by the worker, not discovered later:
- 'Not validated' is `last_validated_at.is_none()`. Nothing reads the user's repository (map line 932, Cluster F). The exploratory-phase half of staleness is already applied upstream by policy::retrieval_weight, which this selection inherits; re-applying it here would be inventing a rule.
- A decision that is thin by 21B's `is_lower_confidence_decision` but HAS been reaffirmed is still presented as binding. That reading follows the line's own words ('assumptions have not been validated'); the alternative is available and is a ruling, not a fact.

---

### Include authority, validity, and rationale metadata when a memory materially constrains the implementation. (line 1133)

Contract: Given a memory whose recorded authority may materially constrain the implementation, when it is injected, Glasshouse carries its authority, rationale, validity and invalidation conditions with it, while preserving the rule that a non-binding memory's conditions are not presented as constraints.

State: **COMPLETE**

Production evidence:
- `src/memory/inject.rs` — `render_entry`
- `src/memory/inject.rs` — `may_constrain`

Regression evidence:
- `context_injection::an_unvalidated_ordinary_decision_is_injected_as_context_and_a_validated_one_as_binding`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/inject.rs: `    if may_constrain(record) {` -> `    if !may_constrain(record) {` | `withhold-metadata-from-a-constraining-memory` | **killed** | `context_injection::an_unvalidated_ordinary_decision_is_injected_as_context_and_a_validated_one_as_binding` |

> withhold-metadata-from-a-constraining-memory observed: context_injection.rs:967 `rationale must travel with it` — the constraining memory arrived without the rationale and conditions that explain what it constrains

Recorded scope limits — stated by the worker, not discovered later:
- Gated on MemoryAuthority::is_binding, the same gate `api::unix::memory_result_json` already applies on this door, so the two surfaces cannot disagree about what constrains.

---

### Prefer a small number of current high-authority memories over a larger collection of historical decisions. (line 1134)

Contract: Given a project with a large accumulated history, when Glasshouse injects, Glasshouse sends a small number of current high-authority memories and never history, while preserving search's ladder order.

State: **COMPLETE**

Production evidence:
- `src/memory/inject.rs` — `briefing`
- `src/memory/inject.rs` — `MAX_INJECTED_MEMORIES`

Regression evidence:
- `context_injection::a_superseded_memory_is_never_injected_while_the_memory_that_replaced_it_is`
- `context_injection::a_memory_the_retrieval_put_into_conflict_is_never_injected_as_settled_knowledge`
- `context_injection::an_absurd_caller_supplied_task_still_yields_a_bounded_injection`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/inject.rs: `.filter(MemoryRecord::is_current)` -> `.filter(|record| !MemoryRecord::is_current(record) || true)` | `inject-a-non-current-memory` | **killed** | `context_injection::a_memory_the_retrieval_put_into_conflict_is_never_injected_as_settled_knowledge` |

> inject-a-non-current-memory observed: SURVIVED on first run against ten passing tests — §80's three questions all answered yes, so the verdict was honest and the filter was genuinely unwatched. `MemoryStore::search` moves a contradicting pair to Conflicted while answering the query that returned it, so a record can pass the SQL `status='active'` filter and stop being current inside the same call; no other test in the file seeds memories that can contradict. The new test produces that case and the re-run is KILLED at context_injection.rs:1102, `a memory the retrieval flagged as conflicted must not be injected`.

Recorded scope limits — stated by the worker, not discovered later:
- SearchScope::Historical is never used on this path, so history is not merely filtered out — it is never queried.

---

### Avoid repeatedly injecting the same unchanged memory into an already-aware hot session unless needed. (line 1135)

Contract: Given a live session that has already been sent a memory, when Glasshouse routes it another task, Glasshouse does not send that same unchanged memory again, while preserving delivery of memories the session has not yet seen.

State: **COMPLETE**

Production evidence:
- `src/api/unix.rs` — `Injected`
- `src/api/unix.rs` — `deliver_memory`
- `src/memory/inject.rs` — `briefing`

Regression evidence:
- `context_injection::the_same_unchanged_memory_is_not_injected_twice_into_one_hot_session`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| src/memory/inject.rs: `.filter(|record| !already_injected.contains(&record.id))` -> `.filter(|record| already_injected.contains(&record.id) || true)` | `remove-the-already-injected-check` | **killed** | `context_injection::the_same_unchanged_memory_is_not_injected_twice_into_one_hot_session` |

> remove-the-already-injected-check observed: context_injection.rs:765 — a hot session that already had the memory received a second block

Recorded scope limits — stated by the worker, not discovered later:
- In memory, not the database: no schema migration, and `database.rs`/`session/store.rs` were not touched. A hot session exists only as long as the SessionRuntime holding its pty, and so does the fact that it has read something.
- The test sends the SAME single-word task each time on purpose. A different task would retrieve nothing under the AND semantics (see 1126), and then 'no second block' would be the search having missed rather than the ledger having worked — the mutation would have survived against a vacuous test.
- Capped at 256 remembered ids per session; past that a memory could be re-injected. Bounded growth preferred to unbounded in a long-running process.
- The record does not survive a restart of `glasshouse api serve`, by design: a session that restarted has read nothing.

---

## REVIEW — the orchestrator owes an answer to each of these

This section is the point of the generator. Everything above is the
worker's facts, transcribed. Nothing below is decided.

- **1125** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1126** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1127** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1128** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1129** — verdict `refused`. Confirm the worker's reason against current source before recording it.
- **1130** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1131** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1132** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1133** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1134** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1135** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).

**Packet errors the worker reported — read these BEFORE its results.**
Thirteen consecutive rounds a worker corrected its packet and was right:
- FEASIBILITY named `memory::search::MemorySearch::search` as the producer without noting its query semantics. `sanitize_query` (src/memory/search.rs:150-170) quotes each token and joins them with spaces, which FTS5 reads as an implicit AND, so a routed task written as a sentence retrieves nothing. The step is inert for prose tasks. Pinned by `context_injection::a_task_written_as_a_sentence_retrieves_nothing_because_the_search_ands_its_terms`.
- ACCEPTANCE TEST 5 says 'an absurd caller-supplied bound', but this path has no caller-supplied bound: the request carries no injection limit at all. The only caller input is the task text, so the test asserts the stronger property — an absurd task still yields a bounded injection.
- ACCEPTANCE TEST 6 and mutation (c) are impossible with injection at `spawn_session` alone: a session is spawned once, so nothing can be injected into it twice and the mutation is unkillable. Injection therefore also runs on `Request::SendMessage` — line 1125 says 'routed task', not 'spawn'; it is the same `SessionApi::send_text` seam with the same `MessageOrigin::Machine`, and the caller's own text is never altered or merged. Flagged rather than assumed.
- EXPECTED FILES listed `crates/glasshouse/src/session/api.rs`; it needed no change, because ruling 1's seam already exists there exactly as required.
- Not a packet error but a live defect found while building: any delivery through this door longer than a terminal's canonical line limit (~1022 bytes, macOS MAX_CANON) is silently discarded AND wedges that session's input permanently. Measured against a real pty: 1000 bytes arrive, 1023 bytes and nothing ever arrives again. `Request::SendMessage` has this today and answers `ok`. `inject::MAX_INJECTED_BYTES = 900` keeps this phase clear of it; fixing `send_message` needs its own packet.

Gates the worker ran (re-run the decisive ones yourself):
- cargo build: clean
- cargo test --test context_injection: 11 passed
- cargo test --test memory_query_api: 9 passed
- cargo test --test project_isolation: 7 passed
- cargo test --test memory_search: 17 passed
- cargo clippy --all-targets --all-features -- -D warnings: clean
- cargo fmt --all -- --check: clean
- scripts/check-doc-boundary.sh: clean
- scripts/blast-radius.sh: every traced target passed — 25 integration targets, --lib 1528 passed, --bin glasshouse 42 passed
- Windows: NOT RUN. `cargo check --target aarch64-pc-windows-msvc` fails with `can't find crate for std` (target listed, std not installed for the active toolchain). src/memory/inject.rs has no cfg, no std::path, no OS string handling, no process/terminal API and no line-ending-dependent logic; src/memory/mod.rs gains one `pub mod` line. `ci-local.sh --windows-vm` at integration is the remaining evidence.

