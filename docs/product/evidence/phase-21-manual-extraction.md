# Capability evidence — phase 21-manual-extraction

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21 manual extraction — "Allow memory extraction to run manually for debugging and evaluation." (line 818)

Contract: Given a session's activity and a model reply, a person can run
extraction from the shipped binary and see what was stored, lowered, dropped
and refused.

State: COMPLETE

Production evidence:
- `glasshouse memory extract --session <id> --activity <path> --reply-from <path>`
  in `main.rs`. Everything except the model call is the production path: the
  chunk is bounded and scrubbed by `SessionChunk::build`, the reply goes
  through the same contract validation, credential screen, conservative
  classification and duplicate check, and what survives is written to the
  project's real memory store.
- Run against the shipped binary on a scratch project. Two memories in, both
  stored, and the second — declared `invariant` with `disposition: proposed` —
  reported as
  `lowered   1d35ff9d…  invariant -> idea (this was proposed and not accepted, so it is an idea and never an instruction)`.
- **`--reply-from` is a model *substitute*, not a model call, and the output
  says so on every run**: `model file (evaluation harness; no model was
  called)`. The configurable-model line above stays open, deliberately.

Regression evidence (`main.rs` unit tests, macOS):
- `test tests::a_manual_extraction_runs_the_whole_pipeline_and_says_no_model_was_called ... ok`
- MC `describe()` returns `"gpt-5.6"` instead of naming the file → `... FAILED` → `... ok`.
- MD feed the pipeline `std::iter::empty()` instead of the activity file → `... FAILED` → `... ok`.

Known limit, recorded rather than fixed:
- The orchestrator's judgment, recorded because the lead deliberately declined
  to make it: this line is closed and the neighbouring
  *"Keep memory-extraction failure non-fatal to the coding session"* is **not**,
  even though both turn on the extractor having a caller. A CLI invocation
  *is* a manual run for debugging and evaluation; it is **not** a coding
  session, and nothing is at risk when extraction fails inside it. Closing the
  second on this caller would be closing it on a caller its sentence does not
  describe.

---
