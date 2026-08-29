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

---

## Phase 21A line 862 — CLOSED 2026-08-29 (batch 48). Phase 21A is now 12/12.

Contract: Given a model proposing a memory during automatic extraction, when
Glasshouse parses it, the memory must declare which authority class it belongs
to — so that a binding requirement and a convenient implementation choice are
distinguishable — and extraction may only ever lower that declaration, never
raise it.

State: COMPLETE

**No code was written.** The mechanism ships; what was missing was somebody
checking whether the line was already satisfied. `memory/extract/authority.rs`'s
own module header ends the paragraph about its ceiling with *"That is Phase
21A's last line."* — this entry is that sentence's follow-through.

**Where the distinction actually lives.** Not in `Disposition`
(`Accepted`/`Abandoned`/`Proposed` — a lifecycle axis) but in
`MemoryAuthority`: a hard requirement is `Invariant` or `Constraint`, a
convenient implementation choice is `Preference` ("a desired direction that
must not force unnecessary complexity"), and `MemoryAuthority::is_binding()` is
a full `match` so a new class must be classified rather than defaulting to
either side.

Production evidence:
- `crates/glasshouse/src/memory/extract/schema.rs:402` — `declared_authority`
  is parsed with `required_enum("authority", ..)` and the field on the parsed
  type is `MemoryAuthority`, **not** `Option<MemoryAuthority>`. A memory that
  declines the distinction does not parse. That is the word *"Require"* in the
  line, enforced at the boundary rather than requested in a prompt.
- `crates/glasshouse/src/memory/extract/authority.rs:67` —
  `EXTRACTOR_CEILING = MemoryAuthority::Constraint`. Automatic extraction may
  assign a **binding** class, so it can express a hard requirement; it may not
  reach `Invariant`, which needs `Classifier::Reviewed` — a person. The module
  documents this as a consequence of Phase 21K's rule that model confidence is
  a presentation characteristic and not evidence, not as a tuning parameter.
- `authority::conservative` combines ceilings by taking the **weakest**, so a
  declaration may only ever be lowered.

Regression evidence:
- `memory_extract_schema::authority_disposition_support_and_confidence_are_each_required_and_validated`
  — the requirement itself.
- `memory_extract_schema::a_binding_decision_needs_rationale_and_a_non_binding_one_does_not`
  — the contract treats the two sides *differently*, which is the distinction
  being load-bearing rather than merely recorded.
- `memory_authority::is_binding_is_true_for_exactly_invariant_constraint_and_decision`
- `extract::authority::tests::a_certain_accepted_constraint_is_stored_as_a_constraint`
  and `::a_declared_invariant_is_lowered_to_a_constraint_and_says_why`
- `extract::authority::tests::ranking_and_bindingness_agree`

Mutation, run by the orchestrator:

| mutation | vocabulary | result |
|---|---|---|
| `required_enum("authority", ..)` → default a missing authority to `MemoryAuthority::Preference` instead of refusing (`schema.rs:402`) | `remove-guard` | **killed** — `authority_disposition_support_and_confidence_are_each_required_and_validated` FAILED at `memory_extract_schema.rs:404`, reporting `missing 'authority' gave Ok(Keep(ExtractedMemory { .. declared_authority: Preference .. }))` |

That failure message is the line's own failure mode in miniature: a memory that
never said whether it was a requirement or a preference, silently filed as a
preference. The `--test memory_extract_schema` target holds the killing test;
checked.

**A stale comment this entry does not fix, recorded so it is not lost.**
`memory/store.rs:90` still reads *"Nothing classifies yet, which is why the
column and this field are optional"*. `extract/mod.rs:492` classifies now. The
sentence is the same species of expired claim the batch-47 ledger sweeps hunted;
it belongs to `memory/**` and no packet owned that file this round.

Platform/external evidence: pure parsing and classification, no `#[cfg]`.
Missing: nothing for this line.
