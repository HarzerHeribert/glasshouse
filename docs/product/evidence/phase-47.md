# Phase 47 — observability without spectacle, 4 of 15 closeable now, 2 named open, 9 blocked

Capability map lines 1757–1771. Sonnet implementer packet, worktree
`glasshouse-phase-47`. Full audit and the mutation ledger are in
`.agent-runtime/report-PHASE-47.md`; this file is the evidence-ledger half.

Nine of the fifteen lines (1757, 1759–1762, 1764–1767) are not this
package's — they name a router, routing-evidence table, cache-temperature
estimate, quota telemetry, and correlation machinery that does not exist yet
on this tree (Phases 30/32B/33/33A/33B/33C/34/37 are all at 0 boxes). The
packet named them and this worker did not attempt them. This ledger covers
only the six lines the packet assigned.

---

### Phase 47 — Add a debug view showing recent lifecycle events for a session (line 1758)

State: **COMPLETE** — promoted from this package's LOCALLY VERIFIED by the
orchestrator, with the residual gap named rather than closed over.

**What the orchestrator added, by running the shipped binary (practice §38).**
`./target/debug/glasshouse` was started in a real cmux pane and its status bar
reads:

    tab session   enter session   n new   N headless   o overview   p project   e events   q qu

So the binding is live in the shipped binary, not only in a render test — and the
footer is visibly truncated at that width, which is the arithmetic this package
reported and the reason it widened `the_status_bar_shows_a_note_next_to_the_bindings`
to 132 columns.

With **zero** sessions recorded, `e` opens nothing — and neither does `o`
(overview). That was checked rather than assumed: the two behave identically, so
the no-op is this shell's established pattern for "there is no presented session",
not something this package introduced.

**Still not driven end to end, and this is the honest limit of the evidence.** The
overlay has not been watched rendering a *populated* event list in a real terminal,
because that needs a recorded session and this project has no pty harness driving
`glasshouse run`'s interactive TUI loop at all. The package said so itself and
correctly declined to build one: `docs/process/worker-capabilities.md` puts PTY
lifecycle at Red tier. **That harness is a follow-up package, and it would serve
far more than this line** — every TUI contract in the map is currently proved by
render tests alone.

What carries the box in the meantime is that the production path is
mutation-proved rather than merely present: deleting `render`'s
`Some(Overlay::SessionEvents)` arm fails two named tests, and deleting the `'e'`
arm of `handle_control_key` fails four. A caller that can be deleted without a test
noticing is not a caller (§35); this one cannot be.

Contract: Given the presented session, when the user presses `e` in control
mode, Glasshouse opens a popup overlay showing that session's own recent
lifecycle events (name and age), while leaving the underlying shell visible
and every other key still working underneath it — the same shape
[`Overlay::ProjectOverview`] (Phase 41) already established for `p`.

Production evidence:
- `crates/glasshouse/src/shell/state.rs` — `Overlay::SessionEvents`,
  `ShellState::open_session_events`, `ShellState::handle_session_events_key`;
  the `e` binding lives in `handle_control_key`.
- `crates/glasshouse/src/shell/view.rs` — `render_session_events`, dispatched
  from `render`'s `match state.overlay()`, filtering
  `ShellState::activity()` (already populated in production by
  `ShellState::note_events`, called from the run loop before this package —
  see "what this packet got wrong" below) down to the presented session's own
  `SessionId`.

No file outside `shell/state.rs` and `shell/view.rs` was needed: the events
this overlay shows were already recorded in production before this package,
by a run-loop call this package did not add or touch.

Regression evidence:
- `shell::view::tests::session_events_shows_only_the_presented_sessions_events_at_a_realistic_and_a_wide_width` —
  two sessions each get an event; only the presented session's text appears,
  at 100 and 400 columns (practice §17).
- `shell::view::tests::session_events_says_so_when_the_presented_session_has_none` —
  the honest empty state.
- `shell::view::tests::e_opens_and_esc_closes_the_session_events_overlay` —
  the key toggles the overlay exactly like `o` and `p` already do.
- `shell::view::tests::the_session_events_footer_names_its_own_key` — the
  overlay's own footer and the control-mode footer advertising `e`.

Mutation proof (practice §35, §41 — mutate the call, not the callee):
- `render`'s `Some(Overlay::SessionEvents) => render_session_events(...)`
  mutated to `=> {}` — killed by two named tests; restored.
- `handle_control_key`'s `KeyCode::Char('e') => self.open_session_events()`
  mutated to `=> Action::None` — killed by four named tests; restored.
- The session filter, `recorded.session() == &session.id`, mutated to
  `|_recorded| true` — killed by the cross-session test above (the other
  session's event text appeared); restored.

Failure/isolation evidence: line 1770's property, for this overlay
specifically — see line 1770 below.

Missing evidence: a shipped-binary (pty) run. Not attempted, and not this
worker's tier to build: `docs/process/worker-capabilities.md`'s risk routing
puts "PTY lifecycle, shutdown, signals, and job control" at Red
(Opus specialist), and no pty harness in this tree currently drives
`glasshouse run`'s interactive TUI loop at all — `tests/session_model.rs`'s
own pty cluster drives `glasshouse resume` through a real terminal, not the
shell's key handling. Building one from nothing was out of scope for this
package; see practice §38, which the packet quoted, and which asks the
orchestrator for a cmux pane rather than a worker inventing a new harness.
Phase 41's project overview (this line's direct sibling) was not proven that
way either, for the same reason.

---

### Phase 47 — Show failure counts by class instead of presenting one unexplained error percentage (line 1763) — OPEN

State: NOT STARTED, blocked on a file this package does not hold

