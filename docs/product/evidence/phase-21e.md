# Capability evidence — phase 21E

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21E — the decision ladder, and the one box that only needed its own test

Contract: Given a project memory holding invariants, constraints, decisions,
preferences and ideas of differing validity and age, when memory is retrieved
for a task, Glasshouse ranks them by a ladder in which authority and current
validity dominate recency — so a validated current constraint outranks an older
architecture decision and an unreaffirmed idea never outranks a live invariant —
while refusing to let an automatic actor supersede a binding invariant without
review.

State: **COMPLETE** for map lines 914, 915, 916, 917, 918 and 924 — six of
twelve. **NOT STARTED** for 919, 920, 921, 922, 923 and 925, each with its
missing source named below.

**No migration.** The ladder is computed from fields Phases 21A–21D already
shipped, and 924 reuses Phase 22's `high_impact_reason` rather than a second
gate.

Production evidence:

- `memory/policy.rs::LadderRung` and `::ladder_rung`, re-exported through
  `memory/search.rs` — line 914's *inspectable* ladder. A caller asks which rung
  a record sits on and gets an answer without re-deriving it from a float; the
  blended `retrieval_weight` stays the tie-break **within** a rung.
- `memory/search.rs` — `MemoryStore::search` sorts by rung before weight, which
  is what makes the ladder reach both production doors (the
  `glasshouse memory search` CLI and `api/unix.rs::query_memory`) without either
  being edited.
- `ladder_rung`'s `!record.is_current()` check — line 915.
- its unconditional `Invariant` branch — line 916.
- its `Constraint` + `is_current` + `last_validated_at.is_some()` branch — line
  917. The validation clause is the whole of the line: an *unvalidated*
  constraint does not get the rung.
- its final `CurrentDecision` vs `StaleOrExploratory` match — line 918.
- `memory/store.rs::require_reviewed_for_high_impact` over `high_impact_reason`,
  reached by `revalidate_superseded` → `MemoryStore::supersede` — line 924.

Regression evidence:

- `memory_search.rs::a_caller_can_name_which_rung_a_search_result_sits_on_without_reading_a_float`
- `memory_search.rs::line_915_a_current_decision_outranks_a_historical_one_despite_a_much_weaker_match`
- `memory_search.rs::line_916_a_binding_invariant_outranks_a_convenience_preference_despite_a_much_weaker_match`
- `memory_search.rs::line_917_only_a_validated_current_constraint_outranks_an_older_decision`
- `memory_search.rs::line_918_an_ordinary_current_decision_outranks_a_stale_idea_despite_a_much_weaker_match`
- `memory_search.rs::recency_does_not_dominate_a_brand_new_idea_never_outranks_an_older_validated_invariant`
  — the phase's own fixed architectural requirement, tested directly.
- `memory_store.rs::an_automatic_reviewer_is_refused_a_high_impact_supersession_and_a_reviewed_one_is_not`
- `memory::policy::ladder_tests::{only_a_validated_constraint_outranks_an_ordinary_decision, a_current_decision_outranks_a_historical_one}`

**Each of 915–918 has a test that fails when only its own rule is removed.** One
test asserting a single global ordering would have "proved" four boxes at once
and proved none of them; the four are separate deliberately.

**Every line_91x test gives the lower-ranked memory the far stronger lexical
match.** That is what makes them non-vacuous: without the ladder the search's own
relevance ordering returns the opposite answer, so each test fails for the right
reason rather than passing because the desired order happened to be the default.

Failure/isolation evidence:

