# Phase 41 — the project overview, 14 of 15 closed; only 1661 left open

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

### The four capacity lines, closed together 2026-08-29 (lines 1657–1660)

**These four entries previously read `NOT STARTED, blocked` on Phase 32A/32B,
and that had stopped being true.** The blockers shipped; nothing came back to
re-read the entries. This is the second time in one batch the same failure mode
surfaced — see 1663 below — and the transferable rule is recorded there.

Closed by `GH-PHASE-41-OVERVIEW` (worktree `.worktrees/phase-41-overview`,
report `.agent-runtime/report-phase-41-overview.md`). One mechanism serves all
four, split deliberately into two halves so the honesty rules are unit-testable:

- `crates/glasshouse/src/shell/mod.rs` — `build_project_overview_capacity`, the
  I/O half. Scopes to `EffectiveConfig::provider_names()` — the project's own
  **configured** providers, not the full `provider::registry::registry()`
  catalog — reads the on-disk `GatewayQuotaCache` exactly as
  `main.rs::resources_report` and `main.rs::disposable_candidate_capacity` do,
  and folds each provider's `reserve_percent` into `CapacityBandThresholds` the
  same way a real routing decision does. A config-read failure becomes one
  honest line; it cannot fail visibly.
- `crates/glasshouse/src/shell/mod.rs` — `resource_capacity_line`, the pure
  formatting half.
- `crates/glasshouse/src/shell/state.rs` — `ProjectOverviewState::resources`.
- `crates/glasshouse/src/shell/view.rs` — `render_project_overview`'s
  "RESOURCE STATE" section, which was a static "not tracked in this build"
  placeholder and now renders one line per configured resource, or an honest
  "no resources configured for this project" when empty.

**Scope decision (integrator-accepted):** "configured resources" means
`provider_names()`, whose own doc says a provider exists there only because a
user or project explicitly configured one — the literal match for the contract's
wording, and the same set `disposable_candidates` already scores a routing
decision over. `NativeSubscription` and `GlasshouseGateway` are not listed
because `reserve_percent` only applies to `DirectProvider`. Adding them is a
straightforward follow-up, not a blocked capability.

Caller reachability (§35) is proven, not assumed, by
`build_project_overview_capacity_reads_a_real_configured_provider_and_a_real_planted_reading`
— a real `UserConfig` on disk and a real `GatewayQuotaCache::store` reading, no
hand-built `CapacityState`.

---

### Phase 41 — Show known resource degradation or quota pressure (line 1657)

Contract: Given a project with configured resources, when the user opens the
project overview, Glasshouse shows each resource's known degradation or quota
pressure, while never inventing a figure for a resource it has no reading for.

State: COMPLETE

Production evidence: `build_project_overview_capacity` + `resource_capacity_line`
+ `render_project_overview`, as above.

Regression evidence:
- `shell::mod::project_overview_capacity_tests::a_measured_reading_renders_its_band_and_says_measured`
- `shell::mod::project_overview_capacity_tests::no_configured_providers_yields_no_resource_lines`
- `shell::view::tests::the_project_overview_shows_resource_capacity_the_run_loop_handed_it`
  — rendered at (120,40) and (400,40) per §17.

---

### Phase 41 — Show normalized remaining-capacity bands for configured resources (line 1658)

Contract: Given a resource with a remaining-capacity reading, when the overview
renders it, Glasshouse shows a normalized band, while showing no number at all
for a resource whose capacity is unknown.

State: COMPLETE

Regression evidence:
- `project_overview_capacity_tests::a_measured_reading_renders_its_band_and_says_measured`
- `project_overview_capacity_tests::no_telemetry_renders_unknown_with_no_number_at_all`
- `shell::view::tests::an_unknown_resource_never_shows_a_number_at_a_realistic_and_a_wide_width`
  — both widths, because a wide terminal is where a stray number would reappear.