Contract asked: does `routing/free.rs` already count failures by class
(`RateLimited` vs `CapacityFailure` vs `CredentialRejected`), so the shell
could surface an existing count? It does not.
`routing::free::ResourceHealth` (free.rs:236-243) keeps exactly two fields:
`consecutive_failures: u32`, a single running count that **resets to zero on
any success** and does not distinguish which `WorkloadOutcome` variant
caused it, and `credential_rejected: bool`, a flag with no count at all. No
per-class tally exists anywhere in this codebase for either the shell or
anything else to read.

Nor is there an existing "one unexplained error percentage" in the shell to
replace — `crates/glasshouse/src/shell/view.rs` and `state.rs` contain no
error-percentage display of any kind (checked by grep for `%`, `percentage`,
`error_rate`, `failure_rate`, `success_rate`; none of the four match
anything relevant). So this line is not "swap a percentage for counts", it
is "the counts do not exist to swap in."

Building the counter is `routing/free.rs` work, which this packet's
`FORBIDDEN FILES` names explicitly (D4): "if only the latest outcome is kept,
say so and leave the box — do not add a counter to `routing/**`." The latest
outcome is not even what is kept; a single resettable streak is. Left open.

Blocked on: a `routing/free.rs` change (out of this partition this round) to
record per-class counts, before any shell surface can show them.

---

### Phase 47 — Keep lifetime token and spend totals out of the default project overview and never present them as achievement counters (line 1768)

State: **COMPLETE** — promoted by the orchestrator. This is an *absence* claim, and
it is proved in both directions at two widths (120 and 400) plus a CRLF-agnostic
source scan over the function body. Practice §17 exists because this project once
shipped an absence test that passed only because the value was clipped off-screen
at 100 columns; that failure mode is closed here by construction. No platform
claim is made and none is required — the assertion is about what `render_project_overview`
emits, and the render path is the production path.

Contract: `render_project_overview` must never render a lifetime token or
spend total, under any fixture, at any width, now or after a future edit to
that function.

Production evidence:
- `crates/glasshouse/src/shell/view.rs::render_project_overview` — every
  section is a `SessionRecord` field, a memory string the run loop supplied,
  or one of six fixed section headings; nothing sums tokens or cost across
  sessions or time. (No such field exists on `SessionRecord` or
  `ProjectOverviewState` for it to sum in the first place.)

Regression evidence (practice §17 — realistic width and 400 columns, because
a value that happened to clip off-screen would make an absence assertion
true for the wrong reason):
- `shell::view::tests::the_project_overview_never_shows_a_lifetime_token_or_spend_total` —
  renders a populated overview (orchestrator, worker, a decision, a todo) at
  120 and 400 columns and asserts none of `token`, `spend`, `achievement`,
  `lifetime` (case-insensitively) appear anywhere on either screen.

Mutation proof, in both directions (practice §17 — a hardened test proves
nothing alone):
- **Mutant → FAIL**: added `lines.push(Line::from("lifetime tokens:
  999999"))` to `render_project_overview`. The test above failed at width
  120 (and would have at 400 — the line is unconditional). The companion
  source-scan test (`the_lifetime_total_scan_is_crlf_agnostic_and_can_say_no`)
  failed simultaneously, for the same mutant, by a different mechanism.
- **Clean code → pass**: reverted the line; both tests pass. `cargo test -p
  glasshouse --lib shell::` — 204 passed, 0 failed, both before introducing
  the mutant and after reverting it.

Source-scanning guard (the packet's "not only a test" — a property of the
source, so a future PR adding a total under a fixture this test never
exercises still gets caught):
- `shell::view::tests::project_overview_never_names_a_lifetime_total` scans
  `render_project_overview`'s own body (found by its `fn` line, ended at the
  next `fn` at any indentation) for `token`, `spend`, `achievement`,
  `lifetime`, case-insensitively. Scoped to that one function specifically —
  `session_detail`'s per-session `model`/`backend` line and Settings' own
  "Maximum marginal cost" routing knob (`state.rs:1326`, a per-decision
  ceiling, not a lifetime total) both contain unrelated words this scan must
  not trip on, and the control test proves it does not.
- Read by `str::lines` (§14), not a multi-line string search: the packet
  named the exact prior incident (a CRLF checkout defeating a multi-line
  literal search, red on Windows CI once already) and
  `shell::mod::run_loop_passes_the_default_timeouts` is the precedent this
  follows line for line.
- `shell::view::tests::the_lifetime_total_scan_is_crlf_agnostic_and_can_say_no`
  proves the scan passes on the real file in both an LF and a synthesized
  CRLF copy, and is capable of saying no on a synthetic source that names a
  forbidden term inside the function (and capable of *not* saying no when the
  term is outside it).

---

### Phase 47 — Add a debug view showing memory-extraction inputs and outputs when explicitly enabled (line 1769) — OPEN

State: NOT STARTED, blocked on files this package does not hold

The hypothesis this packet invited killing (§44) fails here. Memory
extraction (`crate::memory::extract`) does not run inside the interactive
`glasshouse run` process at all: its only production caller,
`run_extraction_after_turn`, is invoked from `report_hook_with` in
`main.rs:1247`, which is a **separate process invocation** — a harness
lifecycle hook, run once per completed turn, entirely outside the shell's
run loop and its `ShellState`. `memory/extract/mod.rs`'s own doc comment on
`Extractor::run` (line 411) says this explicitly: "a Glasshouse that runs
extraction while a TUI owns the terminal must install a [panic] hook of its
own" — a forward-looking caveat for work that has not happened, because
today the TUI and extraction are different processes.