- Mutations, all killed: `invert-condition` (reverse the rung comparison —
  killed all four line_91x tests plus the recency test), `remove-validation`
  (drop `last_validated_at.is_some()` from the constraint rung — killed 917),
  `accept-stale-state` (neuter the `is_current` guard — killed 915),
  `remove-guard` (`require_reviewed_for_high_impact` reduced to `Ok(())` — killed
  924's test **and** the pre-existing revalidation one), `alter-boundary`
  (`high_impact_reason`'s `None` arm returning `None` — killed three tests
  across two phases).
- **Isolation needed no new test, and that is the right answer.**
  `project_isolation.rs::every_revalidation_primitive_refuses_a_memory_planted_from_another_project_and_writes_nothing`
  (Phase 21G, untouched here) already asserts both `supersede` argument
  positions refuse a foreign row **and write nothing to it**, read back at the
  raw-connection level rather than trusting the returned error. Re-run on this
  tree: passes.
- Binary run: three memories sharing the term *ledger* — a constraint and a
  decision with weak lexical matches, and an idea repeating the term seven
  times. `memory search ledger` puts the decision ahead of the idea despite the
  idea's far stronger match (918 visible live); `memory promote <id> invariant`
  then shows it on the invariant rung; `memory revalidate <id> superseded
  --automatic` is refused — *"carries invariant authority, so it may not be
  settled automatically"* — and the same call without `--automatic` succeeds.

**Line 924 was already enforced before this batch, and the box still needed
this work.** `require_reviewed_for_high_impact` and `high_impact_reason` are
unchanged from Phase 21G, and `revalidate_superseded` already called the gate.
What did not exist was a regression test entering through **supersession
itself** — the gate was proven only through `revalidate_reaffirmed`, a sibling
caller. Practice §35's rule is that a caller every test bypasses is not a proven
caller, and the same reasoning applies to a *gate* reached only by another
route: the suite could not tell whether supersession was protected or merely
adjacent to something that was. That is what the new test settles.

**The ladder changed shipped search ordering, and it broke a Phase 21B test.**
`memory_provenance.rs::thin_and_well_proven_decisions_of_different_authority_classes_keep_bm25_order`
asserted that two decisions of *different* authority classes keep whatever order
BM25 gave them. Under the ladder they do not: its pair was an authority
`Preference` (rung `StaleOrExploratory`) and an authority `Constraint` (rung
`CurrentDecision`), and **map line 918 requires exactly that reordering** —
*"place ordinary current decisions above stale preferences, hypotheses, and
ideas."* The old assertion and the new requirement cannot both hold.

**Neither the ladder nor the old contract was discarded.** The old test's real
subject is Phase 21B's *thin-decision demotion* rule and its scoping — that the
rule fires only within one authority class — which the ladder does not
contradict. It could no longer isolate that, because its pair straddled two
rungs and was being reordered by a different rule entirely. The fix holds the
rung constant: both memories now carry authorities on the same
`StaleOrExploratory` rung (`Preference` and `Idea`), so the ladder is neutral and
the thin-decision rule is once again the only thing that could reorder them. The
assertion, and what it proves, are unchanged; the test's doc comment now records
why the rung is held constant.

**It was the integrator who found this, not the worker, and that is a packet
defect.** The packet's verification commands named `memory_search`,
`memory_store`, `memory_validity`, `project_isolation` and `--lib memory` — not
`memory_provenance`. A change to *global search ordering* can break any test that
asserts an order, so scoping its verification to the memory targets whose names
matched the feature was too narrow. It failed on **both** macOS and Ubuntu in the
integration gate, which is where it should have been caught, but a wider packet
would have caught it a worker earlier and cheaper.

Gates run by the integrator on the integrated tree: recorded with the batch in
`docs/process/handoff.md`.

**Why six stay open.**

- **919** (*"treat current source code and executable tests as stronger evidence
  … than stale memory summaries"*) — needs the storage layer to read and judge
  the repository. That is map line 932, declined **four times** (828, 829, 862,
  932) on the recorded ground that a keyword heuristic refuses real memories and
  admits fake ones.
- **920, 921, 922** — all three turn on detecting that a *new requested
  implementation* conflicts with a remembered decision, and **there is no
  producer for "the requested implementation"**: nothing in this build
  represents what an agent is about to build. `flag_contradictions`
  (`memory/search.rs`) detects contradictions *between stored memories*, which
  is Phase 22 and a different input.
- **923** (auto-supersede an ordinary low-risk decision) — the inverse of 924
  and materially riskier: 924 is a refusal, 923 is an automatic write. Now
  well-specified, because 924's gate names exactly the population 923 must stay
  out of.
- **925** (*"record why a decision was superseded"*) — **needs a schema
  migration.** `database.rs:284` has `superseded_by` and no reason column. Red
  tier.

Platform/external evidence: SQLite and text only, no `#[cfg]` added.

Missing evidence:

- Nothing outstanding for 914–918 and 924.
- The six open lines each name their missing producer above.

---

## Phases 21E and 21G — batch 49 team-lead pass: 0 of 12 closed, and that is the correct result

An Opus team lead with one read-only subcontractor took twelve lines and closed
none. Every refusal names its missing producer in current source, checked this
session rather than inherited. **The package's value is the twelve rulings and
one live defect it found on the way.**

### The ruling it applied, stated so it can be argued with

> A capability is *Glasshouse's* when its **decisive input exists inside
> Glasshouse's process boundary.**

Glasshouse sees memory rows, the query text a caller sends, lifecycle events and
checkpoints. It does not see the user's source tree, the agent's plan, or the
agent's diff — and that was **verified rather than assumed**: nothing anywhere
under `crates/glasshouse/src/` reads the user's tracked source or runs their
tests. Every `read_to_string` is Glasshouse's own config or state, git object
ids, or `/proc`; `walkdir`/`glob` have no match at all.

That single check disposes of 919, 921, 943 and most of 923 — they require
reading the repository, which map line 932 already declined four times and
`memory/policy.rs:280-295` records the reason for.

### The twelve, with the link that fails

| line | verdict | missing link |
|---|---|---|
| 919 | premise-invalid | no reader of source or tests exists in the crate |
| 920 | premise-invalid | no producer for "a requested implementation". `search::contradicts` takes **two `&MemoryRecord`** — both sides are stored memories; no type represents a pending change |
| 921 | premise-invalid | agent conduct; the only Glasshouse-side claim is negative, and no test would fail if it were removed |
| 922 | premise-invalid | same missing trigger as 920, **and the offer half is only half-shipped**: `mark_conflicted` ships, but `MemoryStore::resolve_conflict` has **zero non-test callers**. Glasshouse can raise a conflict and cannot resolve one from the binary |
| 923 | premise-invalid | guard has no producer |
| 925 | **blocked — and the ledger's stated blocker is wrong** | see below |
| 943 | premise-invalid | line 932 again, **and both halves fail**: `project_metadata` holds exactly one key, `project_id`, enforced by its own test. There is no project metadata to check against |
| 944 | premise-invalid | no project-wide lifecycle phase exists. `ProjectPhase` is a fact about **one memory's provenance**, written only by extraction |
| 945 | premise-invalid | `ReviewReason::ArchitectureDrift` exists with **no constructor anywhere, including tests** — reachable only by a human typing the string |
| 946 | premise-invalid | identical: `ReviewReason::ProductionIncident`, zero constructors |
| 947 | premise-invalid | "not validated for a period" has a producer; "about to influence a high-impact change" has none. **A proxy was built and rejected** — scoping to what a `search` just returned means "about to be used", and a caller may be browsing; substituting `high_impact_reason(authority)` swaps a fact about the *memory* for the line's *high-impact change* |
| 951 | premise-invalid | presupposes automatic revalidation. `ConflictResolver::Automatic` is constructed at exactly one non-test site, behind a human-typed `--automatic` flag. There is no automatic actor to avoid work for |

**Line 925's correction.** `phase-21e.md` recorded it as *"needs a schema
migration … Red tier."* It does not. `review_reason` is already a persisted
column in `ALL_COLUMNS`, its `CHECK` constrains only the vocabulary, and
`supersede`'s `UPDATE` touches `status`, `superseded_by` and `updated_at` only —
so a reason already on the row survives. The line is smaller than recorded and
is not Red.

### The defect it found instead — project isolation on the READ side

Not one of the twelve. Found during Phase −1, inside its own expected file.

Phase 21G hardened five `UPDATE memories` statements with `AND project_id = ?N`,
on the recorded ground that a leading `self.get(id)?` guard *"is one line a
future edit can drop, and the failure is silent."* **The reads were never
covered — and a listing query has no guard to drop, because it takes no
identifier: the `WHERE` clause is the entire boundary.**

- `MemoryStore::with_status` — `SELECT … FROM memories WHERE status = ?1`, with
  **no project predicate**, and three production callers: `main.rs:2739`
  (`glasshouse memory revalidate --list`) and `shell/mod.rs:1646,1749` (the
  project-knowledge panel, whose keyboard route this very batch closed as line
  234).
- `MemoryStore::count` — whose doc said *"How many memories **this project**
  holds"* while the SQL said no such thing.

Proven in the shipped binary in both directions against a planted foreign row:
before the fix, another project's memory **body** printed verbatim; after,
`no memory is waiting for review`, with the planted row still present —
refusing to list is not deleting.

Not remotely reachable: the insert trigger blocks the normal path. This is
defence in depth against a restored backup, a hand-edited file, or a build
predating the trigger — the same threat model `project_isolation.rs` uses, and
the same one Phase 21G's integrator applied to the write side, now applied to
the side that renders.

Regression evidence: `memory_project_scope::the_review_queue_and_the_status_count_never_reach_a_memory_planted_from_another_project`.

Mutations — three, all killed, one re-run by the orchestrator. Their design is
worth keeping: `AND project_id = ?3` → `AND ?3 IS NOT NULL` rather than deleting
the clause, **because deleting it while leaving `?3` bound fails on rusqlite's
parameter count — a §80 case-4 false KILLED that proves nothing behavioural.**
Keeping the parameter count identical makes the mutation purely behavioural.
