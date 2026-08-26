# Capability evidence — phase 21-credential-acceptance-condition

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21 credential acceptance condition — the extractor is never shown, and never emits, credential material

Contract: Given session activity or an already-stored memory containing a
credential, no credential reaches the model, and no memory carrying one is
stored.

State: COMPLETE

Production evidence:
- Three choke points, not one rule to remember. `SessionChunk::build` scrubs
  (so no chunk anywhere in the program holds un-scrubbed activity);
  `Prompt::build` scrubs the already-stored memories it quotes back (a row
  written before this module existed never passed a screen); `schema::judge`
  screens each emitted element **before reading any of its fields** and
  refuses it whole — so a credential in a field the contract does not even
  read is still caught.
- The two directions are deliberately asymmetric — **scrubbed in, refused
  out**. A session that printed a key still contains everything else the
  project learned that hour, so discarding the hour would lose more than it
  protects. A memory is small and discrete, so losing one costs one, and a
  *redacted* secret in a durable row still carries its neighbourhood.

Regression evidence:
- M1 drop the scrub on every entry → `the_model_is_never_shown_a_credential_from_session_activity` FAILED → ok.
- M2 drop the scrub on quoted existing memories → `the_model_is_never_shown_a_credential_from_an_already_stored_memory` FAILED → ok.
- M3 drop the output screen → `a_memory_carrying_a_credential_is_never_stored` FAILED → ok.
- M15 drop the assignment check from `screen` → `anything_scrub_removes_is_something_screen_refuses` FAILED → ok.
- **M14, the false-positive direction, and the one to point at if only one
  mattered.** Dropping the digit requirement on an assigned value makes
  `secret: memory-belongs-to-the-project` a credential, and a real memory is
  refused: `prose_that_merely_mentions_a_secret_is_not_an_assignment` FAILED →
  ok. An over-eager recognizer gets turned off, taking the protection with it,
  so this direction needs a mutation as much as the other one does.

---