What extraction produces is not persisted anywhere the shell could read it
even if it were the same process: `run_extraction_after_turn` sends its
`ExtractionOutcome` — the closest thing to "outputs" this codebase has — to
`tracing::info!`/`tracing::warn!` only (main.rs:1396-1410). The *inputs* (the
bounded `SessionChunk` built from `EventLog::recent_for_session`) exist only
as a local on extraction's own thread and are dropped when it ends. Neither
side survives past the hook process exiting.

Closing this box needs, at minimum: persisting an extraction record
somewhere durable and readable (a `memory/extract/**` or `main.rs` change),
and a way for the interactive shell process to read it (a `shell/mod.rs`
change, since reading anything outside the in-memory `ShellState` is file
I/O this module's pure render functions are not allowed to perform — see
`render_session_events`'s own doc comment above for the same constraint
applied to line 1758). `main.rs` and `shell/mod.rs` are both named in this
packet's `FORBIDDEN FILES`. Left open, per D5's own instruction: "if you
conclude it needs [something outside this partition], stop and report."

Blocked on: a `memory/extract` or `main.rs` change to persist the exchange,
plus a `shell/mod.rs` change to read it into `ShellState` — neither in this
partition this round.

---

### Phase 47 — Keep diagnostic views optional and do not turn them into the normal user experience (line 1770)

State: **COMPLETE** for the one new surface this package adds — promoted by the
orchestrator. Proved by asserting the new overlay is absent from the default screen
at both widths, which is the whole content of "diagnostic views stay optional".
The line is a standing property, so every future diagnostic surface has to re-prove
it; that is a fact about the line, not an incompleteness in this entry.

Contract: `Overlay::SessionEvents` must never appear unless the user presses
`e` — never on the default screen, at any width.

Production evidence: `render`'s `match state.overlay()` only reaches
`render_session_events` when `state.overlay() == Some(Overlay::SessionEvents)`,
which only `ShellState::open_session_events` sets, which only the `e` key
reaches (see line 1758's key-mutation proof above — mutating that one call
site to `Action::None` killed four tests, so it is the only path in).

Regression evidence:
- `shell::view::tests::session_events_is_absent_from_the_default_screen_at_a_realistic_and_a_wide_width` —
  `sample()`'s default screen (no overlay open) at 100 and 400 columns
  contains no `"session events"` text, proving the overlay does not leak
  into the screen a user sees without asking for it.

This line is a property of the whole phase, not only this package's one
surface, but the other five lines this packet named are either blocked
(1757/1759-1762/1764-1767, not built) or already gated the same way by an
existing key (`p`/`Overlay::ProjectOverview`, proven by Phase 41's own
`the_project_overview_footer_names_its_own_key` and unaffected by this
package).

---

### Phase 47 — Prefer inspectable text and tables over animated knowledge-graph visualizations (line 1771)

State: **COMPLETE** — promoted by the orchestrator. The pre-existing block-element
guard (`nothing_draws_with_block_elements_so_the_design_stays_text_first`, scanning
U+2580–259F) was extended to cover both new overlays and mutation-proved against
one of them. Glasshouse renders through Ratatui and has no animation anywhere, so
the honest close is a guard that keeps it that way rather than a new abstraction,
by
package's new surfaces

Contract: no screen in the shell — including the two overlays this package
touches — draws with Ratatui's decorative widgets (`Gauge`, `Sparkline`,
`BarChart`), which are the only way an animated or graph-like visualization
would be built in this codebase; Glasshouse has no other rendering path.

Production evidence: `render_project_overview` and `render_session_events`
both build a plain `Vec<Line>` and hand it to a `Paragraph` — the same
primitive every other screen in this file uses. Neither calls a Ratatui
widget outside `Block`/`Paragraph`/`Clear`.

Regression evidence: `shell::view::tests::nothing_draws_with_block_elements_so_the_design_stays_text_first`
predates this phase (it already covered the shell's default screen and the
session Overview) and is extended here to also render the ProjectOverview
and SessionEvents overlays, scanning every screen's buffer for Unicode block
elements (U+2580–U+259F) — the range every one of Ratatui's decorative
widgets draws with.

Mutation proof: pushed a line containing `"███"` into `render_session_events`
— the extended test failed, naming the character and the screen. Restored;
the full `shell::` suite (204 tests) passes clean before and after.

One-line evidence argument beyond the guard: this project's rendering has no
animation loop at all to produce a moving visualization from — every frame
in `shell::view::render` is drawn once, synchronously, in response to a key
or a run-loop event, never on a timer, so there is no mechanism by which a
"graph visualization" could animate even if one were drawn.


### Phase 47 — lines 1762 and 1764: returned once as premise-invalid, then CLOSED

State: **COMPLETE** for 1762 and 1764, in batch 43. **The batch-42 attempt was
returned as premise-invalid and that was the right call** — the account below is
kept because the returned packet is what made the closing one possible.

**The gap.** The line asks for *"one row per observed (provider, model, route)
identity"*. That requires knowing **which identities exist**, and
`EvidenceLedger` cannot answer that. Its entire public surface is `record`,
`recent` and `summarize`, and both readers take an `ObservationQuery` whose
`provider` and `model` are **required `&str`** — the caller must already name the
exact identity. `ObservationQuery`'s own doc is explicit that a `None` route
*"matches rows recorded with no route, not 'any route'"*. **There is no wildcard
and no listing operation.**

**What the packet got wrong.** Its five links checked producer, caller,
propagation, consumer and variability — and all five hold for a *lookup*. None of
them asks whether the data can be **enumerated**. A table is not a lookup: it
needs the set, and the set had no producer. The orchestrator verified the
worker's finding directly against `routing/evidence.rs` before accepting it.

