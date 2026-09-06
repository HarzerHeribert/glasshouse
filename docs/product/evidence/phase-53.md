# Capability evidence — phase 53

Phase 53 — *criteria before adding graph storage*. Five lines (1879–1883),
and no entry here until 2026-09-02. The census is `GH-RECON-52-53` (Sonnet
high, read-only; `.agent-runtime/report-recon-52-53.md`), spot-checked by the
orchestrator against current source.

**The tree-wide fact:** no graph-database code exists anywhere in
`crates/glasshouse/src`; the phase's own *fixed architectural requirements*
(`capability-map.md:1874-1877`) defer graph storage until *concrete multi-hop
relationship queries cannot be served adequately by the existing relational
model*, and no unmet multi-hop query is recorded anywhere in
`refusal-register.md` or `design-decisions.md`.

## Censused 2026-09-02 — five lines, two causes

### 1879, 1882 — REFUSED, Cluster Q

- **1879** *"Do not add a graph database solely to visualize project memory."*
  No graph database exists. The nearest tripwire, map line 1107 (☑, Phase 25:
  *avoid rendering a decorative node graph*), guards the **UI widget**, not a
  database, and per the 616/622 ruling a tripwire would not tick this box
  regardless.
- **1882** *"Evaluate whether SQLite relations are insufficient before adopting
  a dedicated graph database."* No graph database has been adopted, so no
  evaluation has been skipped. Unlike 1866 this is answerable today — the
  evidence below (one relationship ever needed, ever built, serving a real
  query) *is* the evaluation — but *"evaluate whether X is insufficient"* is a
  ruling, not a mutation-testable fact, and writing a decision record with no
  proposal to decide is trap 1 (a document instead of a dispatch). **The
  ruling, recorded here so it is not re-derived:** SQLite relations are
  sufficient for every relationship query this build has; the day a graph
  database is proposed, `design-decisions.md` cites this entry and
  `GH-RELATIONSHIP-PROOFS`'s evidence, and 1882 closes on that record.

### 1880, 1881, 1883 — already true in production, never proven; PACKAGED `GH-RELATIONSHIP-PROOFS` (Green, tests only), dispatched 2026-09-02

The existing relational model has exactly **one** typed relationship,
`supersedes`, and it is real, constrained and read by a real query:

- `memories.superseded_by` (`database.rs:467`) with
  `CHECK (superseded_by IS NULL OR superseded_by <> id)` (`:471`),
  `CHECK (superseded_by IS NULL OR status = 'superseded')` (`:472`), an index
  (`:487-488`) and two triggers requiring the target row to exist
  (`:513-523`) — **1880**'s *explicit typed relationship in SQLite*, checked,
  indexed and trigger-enforced rather than a bare foreign key.
- Writer: `MemoryStore::supersede` / `supersede_with_reason`
  (`memory/store.rs:1367`, `:1399`); reader field `MemoryRecord::superseded_by`
  (`:657`).
- The real query it improves: `shell/mod.rs::knowledge_line` (`:2006-2012`)
  appends *"— superseded by {successor}"* to the project-knowledge view's
  line — **1881**'s *only when they improve real queries*, met for the one
  relationship that exists and correctly unmet for `affects` and
  `implemented_by`, which a word-boundary grep shows were never introduced.
- **1883** is Phase 25's already-closed territory restated: lines 1098–1107
  are all ☑, `render_project_knowledge` (`shell/view.rs:775`) renders five
  labelled sections with no node/edge glyph, pinned by
  `the_project_knowledge_view_renders_no_decorative_graph_glyphs`
  (`:3860-3891`); its doc (`:768-773`) states the finding for 1881 directly:
  supersession is *the only relationship this view has*, *said in a sentence,
  never drawn as an edge*.

