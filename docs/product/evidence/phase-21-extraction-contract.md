# Capability evidence — phase 21-extraction-contract

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21 extraction contract — "Define a structured JSON schema…", "Feed the extractor bounded session/event chunks…", "Require the extractor to classify every emitted memory into one supported memory kind.", "Require the extractor to distinguish failed approaches from accepted decisions.", "Require the extractor to avoid duplicating an existing active memory when nothing materially changed."

Contract: Given a model reply, Glasshouse admits only elements that satisfy a
declared schema, refuses the rest by name, and never stores a memory the
project already holds.

State: COMPLETE

Production evidence:
- `memory/extract/schema.rs` — `RESPONSE_SCHEMA` plus a parser enforcing eight
  refusal rules. `the_response_schema_names_every_value_the_parser_accepts`
  pins the schema against `MemoryKind::ALL` and `MemoryAuthority::ALL`, so a
  class added to the store without being added to the prompt fails a test
  rather than silently never being asked for.
- `memory/extract/chunk.rs` — `SessionChunk::build` is the only constructor and
  applies three caps. The load-bearing one is the whole-chunk cap: a thousand
  entries each just under the per-entry cap is an unbounded history assembled
  out of bounded parts, which is exactly what the map's line forbids.
- Failed-versus-accepted is enforced as a consistency rule with teeth:
  `disposition: abandoned` ⟺ `kind: failed_attempt`, and any other pairing is
  refused as `ConflatedDisposition` rather than reclassified. Guessing which
  half a confused element meant would put Glasshouse's judgment behind the
  model's confusion.
- Duplicate detection normalizes case, whitespace runs and trailing sentence
  punctuation, against every active memory in the project *and* against what
  the run has already added. Deliberately nothing subtler: stemming would start
  deciding two different statements are the same, and a duplicate check that
  silently discards a real memory is worse than one that stores a near-duplicate.
- Reached from the shipped binary by `glasshouse memory extract`.

Regression evidence (mutation-proven, all run by the lead in a private
`CARGO_TARGET_DIR`, restored and verified with `diff -q`):
- M12 default an unknown `kind` to `finding`:
  `test memory::extract::schema::tests::every_memory_must_name_a_supported_kind ... ok`
  → `... FAILED` → `... ok`.
- M4 delete the whole-chunk character cap:
  `test a_whole_session_history_cannot_reach_the_model ... ok` → `... FAILED` → `... ok`.
- M9 delete the conflated-disposition refusal:
  `test memory::extract::schema::tests::an_abandoned_approach_cannot_be_filed_as_a_decision ... ok`
  → `... FAILED` → `... ok`.
- M7, M17, M18 (delete the duplicate branch; drop `to_lowercase`; drop the
  whitespace collapse) — all killed by
  `a_memory_the_project_already_holds_is_not_stored_again` and
  `a_reformatted_duplicate_is_still_a_duplicate`.
- M22/M23 make `memories` defaultable again — killed. The absent-key case is
  not pedantry: `extract_json_object` takes the first `{` wherever it sits, so
  a reply wrapped in an array had its inner object read as the whole envelope,
  found no `memories` key, defaulted to empty, and reported **"found nothing"
  with no failure at all** — indistinguishable from a model that looked and
  found nothing. Found by a subcontractor probing envelope shapes.

Failure/isolation evidence:
- Every refusal is per element, so one unreadable memory never discards the
  readable ones beside it. `Rejection::Store` renders a message rather than
  carrying the error, because the memory's text was screened before that point.

Known limit, recorded rather than fixed:
- **M19 survived and the filter was kept.** Replacing `WHERE project_id = ?1`
  with `WHERE ?1 IS NOT NULL` in the duplicate query kills no test: project
  isolation here is *structural*, since every project has its own database
  file. The filter is defence in depth against a future where one file holds
  two projects. This is the second independent lead to report this same
  survivor in this module. If a third does, the right answer is one test
  asserting the *structure* — that no two projects share a file — rather than
  three more survivors.

---
