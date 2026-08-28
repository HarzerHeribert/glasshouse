# Capability evidence — phase 21G

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21G — memory revalidation: the resolution path the binary already advertised

Contract: Given a project memory that has been flagged as needing review, when a
reviewer revalidates it through `glasshouse memory revalidate`, Glasshouse
records the outcome as reaffirmed, needs-review, superseded or invalidated and
makes that outcome immediately visible in what a default memory search returns —
while refusing an automatic reviewer on a high-impact memory, touching only the
memories explicitly selected, and never writing to a row belonging to another
project.

State: **COMPLETE** for map lines 948, 949 and 950 — three of nine.
**NOT STARTED** for 943, 944, 945, 946, 947, 951, each with its missing source
named below.

**No migration.** Every outcome line 949 names already existed as a
`MemoryStatus` variant (`Active`, `NeedsReview`, `Superseded`, `Invalidated`),
and the reviewer gate reuses Phase 22's `high_impact_reason` rather than
inventing a second one. The schema is untouched.

**The gap this closed was one the shipped binary advertised and could not
honour.** `main.rs::memory_challenge` has always printed *"it will not be
returned as current until the challenge is resolved"*, and before this batch
there was no way to resolve it: `MemoryStore::reaffirm`, `::supersede`,
`::set_status` and `::with_status` all had **zero non-test callers**, confirmed
with `scripts/discover.py --seam` before dispatch. Phase 21F created
`NeedsReview` memories through `glasshouse memory challenge` and gave them no
exit.

Production evidence:

