# Capability evidence — phase 21D

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 21D — age decays retrieval weight, and never touches an invariant

Contract: Given a project whose memory has accumulated over months, when
Glasshouse retrieves memories for a task, an old unreaffirmed *decision* or
*idea* ranks below a fresh or recently revalidated one of the same relevance —
while preserving a genuine invariant's weight regardless of age, and preserving
every old memory's reachability through explicit history search.

State: **COMPLETE** for all nine lines (897–905).

Production evidence:
- `memory/policy.rs: retrieval_weight(authority, now, created_at, last_validated_at)`
  and `half_life_days` — the per-authority decay table. `Invariant` never
  decays; `Constraint` slowly; `Decision` at 120 days; `Idea`, `Hypothesis` and
  `Preference` at 30. **Deliberately in `policy.rs`, not inside the ranker** —
  lines 899 and 900 are a policy, and a magic number inside a sort comparator is
  not one a reader can find.
- `memory/search.rs: MemoryStore::search` — the live path `main.rs`'s
  `memory_report` runs. It now selects `bm25(memories_fts) AS relevance` over
  `overfetch_limit(limit)` candidates (5×, capped at 500), multiplies each by
  `retrieval_weight`, sorts on the product, then truncates.
  **The over-fetch is load-bearing and is the part most likely to be got wrong
  by a later edit:** with a plain `LIMIT limit`, decay could only reorder a
  fixed set — it could never *promote* a fresh high-authority memory that fell
  outside the raw top-`limit` back into the result at all.
- `memory/store.rs: MemoryStore::reaffirm(id)` — line 901. Writes
  `last_validated_at` and **nothing else**: not `updated_at`, not `status`, not
  `created_at`. Age is then measured from `last_validated_at.unwrap_or(created_at)`,
  which is lines 899, 901 and 903 in one expression.
- `created_at` (migration 4) already satisfied line 897 and was not modified.

Regression evidence:
- `tests/memory_decay.rs::an_old_idea_with_a_strong_match_ranks_below_a_fresh_invariant_with_a_weak_one`
  — **line 904, and the one a unit test cannot prove.** A year-old `idea`
  repeating the query term seven times (a maximal BM25 match) still ranks below
  a same-instant `invariant` mentioning it once in a long sentence. Real rows,
  real FTS5 ranking, real decay: the ordering assertion over two live rows the
  packet required rather than a unit test of the multiplier.
- `tests/memory_decay.rs::an_ancient_invariant_is_not_demoted_by_age_the_way_an_equally_old_decision_is` — line 898.
- `tests/memory_decay.rs::a_newer_decision_outranks_an_older_one_about_the_same_concern` — lines 899, 903.
- `tests/memory_decay.rs::a_reaffirmed_decision_outranks_an_equally_old_unreaffirmed_twin`
  — line 901, asserting both the ranking **and** that `created_at` is untouched.
- `memory::policy::decay_tests::an_idea_decays_faster_than_an_ordinary_decision` — line 900.
- `tests/memory_validity.rs::needs_review_and_conflicted_memories_stay_out_of_current_search_but_are_findable_as_history`
  — line 905, extending the pre-existing `SearchScope::Current` / `Historical`
  boundary past `Superseded` to `NeedsReview` and `Conflicted`.

Failure/isolation evidence:
- Mutation: bypassing decay weighting in `search()`'s sort (raw `bm25` only)
  killed three of the five `memory_decay.rs` tests.
- **Integrator's finding: the over-fetch had no test at all.** The worker named
  it load-bearing in its own report and then proved everything except it.
  Reducing `overfetch_limit` to the identity — decay can reorder, never promote
  — left the entire workspace suite green, because every test in the file used a
  corpus smaller than its own `limit`, where the over-fetch is invisible.
  `tests/memory_decay.rs::decay_can_promote_a_memory_that_raw_relevance_would_have_truncated_away`
  now closes it: `limit = 3` over a corpus of ten, with the memory that must win
  ranked tenth of ten on raw relevance, so it can only be returned if more than
  three rows were fetched. Re-run against the same mutation: **FAILED**, as it
  must. This is §41's rule applied to the one claim in the package that had
  prose behind it instead of a test.
- **A mutation that SURVIVED, reported rather than hidden.** Removing
  `retrieval_weight`'s early `if authority == Invariant { return 1.0 }` changed
  nothing observable, because `half_life_days(Invariant)` independently returns
  `f64::INFINITY` and `exp(-age/∞) = 1.0`. That is doubled protection, not a
  weak test: removing **both** layers at once killed the pre-existing
  `an_invariant_never_decays` (`left: 0.15000000000005242, right: 1.0`), a test
  the worker did not write. Both layers stay — the early return also states the
  invariant where a reader looks for it. **The worker reported this itself
  rather than declaring five-for-five**, which is the behaviour §41 is for.

Missing evidence / provisional:
- **`RETRIEVAL_WEIGHT_FLOOR = 0.15` is tuned, not derived.** The worker set it
  empirically against the one ordering scenario above; `0.25` was not aggressive
  enough to let a same-instant invariant with a single mention beat a fully
  decayed idea with seven. The mechanism — multiply relevance by a
  per-authority age-based weight, over-fetch before truncating — is the part
  with evidence. **The constant is a starting point and should be re-tuned
  against a real corpus**, and is recorded here so a later reader does not
  mistake it for a derived value.

Platform/external evidence:
- macOS local suite green (1750 tests, 0 failed, on the integrated tree);
  batch 35's full gate and `--windows-vm` cover this tree.