Mutation: `accept-stale-state` — the unknown branch made to append `" 0%"`.
**Killed** by `no_telemetry_renders_unknown_with_no_number_at_all`:
`must show no number at all: openrouter (remote) capacity unknown 0%`.

---

### Phase 41 — Show whether each displayed capacity value is measured, estimated, manual, or unknown (line 1659)

Contract: Given a displayed capacity value, when the overview renders it,
Glasshouse labels that specific number's provenance, while never labelling an
estimate as measured.

State: COMPLETE

**The design decision worth recording**, because a first draft got it wrong and a
test caught it: the label comes from the displayed number's own class
(`Percentage::exact()` / `estimated()`), **not** from
`CapacityState::telemetry_class()`, which answers the resource's *best* class
across every pool and would report "measured" while the number actually on screen
was an estimate. `TelemetryClass`'s five variants collapse into the box's four
words — `Authoritative` and `Observed` both read as "measured", since both are
real readings nobody inferred — and that collapse applies only in the
no-normalized-score branch.

Regression evidence:
- `project_overview_capacity_tests::a_measured_and_an_estimated_reading_of_the_same_resource_render_differently`

Mutation: `remove-validation` — `[{class_word}]` dropped from the final format
string. **Killed**: both readings normalize to 82%, so with the label stripped the
two lines became byte-identical (`openrouter (remote) plenty 82%`).

---

### Phase 41 — Show the next known or estimated reset time for constrained resources (line 1660)

Contract: Given a constrained resource with a known reset, when the overview
renders it, Glasshouse shows that reset, while showing none for a resource that
is not constrained.

State: COMPLETE

Regression evidence:
- `project_overview_capacity_tests::a_constrained_resource_with_a_known_reset_shows_it`
- `project_overview_capacity_tests::an_unconstrained_resource_shows_no_reset`

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

State: COMPLETE

Contract: Given a resource whose protected premium reserve is currently able to
deny a metered routing request, when the overview renders that resource,
Glasshouse says the reserve is limiting routing here, while staying silent about
a reserve that is influencing nothing.

Production evidence:
- `crates/glasshouse/src/main.rs` — `disposable_candidate_capacity` folds the
  configured reserve in via
  `.with_resource_reserve(effective.reserve_percent(provider).value.get())`, and
  `.with_band(band)` carries it onto the candidate.
- `crates/glasshouse/src/routing/disposable.rs` — `choose` feeds that band to
  `evaluate_reserve_spend`, a real allow/deny gate on the metered-fallback path.
- `crates/glasshouse/src/shell/mod.rs` — `resource_capacity_line` renders the
  reserve clause when `band <= CapacityBand::Reserve`.

**The condition is the same boundary the router itself gates on**, not a
threshold invented for the display: `evaluate_reserve_spend`'s own precedence
comment says that above `CapacityBand::Reserve` nothing is protected and every
request is allowed. So a resource at or below that band is one where the reserve
policy actually runs and can deny — which is exactly "influencing routing".

**Deliberately present-tense.** No reserve denial is persisted anywhere — the
decision is computed per-task inside `DisposableRouting::choose` and
`routing::evidence::EvidenceLedger` records provider/model/route/cost/outcome,
not the reserve gate's reason. So this line answers "is the reserve gating this
resource now", not "did it deny some past request". A historical audit trail
would need `EvidenceLedger` extended to record `ReserveDecision::Deny` reasons;
that is new scope, and claiming the past-tense reading today would have been
fiction.

Regression evidence:
- `project_overview_capacity_tests::a_reserve_that_currently_gates_routing_appears`
- `project_overview_capacity_tests::a_reserve_that_influences_nothing_does_not_appear`

Mutation: `invert-condition` — `band <= CapacityBand::Reserve` flipped to
`band > CapacityBand::Reserve`. **Killed** by
`a_reserve_that_influences_nothing_does_not_appear`, which then showed
`plenty 80% [measured]; protected reserve 20% is limiting routing here`.

---

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