**What the worker refused to do, and this is the valuable part.** It could have
reconstructed the identity list from session and provider configuration. That
would have rendered a table that *looks* like recorded evidence but is actually a
mix of *configured* and *observed* — a fabricated measurement, which is exactly
what Phase 47's own "observability without spectacle" heading exists to prevent.
It stopped instead, because `routing/evidence.rs` was FORBIDDEN to it (a
concurrent worker's file) and adding a method there would have been lost.

**What would close them.** One small additive method on `EvidenceLedger` that
lists distinct observed identities within a window, then the overlay this packet
described. Both lines then close together, and 1764 remains honest only if the
report states plainly that `context_state` reads `Unknown` on 100% of real rows —
`NewObservation::with_context_state` still has zero non-test callers.

Missing evidence: an enumeration method on `EvidenceLedger`. Nothing else.


### Phase 47 — 1762/1764 closed: the missing link was one query

**What batch 42 was missing was enumeration, and batch 43 added exactly that and
nothing else.** A `SELECT DISTINCT` over the `routing_observations` rows that
already existed — additive, bounded by a window and a limit, project-scoped, no
migration, and `record` / `recent` / `summarize` / `ObservationQuery` untouched.

Production evidence: the new distinct-identity query on
`routing/evidence.rs::EvidenceLedger`, consumed by a new `ShellState` overlay
built on Phase 25's proven `build_project_knowledge_memory` pattern and rendered
in `shell/view.rs`.

**1762 is closed on TWO of its seven named columns, and the report says so.**
Sample count and observation window are rendered; **TTFC, effective TTFC, TTFT,
decode throughput and rounds-per-minute are not, and have no field anywhere in
the row type, the identity type, or the SQL** — verified by a `bypass-fallback`
mutation proving no code path could draw them even fabricated. The line's own
*"when available"* is what licenses that, and the five absent columns are
structurally unavailable to the gateway's ingress design, not merely unwired.

**1764 is honest, and the honesty is the point.** `ContextState` reads `unknown`
on **100% of real production rows** — `NewObservation::with_context_state` has
zero non-test callers, and the gateway producer has no cache-state signal at all.
The line asks which of *warm / cold / unknown* the evidence came from, and
*unknown* is one of the three it names. **This is a provenance surface, not a
cache-temperature measurement**; line 1760, which wants an estimate, stays out of
scope precisely because building one from an always-`unknown` column would be
inert.

Failure/isolation evidence:

- `no_fabricated_columns_appear_in_the_route_evidence_table` renders at **both**
  120 and 400 columns and scans the flattened buffer for `ttfc` / `ttft` /
  `throughput` / `rounds per minute` / `decode` (§17 — an absence assertion is
  only as strong as the viewport it renders into).
- **One mutation survived, and its disposition is correct.** Removing the new
  query's project scoping killed no test — because cross-project leakage is
  enforced **one layer down by the database schema**, proven by
  `a_foreign_project_id_row_cannot_even_be_inserted_into_this_database`. Recorded
  so a future reader does not mistake the survivor for a weak test.

**`packet_errors: []` — the first this session, and it was earned rather than
skipped.** The worker verified the packet's feasibility block against production
code rather than accepting it: that `EvidenceLedger`'s surface really was
`record`/`recent`/`summarize` with no listing method, that `ObservationQuery`'s
fields really are required `&str`, and that `with_context_state` really has zero
non-test callers.

Missing evidence: the five structurally-unavailable columns, which need a
component that reads the response stream's framing — the same blocker
`routing/evidence.rs`'s own module header records.

---

### Phase 47 — Show route health, immediate availability, cadence, quota reset, and failure-domain evidence as separate concepts (line 1765)

State: **COMPLETE** — promoted by the orchestrator, batch 50. The worker
proposed COMPLETE; the ruling and its one recorded limit are the orchestrator's.

Contract: Given a project whose gateway has observed at least one free
resource, when the user presses `h`, Glasshouse opens a read-only overlay
rendering route health, immediate availability, cadence, quota reset and
failure-domain evidence as five separately labelled concepts — printing
`unknown` rather than a zero for anything no provider stated and never claiming
independent failure domains — while preserving the default screen and every
other binding.

**Why this was closeable when seven sibling lines were not.** The shell process
holds no router and no gateway: `shell::run` (`shell/mod.rs:72`) takes only a
`&Runtime` and is reached from `main.rs:326`, while the gateway starts at
`main.rs:535`/`:1088`. A shell debug view can therefore render only what is
durable on disk. This line's five concepts are, because `gateway/mod.rs`'s
accept loop writes `GatewayQuotaCache` and `GatewayHealthCache` on every
forwarded exchange and `glasshouse resources` already reads them back from a
different process. This view is a second reader of that same seam.

**Why the line was not already satisfied.** The pre-existing consumer,
`provider::resources::render_health` (`resources.rs:683`), prints route health,
immediate availability and cadence as **one status word on one line**, quota
reset from a different function, and failure-domain evidence nowhere. That
collapse is what line 1765 forbids.

Production: `shell/mod.rs::build_route_health_table` and the
`Action::OpenRouteHealth` arm; `shell/state.rs::{RouteHealthRow,
ShellState::open_route_health, handle_route_health_key}` and the `h` binding in
`handle_control_key`; `shell/view.rs::{render_route_health, describe_deadline}`.

Regression: 6 builder tests in `shell::route_health_tests`, 9 render tests in
`shell::view::tests`, and 5 external tests in `tests/observability_views.rs`.
Every render assertion runs at a realistic width and at 400 columns. The
"no two concepts on one line" check uses the raw buffer rather than the
flattened one, because it is a claim about lines and flattening would make it
unfalsifiable.

Mutations — four KILLED, re-run by the orchestrator in the integrated tree:

| mutation | result | killed by |
|---|---|---|
| collapse `immediate availability` into `route health` | KILLED | `route_health_keeps_line_1765s_five_concepts_on_separate_lines` |
| render an unstated cadence as `0 request(s) per 0s` | KILLED | `route_health_says_unknown_rather_than_zero_for_what_no_provider_stated` |
| `FailureDomain::Unknown` → `Independent` in the builder | KILLED | `two_resources_on_one_provider_are_shared_and_never_independent` |
| drop the installation-scope header | KILLED | `the_route_health_view_names_its_scope_and_prints_no_secret` |

**RECORDED LIMIT — the run-loop dispatch arm is unwatched.** A fifth mutation,
`state.open_route_health(build_route_health_table(runtime))` →
`open_route_health(Vec::new())`, **SURVIVED** against 275 real tests. It is not
a false survivor: the line is on the production path (pressing `h` in the
shipped binary reaches it) and no test reaches it, because it lives inside
`shell::run`'s event loop and **nothing in this tree drives the interactive TUI
loop**. This is pre-existing and systemic — the `Action::OpenRouteEvidence` arm
three lines above has the identical gap, as does every other overlay dispatch
in that `match`.

A structural test was deliberately **not** added to paper over it:
`main.rs`'s own `every_gateway_the_binary_starts_is_given_the_evidence_ledger`
records that such tests prove structure and that *"Phase 33A's boxes do not
close on this test."* The honest fix is a pty harness driving `glasshouse run`'s
key handling, which would serve every TUI contract in the map rather than this
one line. **Named as the highest-leverage follow-up package.**

Further limits: the overlay shows installation-wide readings, because both
caches live under `data_dir()` and not `project_state_dir` — the view labels its
own scope and a test fails if that label is removed. `failure_domain` is
computed from the provider grouping rather than `FailureDomain::between`,
because neither cache stores a `Backend`; `Independent` is unreachable by
construction.

---

### Phase 47 — lines 1757, 1759, 1760, 1766, 1767 returned premise-invalid (batch 50)

State: **NOT STARTED** for all five. Each was checked to the four-link standard
against current source and then attacked by an independent read-only
subcontractor, which found no refutation.

- **1757** — `RoutingExplanation` (`routing/mod.rs:475`) has no durable sink:
  every production sink is a `tracing` line or an in-memory `Vec`
  (`gateway/session.rs:498`, `routing/interactive.rs:921`, `main.rs:1736-1751`).
  The propagation slot is dead too: `ShellState::record_disposable_choice`
  (`shell/state.rs:1216`) has zero production callers. `shell/view.rs:1793`
  already renders it when set — the consumer exists, the feed does not.
  **What would close it:** one durable write of a rationale from
  `gateway/session.rs` or `memory/extract/disposable.rs`.
- **1759** — the retrieved set is never recorded. `Extractor::run`
  (`memory/extract/mod.rs:413`) drops the local `existing` after
  `Prompt::build`; `ExtractionOutcome` has no field for it (`recorded` is what
  was *stored*, a different set). `RetrievalResult`'s only production caller
  renders to a `String` (`main.rs:2425`).
- **1760** — there is no cache-temperature signal. `grep -rn temperatur
  crates/glasshouse/src` returns nothing. `NewObservation::with_context_state`
  (`routing/evidence.rs:328`) has zero non-test callers, so `ContextState`
  reads `unknown` on 100% of real rows, and `NewObservation` has no setter at
  all for `cached_input_tokens`.
- **1766** — two links missing: 1757's absent rationale, and nothing durably
  records that a routing decision happened at all. Recomputing an explanation
  in the shell was considered and refused: it would render the factors of a
  decision that was never made.
- **1767** — nothing in this codebase computes a correlation.
  `routing/domain.rs:31` says so in the source, which is why
  `FailureDomain::Independent` is documented as never produced.

---

### Phase 47 — 1763 and 1769 re-refused in batch 50, with the missing link one layer more precise

State: **NOT STARTED** for both. These supersede the earlier OPEN entries above;
the conclusions are unchanged and the reasons are sharper.

**1763 — failure counts by class.** The classes are computed and then destroyed
before anything durable sees them. `routing::free::WorkloadOutcome`
(`routing/free.rs:173`) has three failure classes and `gateway::session::classify`
produces one per exchange, but `ResourceHealth` (`routing/free.rs:243`) keeps
only `consecutive_failures` — **reset to zero on any success**
(`routing/free.rs:292`) — and a `credential_rejected` bool with no count.
Separately, `SessionRouting::record_routing_observation`
(`gateway/session.rs:336-347`) collapses `Outcome::Unreachable` and a non-2xx
`Forwarded` into one `RoutingOutcome::Failed`.

**An orchestrator error, corrected here so it is not inherited.** On finding
that `GatewayFailure` (`events/mod.rs:308`) has three variants and that batch
50 wired `events::degrade_resource` into production for the first time (line
1735), the orchestrator recorded that 1735 "unlocks 1763". **That is wrong.**
`session::gateway_failure` (`gateway/session.rs`) maps only
`Outcome::Unreachable` → `GatewayFailure::Unreachable`; `Forwarded`,
`Unauthenticated`, `Declined`, `Unrouted`, `ClientGone` and `Idle` all return
`None`. Every `TimedOut`/`Rejected` construction in the tree sits behind a
`#[cfg(test)]` boundary (`memory/extract/lifecycle.rs:139`,
`shell/state.rs:7547`, `events/mod.rs:478`). Production emits exactly **one**
class, and counts-by-class of one class is not the capability.

**What 1763 actually needs** is therefore a product decision before any code:
*is a non-2xx `Forwarded` a gateway failure?* The gateway currently says no on
purpose — `Forwarded` means the gateway did its job and the upstream answered.
Widening that is a deliberate change to what "gateway failure" means, and the
packet that takes this line must rule on it rather than assume it.

**1769 — extraction inputs and outputs.** Memory extraction never runs in the
shell process: `Extractor::run`'s two production callers are `main.rs:1728`
(the `glasshouse hook` process) and `main.rs:2926` (`glasshouse memory
extract`), and its `ExtractionOutcome` reaches only `tracing::info!`.
`database.rs` creates seven tables and none is an extraction record. Closing it
needs a durable extraction record plus a caller change where extraction
actually runs.

**One redaction fact worth carrying to whichever packet closes it**, verified
while refusing rather than assumed: `Prompt` (`memory/extract/mod.rs:145`) is a
newtype with one constructor and no `From<String>`, and `Prompt::build` scrubs
`existing` through `credentials::scrub` on the way in. A future extraction
debug view can show a `Prompt` safely **only because of that constructor** —
the guarantee is about the `Prompt` type, not about a `SessionChunk` or a raw
reply, neither of which has an equivalent screen.

#### 1765 — the recorded limit is now CLOSED (batch 51)

The entry above records a SURVIVED mutation and names its fix: *"a pty harness
driving `glasshouse run`'s key handling, which would serve every TUI contract
in the map rather than this one line."* That harness now exists —
`crates/glasshouse/tests/tui_harness.rs` — and the mutation is **KILLED**,
re-run by the orchestrator in the integrated tree:

    state.open_route_health(build_route_health_table(runtime));
      ->  state.open_route_health(Vec::new());

    KILLED — pressing_h_draws_the_route_health_the_run_loop_read_from_disk
    tui_harness.rs:226: at 120x40 the interface opened route health and drew its
    empty state. The gateway caches on disk hold a reading for `anyrouter`, and
    `build_route_health_table` returns it — so the run loop's
    `Action::OpenRouteHealth` arm did not carry it to the view.

**It fails on the test's own named assertion, not on a fixture timeout**, and
that was designed rather than lucky: each test waits twice, and the second wait
accepts *either* the seeded value *or* the view's own empty-state sentence, so a
mutated tree reaches the `assert_eq!` instead of dying in the wait. That is
practice §80 case 5 engineered into a harness rather than discovered after it.

Three properties of the harness worth carrying to every future TUI contract:

- **The ready signal is causal, not a delay** (§38). `TerminalGuard::acquire`
  enables raw mode before touching the alternate screen, `Screen::acquire` calls
  it before building the Ratatui terminal, and `shell::run` draws after that —
  so the banner appearing *proves* raw mode is on and the event source exists.
  Nothing sleeps for a fixed period.
- **The screen is read through `vt100`, not by stripping escapes.** Ratatui
  redraws by diffing, emitting only changed cells in runs separated by cursor
  moves; stripping those concatenates unrelated rows. Assertions are per
  rendered row, which is also what makes "these two facts are on the same line"
  expressible.
- **Measured, not asserted:** 540 runs after the fix, 0 failures, 0 orphans —
  bounding a residual below ~0.55% at 95% confidence, which the report
  explicitly declines to call zero. Its own first version failed 1 run in 30 on
  a partial-frame race, and that was found by measuring rather than by passing.

`1765` was already COMPLETE; this does not change its state. It removes the one
limit that entry recorded, and it makes every other overlay dispatch arm in
`shell/mod.rs` testable for the first time.

## 1763 — CLOSED 2026-09-02: the readout landed under Phase 33C's line 1365, and this line was never revisited

The batch-50 refusal above asked for a product decision — *is a non-2xx
`Forwarded` a gateway failure?* — and `GH-FAILURE-TAXONOMY` (2026-08-31,
`phase-33c.md`) answered it for line 1365: every exchange records a nine-way
`failure_class` from status, headers and framing (never body content), and
`glasshouse resources` prints, per direct provider,
`failures 24h    cadence throttled N, quota exhausted N, provider unhealthy N —
of N exchange(s), N served` and `by class        …` — with **no total on
purpose** (`FailureClassCounts` has none, `render_failure_classes` prints none):
1365's three figures cannot be summed, which is this line's *instead of one
unexplained error percentage* clause. Nobody re-read 1763 when that landed.

**Contract.** Given routing observations recorded for a provider in the last
twenty-four hours, when `glasshouse resources` renders that provider,
Glasshouse shows its failure counts by class beside the exchanges they are out
of, while never presenting one summed failure figure or percentage.

**Production.** `provider/resources.rs::render_failure_classes`, from
`GatheredTelemetry::gather_failure_classes` over
`EvidenceLedger::failure_classes_by_provider`; caller
`main.rs::resources_report`.

**Regression** (shipped binary):
`gateway_failure_taxonomy::resources_renders_cadence_quota_and_health_as_three_figures_with_denominators`
— rows of three classes planted through the gateway, both lines asserted, and
the absence of a summed `failures 24h    4` asserted.

**Mutation** (orchestrator, `mutate.sh --allow-dirty` in a worktree at the
same source): `if !by_class.is_empty()` → `if by_class.is_empty()` (the
by-class line vanishes) → **KILLED** by that test, panicking at
`gateway_failure_taxonomy.rs:995`, the `by class` assertion.

State: **COMPLETE**. Phase 47 stands at 9 of 15. 1757, 1760, 1766, 1767 and
1769 stay Cluster H; **1759's producer now exists** — `MemoryRetrieved` rows
carry the session id from both doors since wave 94 — and the readout is
`GH-RETRIEVED-VIEW`'s (see the register).

## 1759 — CLOSED 2026-09-02 (`GH-RETRIEVED-VIEW`, Amber-light, Sonnet medium): the retrieved set is recorded, so the view is one query

The batch-50 refusal above said *the retrieved set is never recorded*. Since
wave 94 (`GH-LAUNCH-BRIEFING`) every memory delivered to a session from
either door is a `MemoryRetrieved` row carrying the session id — the
producer landed under Phase 27's contract, and this line was never revisited
(the same shape as 1763 above). This package is the reader.

**Contract.** Given `MemoryRetrieved` rows recorded with a session id, when a
user runs `glasshouse memory retrievals --session <id>`, Glasshouse lists that
session's retrieved memories newest first — memory id, the instant, the
retrieval scope, and the memory's current kind and one-line summary when the
memory still exists — bounded by `--limit`, while preserving that the
existing windowed report is byte-identical without `--session`, that a
session with no rows prints *no memory was retrieved for session <id>* rather
than an error, and that another project's rows are never read.

**Production evidence.** `evaluation/mod.rs::EvaluationObservations::retrievals_for_session`
(beside `recent_of_kind`, `WHERE kind = ?1 AND session_id = ?2 ORDER BY seq
DESC LIMIT ?3`, project-scoped as its siblings); `cli.rs::MemoryCommand::Retrievals`
gains `--session` and `--limit` (default 20); `main.rs::memory_retrievals_report`
branches on `--session` before touching the windowed readers and
`render_session_retrievals` prints one line per row, *(memory no longer
present)* for a memory `MemoryStore::get` cannot resolve. Its data today:
the launch briefing's two doors and the machine door's `deliver_memory`.

**Regression evidence** (`tests/evaluation_observations.rs`, shipped binary,
rows planted through the production `record_memory_retrieval`):
`glasshouse_memory_retrievals_session_lists_only_that_sessions_rows_newest_first`,
`glasshouse_memory_retrievals_session_marks_an_unresolved_memory_without_erroring`,
`glasshouse_memory_retrievals_session_with_no_rows_prints_a_sentence`,
`glasshouse_memory_retrievals_session_respects_limit`; the three existing
windowed-report tests, unmodified, prove the no-session path unchanged.

**Mutations** (worker, three, all KILLED, restored byte-identical):
`session-filter-dropped` (the `AND session_id = ?2` removed) — session-b's
row leaked into session-a's listing; `oldest-first` (`DESC` → `ASC`, the find
string widened to the whole statement so the file's other three orderings
stayed untouched) — the ordering assertion; `missing-memory-errors` (the
unresolved arm made a panic) — the child process panicked and the test's
exit-status assertion failed.