The package proves these as lines: one same-crate test that a superseded
record's knowledge line names its successor (mutation: drop the branch), the
existing no-glyph test as 1883's regression evidence (mutation: draw an edge),
and the grep citation as 1881's negative half — no mutation is possible on a
relationship that was never built, and a passing test over an absent feature
proves nothing (Cluster Q's own rule). Zero production lines change.

---

## 1880, 1881, 1883 — CLOSED 2026-09-02 (`GH-RELATIONSHIP-PROOFS`, Green, Sonnet medium, tests only)

Thirty-five lines, all in `shell/mod.rs`'s `project_knowledge_tests`; zero
production lines changed. Two packet errors, both the orchestrator's: this
file did not exist in the worker's worktree (cut before the census commit
landed), and *"the five-sections test"* is five tests — all named below.
The targeted gate's first run tripped the entitlement-scrub family once
(*"the fake harness never exited"*, a different module, green alone and on
the rerun) — the same load-flaky family the wave-81 sweep is attributing.

**Phase 53 stands at 3 of 5.** 1879 and 1882 stay Cluster Q (above).

### Add explicit typed relationships in SQLite first when relationships become useful. (line 1880)

Contract: Given a memory that was superseded by another, when the project-knowledge view renders it through knowledge_line, the line names the successor in words, recorded and read back through the real MemoryStore rather than a hand-built record.

State: **COMPLETE** — ruled 2026-09-02. The relationship is written by the real store, constrained by the schema, read back through the store, and named by the real knowledge line; the mutation that drops the render is KILLED by a test that never hand-builds the record.

Production evidence:
- `crates/glasshouse/src/memory/store.rs` — `MemoryStore::supersede`
- `crates/glasshouse/src/memory/store.rs` — `MemoryStore::get`
- `crates/glasshouse/src/shell/mod.rs` — `knowledge_line`

Regression evidence:
- `shell::project_knowledge_tests::a_supersession_recorded_through_the_real_store_is_named_in_the_knowledge_line`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| shell/mod.rs::knowledge_line -- remove the `if let Some(successor) = &record.superseded_by { line.push_str(...) }` branch | `drop-relationship-render` | **killed** | `shell::project_knowledge_tests::a_supersession_recorded_through_the_real_store_is_named_in_the_knowledge_line` |

> drop-relationship-render observed: panicked at crates/glasshouse/src/shell/mod.rs:5263:9: assertion failed: line.contains("superseded by")

Recorded scope limits — stated by the worker, not discovered later:
- proves the query-layer wiring only; the render-layer path is line 1883's evidence, below

---

### Introduce relationships such as supersedes, affects, and implemented_by only when they improve real queries. (line 1881)

Contract: Given the only relationship kind ever introduced between memories is supersession, when the project-knowledge view or any production code is searched for affects/implemented_by, none exists -- and when a supersession is recorded, it is said in words, never drawn as a graph edge.

State: **COMPLETE** — ruled 2026-09-02. The positive half rides 1880's proof (supersession exists because a real query names it); the negative half is the worker's grep, quoted in its report — six prose hits, no type, variant or field — which is the only evidence an absence can have, and a passing test over an absent feature would have proven nothing.

Production evidence:
- `crates/glasshouse/src/memory/store.rs` — `MemoryStore::supersede`
- `crates/glasshouse/src/shell/mod.rs` — `knowledge_line`

Regression evidence:
- `shell::project_knowledge_tests::a_supersession_recorded_through_the_real_store_is_named_in_the_knowledge_line`
- `shell::view::tests::the_project_knowledge_view_renders_no_decorative_graph_glyphs`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| shell/mod.rs::knowledge_line -- remove the supersession branch (same mutation as 1880) | `drop-relationship-render` | **killed** | `shell::project_knowledge_tests::a_supersession_recorded_through_the_real_store_is_named_in_the_knowledge_line` |

> drop-relationship-render observed: panicked at crates/glasshouse/src/shell/mod.rs:5263:9: assertion failed: line.contains("superseded by")

