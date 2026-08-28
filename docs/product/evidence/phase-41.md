# Phase 41 — the project overview, 6 of 15 closeable now, 9 named open

Capability map lines 1650–1664. Sonnet implementer packet, worktree
`glasshouse-phase-41`. Full audit and per-box reasoning are in
`.agent-runtime/report-PHASE-41.md`; this file is the evidence-ledger half.

### Phase 41 — Add a project overview screen that summarizes active sessions, open work, recent memory, and resource state (line 1650)

Contract: Given a project with sessions and memory, when the user presses
`p` in control mode, Glasshouse opens a popup overlay showing sessions
grouped by role/lifecycle, current binding memory, unresolved todos, and an
honest resource-state note, while leaving the underlying shell visible and
every other key still working underneath it.

State: LOCALLY VERIFIED

Production evidence:
- `crates/glasshouse/src/shell/state.rs` — `Overlay::ProjectOverview`,
  `Action::OpenProjectOverview`, `ShellState::open_project_overview`,
  `ShellState::handle_project_overview_key`; the `p` binding lives in
  `handle_control_key`.
- `crates/glasshouse/src/shell/mod.rs` — the run loop's
  `Action::OpenProjectOverview` arm calls `build_project_overview_memory`
  (reads `crate::memory::ProjectMemory`) and hands the result to
  `ShellState::open_project_overview`, exactly the split `Action::OpenSettings`
  already uses for file I/O this module keeps out of `shell/state.rs`.
- `crates/glasshouse/src/shell/view.rs` — `render_project_overview`, dispatched
  from `render`'s `match state.overlay()`.

Regression evidence:
- `shell::view::tests::the_project_overview_says_so_when_a_section_has_nothing`
  — every section (orchestrator, running, waiting, completed, decisions,
  todos) renders with no sessions and no memory.
- `shell::view::tests::the_project_overview_shows_memory_the_run_loop_handed_it`
  — memory the run loop supplies reaches the screen.
- `shell::project_overview_tests::opening_the_project_overview_shows_real_memory`
  — the `p` key, `build_project_overview_memory`, and a real on-disk memory
  database wired together end to end.

Failure/isolation evidence:
- `shell::view::tests::a_project_memory_read_failure_still_opens_with_an_honest_note`
  — a memory read failure still opens the overlay (sessions still show) and
  names the failure rather than silently rendering empty sections.

Platform/external evidence: run on macOS via `cargo test -p glasshouse --lib
shell::` (197 passed). No CI run yet — the local gate was not run for this
package because four other workers were live in the same round (packet's
§40 instruction); see the report for what was and was not run.

Missing evidence: a shipped-binary (pty) run. Not attempted — the map line
does not ask for anything a pty test would prove beyond what the TestBackend
render tests already do, and Phase 11's session overview (this phase's
sibling) was not proven that way either.

---

### Phase 41 — Show the current orchestrator session if one is designated (line 1651)

State: LOCALLY VERIFIED

Production evidence:
- `crates/glasshouse/src/session/store.rs:81-85` — `SessionRole::Orchestrator`,
  already recorded on real sessions (`SessionRecord.role`).
- `crates/glasshouse/src/shell/view.rs::render_project_overview` — finds the
  session with `role == SessionRole::Orchestrator` and renders it, or an
  explicit "no session is designated" line.

Regression evidence:
- `shell::view::tests::the_project_overview_separates_orchestrator_and_workers_by_role_and_lifecycle`
  — an `Orchestrator` session's row appears.
- `shell::view::tests::the_project_overview_says_so_when_a_section_has_nothing`
  — the honest empty-state line when none is designated.

Mutation proof: `.find(|session| session.role == SessionRole::Orchestrator)`
mutated to `.find(|_session| false)` — the orchestrator test failed; restored.

---

### Phase 41 — Show currently running workers (line 1652)

State: LOCALLY VERIFIED

Production evidence:
- `render_project_overview` filters `SessionRole::Worker` sessions by
  `SessionLifecycle::Running`, using the same `SessionRole`/`SessionLifecycle`
  the production launch and lifecycle paths already record.

Regression evidence: same two tests as line 1651.

Mutation proof: the `Running` filter predicate mutated to always-false —
caught by `the_project_overview_separates_...` test; restored.

---

### Phase 41 — Show workers waiting for user input (line 1653)

State: LOCALLY VERIFIED

Production evidence: same filter mechanism, `SessionLifecycle::WaitingForUser`.

Regression evidence: same tests as line 1651. Mutation-proofed alongside the
Running filter (both predicates zeroed in one mutation, both caught).

---

### Phase 41 — Show recently completed workers (line 1654)

State: LOCALLY VERIFIED, with a named interpretation

Contract: "recently" is read as *most recently completed, however long ago
that was* — sorted by `last_activity_at` descending, capped at
`RECENTLY_COMPLETED_ROWS` (5) — rather than a time-boxed window, because
nothing in this codebase currently defines "recent" as a duration. If the
orchestrator wants a stricter definition, this needs a second look before
being called the same box.