- `main.rs::memory_revalidate` over `MemoryStore::revalidate_reaffirmed` /
  `revalidate_needs_review` / `revalidate_superseded` / `revalidate_invalidated`
  (`memory/store.rs`), reached from `cli.rs`'s `MemoryCommand::Revalidate`
  through `main.rs`'s command match — `glasshouse memory revalidate <id>
  <outcome>` — line 949. `reaffirmed` is deliberately two store calls,
  `reaffirm` then `set_status(Active)`: `reaffirm`'s own doc keeps
  `last_validated_at` and lifecycle status separate, and the command is the
  review that resolves both.
- `MemoryStore::require_reviewed_for_high_impact`, called first by all four
  `revalidate_*` methods and reusing `resolve_conflict`'s existing
  `high_impact_reason` gate — line 948. An automatic reviewer is refused on a
  binding authority **and on an unclassified one**; unclassified counting as
  high-impact is the conservative direction Phase 21A requires and was not
  weakened.
- `main.rs::memory_revalidate_list` over `MemoryStore::with_status` — line 950.
  `glasshouse memory revalidate --list [--limit N]` is the bounded selection
  half, and it gave `with_status` its first production caller.

Regression evidence:

- `main.rs::tests::a_challenged_memory_is_reaffirmed_back_into_default_search_with_a_fresh_validation`
  — the round trip, and the load-bearing test.
- `memory_store.rs::every_revalidation_outcome_leaves_the_matching_status` — all
  four outcomes, plus the successor `superseded` records.
- `memory_store.rs::an_automatic_reviewer_is_refused_a_high_impact_revalidation_and_a_reviewed_one_is_not`
  — line 948 in both directions, including the unclassified case.
- `main.rs::tests::revalidate_list_is_bounded_to_needs_review_memories_and_touches_nothing`
  — `--limit 2` over five memories in three statuses returns exactly two, both
  `NeedsReview`, and every memory's status is asserted unchanged afterwards.
- `project_isolation.rs::every_revalidation_primitive_refuses_a_memory_planted_from_another_project_and_writes_nothing`.

Failure/isolation evidence:

- **Defence in depth on five `UPDATE memories` statements, and the integrator
  re-ran the decisive check in both directions.** `supersede`, `set_status`,
  `mark_for_review`, `reaffirm` and `set_authority` each wrote
  `WHERE id = ?1` with no `project_id`, relying entirely on a leading
  `self.get(id)?`. All five guards were present — verified by reading each one;
  **there was no live defect** — but the guard is one line a future edit can
  drop, and the failure is silent: the *trailing* `self.get(id)` re-checks scope
  after the write, so the call still returns a correct-looking error while the
  foreign row has already been flipped. Each statement now also carries
  `AND project_id = ?N`, and **every leading guard was kept**.
- **Mutation, run by the integrator on the integrated tree, both ways:**
  removing `mark_for_review`'s leading guard **alone** leaves
  `every_revalidation_primitive_refuses_a_memory_planted_from_another_project_and_writes_nothing`
  **passing** — the scoped `WHERE` now excludes the foreign row on its own.
  Removing the guard **and** the `AND project_id = ?6` together makes it
  **FAIL**, which is what proves the test is not vacuous. Restored
  byte-identically (`diff -q` against a pre-mutation copy) and re-run green.
- Worker mutations, all killed: `skip-state-update` (reaffirm without the status
  change), `remove-validation` (the high-impact gate made unconditionally `Ok`),
  `alter-boundary` (unclassified treated as not high-impact),
  `accept-stale-state` (`Invalidated` counted as current).
- **An error message corrected at integration.** `MemoryStoreError::ReviewRequired`
  read *"so its **conflict** may not be resolved automatically"* — accurate for
  Phase 22's caller and misleading for this one, where no conflict exists. The
  worker reused the variant exactly as instructed and escalated the wording
  rather than redesigning it, which was the right call. Generalised to *"so it
  may not be settled automatically"*, which is accurate for both callers, and
  confirmed in the running binary.

Binary run, by the integrator against a scratch SQLite-backed project, the whole
story the binary previously could not finish: `memory search falcon` (present) →
`memory challenge <id> production_incident` → `memory search falcon` (*"No
current memories match"*) → `memory revalidate --list` (the queue, with its
reason) → `memory revalidate <id> reaffirmed --automatic` → refused, *"memory
`<id>` carries decision authority, so it may not be settled automatically; a
person or a stronger agent has to decide"* → `memory revalidate <id> reaffirmed`
→ *"is now active"* → `memory search falcon` (back) → `memory revalidate --list`
(*"no memory is waiting for review"*). An unknown outcome is refused by naming
the four valid ones.

Gates run by the integrator on the integrated tree: `scripts/ci-local.sh`
**13/13**, including the ubuntu clippy leg; 3723 tests passed, 0 failed; zero
slow-test warnings; `cargo fmt --all -- --check`, clippy `-D warnings` and
`cargo doc` all clean.

**Why six stay open.**

- **943** (*"a lightweight revalidation operation that checks selected memories
  against current repository state"*) — the operation exists; the second half
  does not, deliberately. Inferring from the repository whether a memory is
  still true is map line 932, which this project has declined **four times**
  (828, 829, 862, 932) on the recorded ground that a keyword heuristic *refuses
  real memories and admits fake ones*. The command presents what the memory
  itself records — validity conditions, invalidation conditions, provenance,
  `last_validated_at` — and lets a person or a stronger agent decide, which is
  also what line 948 asks for. Closing 943 needs a source of repository truth
  this project has repeatedly judged unavailable, not more code.
- **944, 945, 946** (revalidation runs at a lifecycle-phase change, before a
  refactor, after an incident) — the worker argued these both ways, as
  instructed, and the argument against wins. The map's verb is *"allow
  revalidation to **run**"*, and **none of the three triggering events has a
  producer**: `ProjectPhase` is a per-memory recorded field, not a project-wide
  current-phase signal; nothing detects that a refactor is starting; nothing
  reports an incident. A human can already revalidate by hand for any of these
  reasons, which is real but is a different claim from the one the line makes.
  Leave open pending a producer for at least one event.
- **947** — `last_validated_at` gives the *"has not been validated for a
  configurable period"* half; *"and is about to influence a high-impact change"*
  has no producer, because nothing in this build knows what change is about to
  happen.
- **951** (*"avoid automatic revalidation work when the memory is not about to
  affect any current task"*) — **the worker proposed this closed and the
  integrator declined it.** Its only evidence is that no sweep code exists,
  which the report itself describes as *"a structural/negative check, confirmed
  by reading the diff rather than a runtime test."* By this project's own rule a
  regression test counts only when it *would fail if the required behaviour were
  removed*, and nothing here would. The line also presupposes automatic
  revalidation, which does not exist; this is the same shape as map line 1748,
  which was un-ticked for exactly that reason. It becomes closable when
  something revalidates automatically and can be shown not to run.

Platform/external evidence: SQLite and text only, no `#[cfg]` added. Covered by
the macOS and Ubuntu legs above; Windows run recorded in the handoff.

Missing evidence:

- Nothing outstanding for 948, 949 and 950.
- The six open lines each name their missing producer above.