Recorded scope limits — stated by the worker, not discovered later:
- the negative half (no affects/implemented_by) is proven by grep citation, not a test -- an absence has nothing to run

---

### Keep the user-facing project-knowledge view useful even if no graph database is ever added. (line 1883)

Contract: Given no graph database is ever added, when the project-knowledge view renders, it stays useful -- five labelled plain-text sections covering every kind of durable knowledge, and no node/edge glyph is ever drawn, including for the one real relationship (supersession).

State: **COMPLETE** — ruled 2026-09-02. Phase 25's five section tests and its no-glyph test are this line's regression evidence, and the `draw-an-edge` mutation shows the no-glyph test watches the one real relationship's sentence, not only decoration.

Production evidence:
- `crates/glasshouse/src/shell/view.rs` — `render_project_knowledge`
- `crates/glasshouse/src/shell/view.rs` — `push_knowledge_section`

Regression evidence:
- `shell::view::tests::the_project_knowledge_view_renders_no_decorative_graph_glyphs`
- `shell::view::tests::the_project_knowledge_view_shows_active_decisions_in_their_own_section`
- `shell::view::tests::the_project_knowledge_view_shows_known_constraints_in_their_own_section`
- `shell::view::tests::the_project_knowledge_view_shows_features_in_their_own_section`
- `shell::view::tests::the_project_knowledge_view_shows_failed_approaches_in_a_historical_section`
- `shell::view::tests::the_project_knowledge_view_shows_unresolved_todos_in_their_own_section`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| view.rs::push_knowledge_section -- append a '→' glyph to any rendered line containing "superseded by" | `draw-an-edge` | **killed** | `shell::view::tests::the_project_knowledge_view_renders_no_decorative_graph_glyphs` |

> draw-an-edge observed: panicked at crates/glasshouse/src/shell/view.rs:3888:17: map line 1107: no decorative graph glyph `→`, width 120: ...superseded by mem_01AAAAAAAAAAAAAAAAAAAAAAAA →...

Recorded scope limits — stated by the worker, not discovered later:
- proves this view specifically; says nothing about a future graph-shaped view the map does not currently require

---

---

## 1879, 1882 — CLOSED 2026-09-06 by the user's decision: adopted as standing rules

The Cluster Q refusal above is superseded; its 1882 paragraph is the evaluation this rule asks for and stays the record.

### Do not add a graph database solely to visualize project memory. (line 1879)

Contract: Given a proposal for a graph database, when its stated purpose is visualizing project memory, it is refused; map line 1107 already forbids the decorative widget and this forbids the store.

State: **COMPLETE** — ruled 2026-09-06 by the user ("i agree with you on all 7 items"), recorded in `design-decisions.md`, *Seven experiment gates adopted as standing rules*. Ticked on the decision, not on a mechanism: no embedding index or graph store exists in this build, so there is nothing to mutate and no test names it. 

Limits: a standing rule, not a verified behaviour. Any package that adds what this line governs must cite this entry in its Phase −1 and carry the rule's own test; if such a package lands without one, this box comes back off.

### Evaluate whether SQLite relations are insufficient before adopting a dedicated graph database. (line 1882)

Contract: Given a proposal for a graph database, when it is made, it first shows a real query that SQLite relations cannot serve; today's record (one relationship ever needed, built by `GH-RELATIONSHIP-PROOFS`, serving a real query) is the evaluation the proposal must overturn.

State: **COMPLETE** — ruled 2026-09-06 by the user ("i agree with you on all 7 items"), recorded in `design-decisions.md`, *Seven experiment gates adopted as standing rules*. Ticked on the decision, not on a mechanism: no embedding index or graph store exists in this build, so there is nothing to mutate and no test names it. The refusal paragraph above already recorded the evaluation; this entry makes it the standing answer.

Limits: a standing rule, not a verified behaviour. Any package that adds what this line governs must cite this entry in its Phase −1 and carry the rule's own test; if such a package lands without one, this box comes back off.

