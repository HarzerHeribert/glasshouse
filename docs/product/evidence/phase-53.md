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
