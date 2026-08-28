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
