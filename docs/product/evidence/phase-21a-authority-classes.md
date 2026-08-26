# Capability evidence — phase 21a-authority-classes

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21A authority classes — all seven classes, classification by authority, conservative classification, explicit promotion (lines 828–841)

Contract: Given memories of differing authority, Glasshouse stores the class,
honours it distinctly, never lets automatic extraction mint an invariant, and
lets a person promote or demote explicitly.

State: COMPLETE

Production evidence:
- `MemoryAuthority` with seven classes, each round-tripping through SQLite
  unchanged, driven from `MemoryAuthority::ALL` so an eighth class fails a test
  rather than passing unnoticed. `is_binding()` and `MemoryStore::binding()`
  honour them distinctly.
- **`glasshouse memory search` prints the class.** This is the fixed
  architectural requirement — *retrieval must preserve the distinctions instead
  of flattening all memories into equally authoritative text* — and until this
  batch the one surface a person could reach dropped `authority` on the floor.
  An unclassified memory prints `unclassified`; it does not borrow a class.
- `glasshouse memory promote <id> <authority>` sets any class including
  `invariant`, as `Classifier::Reviewed` — the person typing it is the review
  the class requires. Demotion is never refused by either classifier: 21A's
  concern is memories becoming binding without anyone deciding they should, and
  requiring review to *demote* would leave an over-confident classification in
  place.
- **An extractor may not mint an invariant, at all**, and two independent
  controls enforce it: the producer cannot construct one (`EXTRACTOR_CEILING`
  is `Constraint`) and the store will not accept one from `Classifier::Extractor`.
  The map's line reads *"avoid promoting **uncertain** memories to invariants"*,
  which sounds like a certain one could be promoted. It cannot be, and the map
  answers this itself: Phase 21K requires model confidence to be treated as a
  presentation characteristic and never as evidence, so the only certainty an
  extractor has access to is not evidence of anything.
- `disposition` is what makes *"an idea discussed enthusiastically"* checkable
  rather than hoped for: `proposed` caps authority at `idea`, so no stated
  confidence can turn a proposal into a decision. Verified in a real binary
  run, above.

Regression evidence:
- `test tests::a_memory_search_names_the_authority_class_of_every_result ... ok`
  — drives all seven classes from `MemoryAuthority::ALL` plus an unclassified
  memory. MA (drop `{authority}` from the search line) → `... FAILED` → `... ok`.
- `test tests::a_person_can_promote_a_memory_and_demote_it_again ... ok`
  — promote to `invariant`, demote to `preference`, clear to `unclassified`,
  and refuse a class that does not exist. MB (`Classifier::Extractor` instead
  of `Reviewed`) → `... FAILED` → `... ok`.
- M13 remove the extractor ceiling:
  `test memory::extract::authority::tests::no_extraction_can_produce_an_invariant ... ok`
  → `... FAILED` → `... ok`. `no_input_triple_yields_an_invariant` walks all
  7 × 3 × 3 = 63 inputs.
- M21 remove the store's refusal:
  `test an_extractor_may_not_mint_an_invariant_and_nothing_is_written ... ok`
  → `... FAILED` → `... ok`. **Killed only by a subcontractor's test.**
- M20 remove `binding()`'s `is_binding` filter:
  `test binding_returns_only_active_binding_classified_memories ... ok`
  → `... FAILED` → `... ok`. **Also killed only by a subcontractor's test.**
- M8 store the declared authority rather than the conservative one:
  `a_model_cannot_write_an_invariant_into_this_project` FAILED → ok.

Known limit, recorded rather than fixed:
- `idea`'s *"must never be injected as binding instructions"* is half-proved:
  `is_binding()` is false and `binding()` excludes it, but the **injection**
  half is Phase 27 and unbuilt, so nothing can violate it yet. That is an
  absence of risk, not evidence, and it is recorded as such.

---