Production evidence:
- `render_project_overview` filters `SessionRole::Worker` sessions whose
  `disposition()` is `Closed` or `Resumable`, sorts by
  `last_activity_at` descending, truncates to 5.

Regression evidence:
- `shell::view::tests::the_project_overview_separates_orchestrator_and_workers_by_role_and_lifecycle`
  — a `Stopped` worker with a native id (`Resumable`) appears.

Mutation proof: the disposition filter mutated to always-false — caught;
restored.

---

### Phase 41 — Show important active decisions and constraints (line 1655)

State: LOCALLY VERIFIED — first production caller of `MemoryStore::binding`

Contract: Given current (`Active`), classified (`authority.is_some()`,
`is_binding()`) memory, when the project overview opens, Glasshouse lists
each one's kind and a short summary, sourced from the same query
`MemoryStore::binding` already defines for exactly this purpose.

Production evidence:
- `crates/glasshouse/src/memory/store.rs:1283` — `MemoryStore::binding`,
  which before this phase had **no production caller** — only
  `tests/memory_authority.rs` exercised it (§35 in
  `docs/process/orchestration-practice.md`).
- `crates/glasshouse/src/shell/mod.rs::build_project_overview_memory` — the
  first production call, reached from `Action::OpenProjectOverview`.

Regression evidence:
- `shell::project_overview_tests::active_decisions_and_constraints_are_read_through_the_real_binding_query`
  — a `Constraint` and a `Decision`, both classified, are read back; an
  unclassified `Finding` is not.

Mutation proof: replaced the `store.binding(...)` call with a hardcoded
empty `Vec` — the test above failed (`left: 0, right: 2`); restored.

---

### Phase 41 — Show unresolved project-memory todos (line 1656)

State: LOCALLY VERIFIED — first production caller of `memory::snapshot`

Contract: Given `MemoryKind::Todo` memories with `MemoryStatus::Active`
(current, hence still open per `MemoryStatus::is_open_work`), when the
project overview opens, Glasshouse lists each one and reports how many more
exist beyond the shown budget.

Production evidence:
- `crates/glasshouse/src/memory/snapshot.rs` — `snapshot()` and
  `Snapshot::section(MemoryKind::Todo)`, which before this phase had **no
  production caller** anywhere in `crates/glasshouse/src` — only
  `tests/memory_snapshot.rs` exercised it.
- `crates/glasshouse/src/shell/mod.rs::build_project_overview_memory` — the
  first production call.

Regression evidence:
- `shell::project_overview_tests::only_unresolved_todos_are_shown` — an open
  todo appears; one marked `Resolved` does not.
- `shell::view::tests::the_project_overview_shows_memory_the_run_loop_handed_it`
  — the omitted-count line renders when the run loop reports one.

Mutation proof: replaced `snap.section(MemoryKind::Todo)`'s match with a
hardcoded `None` — `only_unresolved_todos_are_shown` failed
(`left: 0, right: 1`); restored.

---

### Phase 41 — Show known resource degradation or quota pressure (line 1657) — OPEN

State: NOT STARTED, blocked

Nothing in `crates/glasshouse/src` records resource degradation or quota
pressure. `crate::provider::registry::QuotaModel` (registry.rs:61-83) names
only the *shape* a resource's quota takes (subscription / metered / unmetered
/ delegated); its own doc comment (registry.rs:29-32) states plainly that
"live quota telemetry... is Phase 32B, which does not exist yet." There is no
mechanism this box could call. The overview's "RESOURCE STATE" section shows
one honest line naming the blocking phases rather than fabricating a value.

Blocked on: Phase 32A (unified quota/capacity model) and Phase 32B (quota
telemetry sources) — named in `docs/product/capability-map.md`.

---

### Phase 41 — Show normalized remaining-capacity bands for configured resources (line 1658) — OPEN

State: NOT STARTED, blocked. Same reasoning as line 1657: no capacity value
of any kind is measured anywhere in this codebase to normalize into a band.

Blocked on: Phase 32A / 32B.

---

### Phase 41 — Show whether each displayed capacity value is measured, estimated, manual, or unknown (line 1659) — OPEN

State: NOT STARTED, blocked. This box presupposes line 1658's capacity
values exist to tag; they do not.

Blocked on: Phase 32B.

---

### Phase 41 — Show the next known or estimated reset time for constrained resources (line 1660) — OPEN

State: NOT STARTED, blocked. `QuotaModel`'s doc comment (registry.rs:58-60)
explicitly excludes "a rolling-window reset time" from what it tracks.

Blocked on: Phase 32B.

---

### Phase 41 — Show the currently selected routing model and its recent latency (line 1661) — OPEN

State: NOT STARTED, blocked (half-exists, half absent)

`crate::config::RoutingModelChoice` (config/mod.rs:1182) records a
*configured intent* (deterministic / automatic / pinned), already surfaced in
the Settings overlay's routing row — not an *observed, currently active*
selection at runtime. `crate::config::RouterLatencyMs` (config/mod.rs:1339)
is a configured *ceiling* ("max acceptable router latency"), not a measured
recent latency; nothing in `routing/mod.rs` or `routing/interactive.rs`
records an observed latency. Since half of what the line asks for does not
exist, the honest answer to "can Glasshouse show this" is no, not "half of
it, sort of."