**Packet error, the worker's, and right:** the packet's test (b) said *a
memory deleted after retrieval*; the store has no delete, so the unresolved
row is a memory id never recorded — the same `get → None` path.

**Recorded limits.** A retrieval recorded with no session id (the CLI's own
`memory search` today) is not listed by this view; `observed_at` prints as
raw unix seconds because no person-facing instant formatter exists in the
crate (searched) — a wording follow-up, not a defect; an unknown session id
and a session with no retrievals print the same sentence, as every other
`session_id`-keyed reader treats ids.

State: **COMPLETE**. Phase 47 stands at 10 of 15; 1757 and 1766 are
`GH-ROUTE-RATIONALE-SINK`'s (live); 1760, 1767, 1769 stay Cluster H.

## 1757 and 1766 — CLOSED 2026-09-02 (`GH-ROUTE-RATIONALE-SINK`, Amber, Sonnet high): the session router's rationale on the durable sink

The batch-50 refusal above named the missing link exactly: `RoutingExplanation`
had no durable sink, and recomputing one in a view would render *the factors
of a decision that was never made*. The durable sink chosen on 2026-08-30 took
the disposable router's rationale first and claimed no box; this package puts
the **session** router's on it, at the two moments the launch already records
its decision (`design-decisions.md`, *The session router's rationale row*).

**Contract (1757).** Given a launch that routed, fresh or continued, when the
launch records its decision, Glasshouse also records the decision's
explanation as one `SessionRouteDecided` row against the session — subject
the destination id, detail the contributions as `{name, magnitude, evidence}`
— so that `sessions show <id>` lists every contribution with its signed
magnitude and evidence, while a session with no row shows `-` and no task
text reaches the row.

**Contract (1766).** Given the newest such row, when `glasshouse status`
runs, Glasshouse prints that decision's destination and its three largest
contributions by absolute magnitude on one line, ties in recorded order, or
*none recorded*.

**Production evidence.** `evaluation/mod.rs::EvaluationKind::SessionRouteDecided`;
`record_session_route` beside `record_disposable_route`, same fail-soft
shape; `EvaluationObservations::session_route_for` / `latest_session_route`;
`route_contributions` (tolerant, `[]` on anything malformed). The detail is
encoded and decoded by a hand-written encoder and char-cursor parser scoped
to that one array shape — **the worker's first packet correction, and
right**: the module's own pinning test
(`the_evaluation_module_has_no_path_out_of_the_project`, map line 1856)
forbids `serde_json` and `serde::Serialize` in the file, so the packet's
*serializable mirror* could not be serde. `main.rs::launch_session` calls
`record_session_route` after each of its two `record_routed_session` calls;
`routing_rationale_section` in `session_detail`; `last_routing_decision_line`
in `status_report`.

**Regression evidence** (`tests/route_rationale.rs`, shipped binary, the
launch fixture of `evaluation_observations.rs`, 6):
`a_fresh_launch_shows_a_rationale_route_also_names`,
`status_lists_at_most_three_factors_by_absolute_magnitude`,
`an_unrouted_session_shows_a_dash`, `a_project_with_no_launches_shows_none_recorded`,
`a_continued_launch_records_its_rationale_against_the_continued_session`,
`the_task_text_never_reaches_a_rationale_rows_detail`.

**Mutations** (worker, four, all KILLED, restored byte-identical):
`fresh-site-dropped` and `continued-site-dropped` (each producer call
removed) by the fresh and the continued test respectively — *one rationale
row per launch*; `subject-blank` by the status test — *status must name the
second launch's own destination*; `weakest-first` (the sort reversed)
**SURVIVED the worker's first test**, which checked only that the three
selected were non-increasing — a routing explanation carries several
legitimate `0.0` informational contributions, so a reversed sort can select
three ties and pass — strengthened to compare against the row's own full
list independently sorted, then KILLED (§80's reading rule, followed).

**Corrections at integration, the orchestrator's.** The worker could not
touch `database.rs` (forbidden): `EVALUATION_KINDS` and the pinning test
`every_kind_the_type_can_produce_is_one_the_schema_constant_declares` did not
know the new kind; both gained it in wave 103's integration, gated with
`--lib database` and `--lib evaluation`. The worker's third correction — a
fresh launch's destination id is a profile slug, never a session id — is
recorded as the reason test (b) compares against
`RoutingCostClassObserved`'s own detail.

**Recorded limits.** `route_contributions`'s malformed-input tolerance has no
dedicated unit test (nothing but its own producer writes the column); the
view is the CLI (1762/1764's precedent), the shell's session view does not
draw the row; macOS only.

State: **COMPLETE** for 1757 and 1766. Phase 47 stands at 12 of 15; 1760,
1767 and 1769 stay Cluster H on their own missing links (a temperature
estimate, a correlation, a durable extraction record).

## 1760, 1767 and 1769 — CLOSED 2026-09-03 (`GH-PHASE47-DEBUG-VIEWS`, Amber, Sonnet high): three refusals that outlived their blockers, and Phase 47 is complete

**Why now.** Batch 50 refused all three on missing producers. Since then: the router's prompt-cache estimate is durable on the session's `SessionRouteDecided` rationale row (wave 103) beside real `cached_input_tokens` on translated rows (1333); `route_correlations_section` prints every pair's sample size and confidence (Phase 33C's correlation package); every extraction writes a routing row and `ExtractionOutcome` reaches the one site where extraction runs. Re-reading the rows' *reasons* against the week's commits found all three.

**1760 — the cache-temperature debug view.** `sessions show <id> --debug` appends `prompt-cache estimate (1760):` — the session's newest rationale row's `prompt-cache state` contribution (magnitude and evidence verbatim, labelled the router's estimate at launch) or *no routing rationale recorded*; `EvidenceLedger::cached_share_for_session`'s reading over the session's own translated rows (`cached-input share N% over K translated exchanges (C cached of I input tokens)`) or its absence sentence; and one fixed sentence saying the share is what providers reported and the estimate preceded it. Without `--debug`, byte-identical (asserted). No temperature model was invented — the view shows the estimate the router made and the evidence beside it. Two tests; `estimate-not-read` and `share-ignores-session` KILLED (the second with a planted second session).

**1767 — the proof.** No production change: the `Measured` line already prints `{overlaps} of {sample_size} overlapping observations — correlation {confidence:.2}`, sample before confidence, and the *insufficient evidence* line names both the sample and the required floor. A structural assertion was added to the existing test; `sample-dropped` KILLED. `FailureDomain::Independent` is still never earned (`routing/domain.rs`), so no independence is ever implied from sparse data — which is the line's other half, held by construction.

**1769 — extraction diagnostics, opt-in.** `[memory] extraction_diagnostics` (default off, project over user, the `retrieval_diagnostics` shape) and `memory/extract/diagnostics.rs`: one JSON line per run appended to `<state_dir>/memory-extraction.jsonl` by `run_extraction` — trigger, model, session, commit, the activity counts, `recorded: [{id, kind}]`, `lowered`, `rejected: [{kind, reason}]` with a closed reason vocabulary built from the `Refusal` variants' field *names*, the failure word, the call's tokens and duration. Never the prompt, a body, a subject, or a rejection's payload; fail-soft. Proven through the shipped binary against a loopback model with a planted subject/body and a planted credential-shaped rejection, neither of which reaches the line; `diagnostics-on-by-default` and `body-leaks` KILLED.

**Packet errors, all right:** the reranker's diagnostics writer is `serde_json` over a `Serialize` struct (the serde pin is `evaluation/mod.rs`'s alone), so this file is serde-encoded too; `LifecycleEvent` carries no free text, so activity text cannot be planted through the real path (the two other plants were); the CLI type is `SessionCommand::Show`.

**Recorded limits** (the worker's): the percentage is whole-percent; `model` and `session_id` are carried verbatim (their producers' contracts already forbid a credential or a URL); macOS only. **Bookkeeping:** the worker's `main.rs` co-edit claim did not persist — the second worker tonight to report a vanished claim on that file; the barrier record on `main.rs` is unreliable and integration reconciles by diff, not by the record.

State: **COMPLETE** for 1760, 1767 and 1769. **Phase 47 stands at 15 of 15.**
