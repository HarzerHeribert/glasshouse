# Capability evidence — phase 66

Phase 66 — Pane: a decision model beside the task model (map lines 2613–2615),
recorded 2026-09-16 from the user's steering (`design-decisions.md`, *Jev is a
classifier for Pane first; routing stays static*). Entries are bounded by the
*Decompression* ruling: the contract, the tests by name, the decisive mutation where
a decision was added, the limits, and the report by path.

**Non-negotiables every entry is checked against.** The decision model writes no
text and decides no capability: it never sets a tool's `Purity`, never adds or
removes a sandbox grant, never answers an exact-call approval, never proves a
command-lifting equivalence. A failed, slow or absent decision leaves the session
exactly as it is without one. The gateway relays the protocol byte for byte and
parses nothing of it. The key lives in the gateway's store or its environment,
never in a packet, a report or a worker.

**Gate.** `scripts/blast-radius.sh --targeted <changed files>` per package; the
GitHub sweep's cells are the platform verdict.

**Provider facts.** Every `Declared` fact about `typesafe` stays `Unverified` until
a probe with a real key is recorded here with its date and its exact response shape.
Read 2026-09-16 from docs.typesafe.ai, unverified against a live endpoint: `POST
https://api.typesafe.ai/v1/systemone`, `Authorization: Bearer`, model `jev-latest`,
body `{state, model, questions{key: {type, instructions, criteria}}}`, answers
`{noul}` / `{choice, probabilities, confidence}` / `{score, legend, confidence}`,
`usage{input_tokens, output_tokens}`.

---

## Line 2613 — the gateway carries the protocol

⟨open⟩

## Line 2614 — Pane asks typed questions

⟨open⟩

## Line 2615 — request intent, effect hold

⟨open⟩