Blocked on: Phase 33A (routing evidence ledger) and Phase 34B/34C
(routing-model role, automatic selection) — named in the capability map.

---

### Phase 41 — Show the harness, backend, model, pairing class, and response profile for active sessions when relevant (line 1662)

State: LOCALLY VERIFIED

Contract: Given an active session, when the project overview shows it (as
orchestrator, running, or waiting), Glasshouse's row includes all five
already-recorded facts.

Production evidence:
- `crates/glasshouse/src/session/store.rs:452-508` — `SessionRecord`'s
  `harness`, `backend_resource`, `model`, `pairing_class`,
  `response_profile` fields, all populated on the real launch and lifecycle
  paths (outside this phase's scope; not re-verified here).
- `crates/glasshouse/src/shell/view.rs::session_detail` — the first place
  in the shell that renders all five together for one session; the
  session-level Overview (Phase 11) shows only `harness`.

Regression evidence:
- `shell::view::tests::the_project_overview_separates_orchestrator_and_workers_by_role_and_lifecycle`
  renders sessions through this path (implicitly covers `session_detail`
  not panicking on `None` fields); no test asserts the exact detail string
  because the fixtures used leave those fields `None` — see Missing
  evidence.

Missing evidence: no test asserts a *populated* `backend_resource` / `model`
/ `pairing_class` / `response_profile` actually appears in the rendered
text (all fixtures used `None` for these fields, which exercises the
`"unresolved"`/`"not recorded"` fallback branches, not the `Some` branches).
Worth a follow-up unit test that sets these fields and asserts the row
contains them.

---

### Phase 41 — Show protected premium reserves when they influence routing (line 1663)

> **SUPERSEDED 2026-08-29 — the account below was true when written and is not
> true of current source.** Batch 42 wired `ProviderConfig::metered_models`, and
> with it the reserve reached a live routing decision; nothing came back to
> update this entry, so it went on telling three later packets that the box was
> blocked. A worker checked the citation against source before trusting it
> (§5), found it stale, and closed the line. **Verified independently by the
> integrator before accepting:**
>
> - `main.rs:1387` — `disposable_candidate_capacity` folds the configured
>   reserve in: `.with_resource_reserve(effective.reserve_percent(provider).value.get())`
> - `main.rs:1395` — `.with_band(band)` carries it onto the candidate
> - `routing/disposable.rs:558-566` — `evaluate_reserve_spend(ReserveDecisionInputs { band: candidate.value().capacity.band, .. })`,
>   a real allow/deny gate on the metered-fallback path
> - `main.rs:1336` — `disposable_candidate_capacity`'s production caller,
>   confirmed by `discover.py --seam`
>
> **The transferable part: an evidence entry that records a blocker does not
> expire when the blocker does.** This one outlived its own truth by three
> batches because nothing re-reads a `NOT STARTED, blocked` entry when the thing
> it was blocked on ships. When a batch removes a blocker, grep the ledger for
> entries that named it.

State (superseded): NOT STARTED, blocked

`crate::config::EffectiveConfig::premium_reserve` (config/mod.rs:2300) is a
real, configurable percentage — but nothing in `crate::routing` reads
`premium_reserve_percent` to make any decision. The map's own neighbouring
open boxes (protecting premium capacity in a "reserve band", preferring
alternative resources when tight) are still unchecked, confirming the
routing effect this line requires does not exist. Since the reserve never
influences routing yet, the box's condition ("when they influence routing")
can never be true, so this is left open rather than showing an inert
percentage as if it meant something.

Blocked on: the reserve-band routing logic capability-map lines ~1571/1287
describe, which has no implementation yet (Phase 33/34-adjacent, unnamed
more precisely in the map itself).

---

### Phase 41 — Keep the overview factual and derived from stored state rather than generating decorative AI commentary by default (line 1664)

State: LOCALLY VERIFIED, by construction

Contract: `render_project_overview` is a pure function of `&ShellState` (see
`shell/view.rs`'s own module doc: "reads, never mutates, and never blocks"),
and every line in it is either a `SessionRecord` field or a string
`build_project_overview_memory` copied (truncated, never rephrased) from
`MemoryStore`/`memory::snapshot`. No model client is reachable from
`shell/view.rs`'s render path.

Production evidence:
- `crates/glasshouse/src/shell/view.rs::render_project_overview` and
  `session_detail` — no call into any harness, provider, or LLM client.
- `crates/glasshouse/src/shell/mod.rs::summarize_memory_line` — truncates
  the stored `subject`/`body` verbatim; never paraphrases.

Regression evidence: none written specifically for "no model call happens"
— this is an architectural property (the render path's function signature
takes no network/model handle), not a runtime behavior a unit test can
observe. Flagging this as the honest gap: a stronger proof would be a
compile-time check (e.g. `render` taking no `Runtime`/client argument),
which already holds structurally but is not asserted anywhere.
