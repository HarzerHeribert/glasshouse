# Capability evidence — phase 21b

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21B — decision provenance, 11 of 11 (lines 844–854)

Contract: A durable decision carries why it was made, when in the project's
life, what problem it solved, the assumptions that made it reasonable, and the
evidence behind it — and a decision missing all of that is treated as weaker
than one that carries it.

State: COMPLETE

Production evidence:
- **Producer:** `PROMPT_CONTRACT` rules 11–13 name every field; `RESPONSE_SCHEMA`
  asks for them with bounds; `schema::judge` validates each — optional, trimmed,
  bounded, refused **by name** when over, and `project_phase` refused by name
  when outside the map's five. `Extractor::store_one` writes them.
- **Consumer:** `glasshouse memory search` prints every field that has one, and
  `memory/search.rs` acts on their absence. Both are surfaces a person reaches,
  and both are in this batch — the §5 test applied to eleven storage lines that
  could easily have been eleven unread columns.
- Flat columns rather than a related table, deliberately: each line holds one
  concise sentence, `NULL` means *not known* and never *none*, and Phase 21C
  needs them **separable** rather than normalised. A `memory_assumptions` table
  with a category column would have been one join on every read for no
  capability the map asks for.

**853 was treated as a behaviour, because its verb is *treat*.**
`MemoryRecord::is_lower_confidence_decision()` is `kind == Decision &&
provenance.is_thin()`, where thin is *missing rationale **and** missing
assumptions* — **and**, not **or**, because that is what the line says.
`memory::search::demote_thin_decisions` then reorders **only decisions within
one authority class**. The obvious implementation — sorting thin decisions to
the bottom — reads the line as *lower-confidence than everything*, which it
does not say and which would be a real search regression. Both qualifiers in
the sentence are load-bearing: it compares a decision **to a decision**, **of
the same authority class**. Driven against the shipped binary with three
matching memories where the thin one carried the term three times and would
have won on BM25 alone: the two `preference` decisions swapped and the
`constraint` between them did not move. One test per clause, each killed by a
mutation dropping exactly one predicate.

Known limit, recorded rather than glossed:
- Lines 845–852 say *"Store …"*, and storing plus retrieving is what they ask
  for. **Nothing yet *acts* on them** — that is Phase 21C, which does not exist.
  853 is the exception and was treated as one.

---
