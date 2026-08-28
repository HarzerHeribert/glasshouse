# Capability evidence — phase 35B

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 35B — candidate scoring, wired into the one routing decision the binary actually makes

Contract: Given several eligible routing candidates for a disposable job, when
Glasshouse picks one, it scores them with an inspectable weighted function whose
every contribution is named and carries its evidence — while preserving the rule
that a candidate failing a hard constraint is never scored at all, only refused.

State: **COMPLETE** for map lines 1530, 1532, 1536, 1547, 1549, 1552, 1553 and
1554 — eight of twenty-five. **PARTIALLY VERIFIED** for 1548. **NOT STARTED**
for the other sixteen, each with its missing source named below.

Production evidence:
- `main.rs:1120` → `DisposableRouting::for_support_work(...)` →
  `memory::RoutedNoModel::new(JobKind::MemoryExtraction, …)` →
  `DisposableRouting::choose` — **the one routing decision the shipped binary
  makes today.** `choose` now builds a real `routing::RoutingExplanation` for
  the candidate it picks, and that explanation reaches
  `ExtractionModel::describe()`, the string `Extractor::run` records on every
  `ExtractionOutcome`.
- `main.rs::disposable_candidate_capacity` reads a real on-disk
  `GatewayQuotaCache` through `provider::resources::observed_capacity` — the
  same call `glasshouse resources` makes — so lines 1536 and 1549 are fed by
  live telemetry rather than a fixture.

Regression evidence:
- `routing::disposable::tests::the_winning_candidate_carries_a_named_inspectable_explanation`
- `routing::disposable::tests::real_capacity_and_reset_data_reach_the_explanation`
- `main.rs::tests::disposable_extraction_model_prefers_a_configured_free_model_and_names_the_reason`
- `main.rs::tests::disposable_extraction_model_reflects_real_cached_capacity_telemetry`
  — the last two enter through `disposable_extraction_model`, the function
  `main.rs`'s own hook path calls, and one plants a real cache file first.
- `tests/routing_score.rs` — four tests at the public-API boundary.

Failure/isolation evidence:
- **The decisive mutation, re-run independently by the integrator.** Replacing
  `choose`'s free-branch `self.score(...)` with an empty `RoutingExplanation`
  fails all four tests above — two of them in the binary target, which is the
  half that proves the shipped path rather than the library. Restored
  byte-identical.
- **A mutation that survived, and the dead mechanism it exposed.** The worker's
  first draft called `apply_hard_constraints` *and* kept a redundant
  `if !self.metered.permits_metered()` early check. Mutating the constraint
  closure to admit everything left 13/13 green: the hard-constraint call was
  **decorative**. It removed the redundant check so the metered-withheld result
  derives from the eligible set alone, re-ran the same mutation, and it now
  fails `glasshouses_own_run_refuses_a_metered_resource_without_an_opt_in`.
  Line 1532 and line 1553 rest on that being load-bearing, and it now is.

**Why seventeen stay open, grouped by the kind of missing thing.** This list is
the next packages' brief and is the more useful half of this entry.

- **No source exists yet in this build**: 1534 (context quality), 1535 and 1545
  (prompt-cache temperature — Phase 30/31, 0/15), 1537 (provider health — Phase
  33, 0/15; deliberately *not* conflated with `routing::free::FreePool`'s
  per-credential cooldown, which is a disposable-pool signal, not a
  cross-provider reliability ledger), 1538 (marginal cost — Phase 32G, 0/10),
  1539 (expected latency), 1546 (cadence — Phase 33A's ledger, built this same
  round and not yet read by anything).
- **Structurally inapplicable to this caller, which is different from
  unbuilt**: 1533 and 1551 (session affinity, switching cost — a disposable job
  has no session, and `routing/interactive.rs` carries a test that fails the
  build if `crate::session` appears in it), 1543 and 1544 (effective TTFC, tool
  rounds — `RoutedNoModel` calls no model and disposable jobs use no tools).
- **Nothing differentiates candidates on this axis**: 1531 (workload-tier fit —
  no candidate carries a capability rating; `WorkloadTier::Leaf` is the *job's*
  tier for the reserve gate, not a per-candidate score).
- **Hypothesis killed — see below**: 1540, 1541, 1542.
- **Mechanism built, caller unreachable in practice**: 1550.
- **1547 is now CLOSED** (batch 39) — failure-domain diversity. The full
  evidence is in `docs/product/evidence/phase-33c.md`, because the ranking it
  feeds is `InteractiveRouting::on_provider_failure`'s rather than the
  disposable scorer's. Note what changed against the guess recorded here: this
  entry expected `FreePool`'s per-credential allowance to be the signal, and the
  signal that actually decided it was **the provider on the candidate's own
  `Backend`** — a credential exhaustion is a *quota*-domain fact, and 1547 asks
  about the *failure* domain. Phase 33C's line 1371 exists precisely to keep
  those two apart, and conflating them here would have scored a sibling
  credential as if it were a different upstream.

**The pairing prior does not apply here, and the packet was wrong to say it
did.** The orchestrator's own design decision listed 1540/1541 as closable via
`native_pairing_prior_contribution`. The worker checked before building and
found it structurally impossible, not merely unsourced:
`PairingQuery::harness` is a required `IntegrationId`; all ten variants are
third-party coding harnesses a user launches; and `DisposableCandidate` carries
no harness because **a disposable job is Glasshouse's own internal call, never
made inside one of those harnesses**. Constructing a `PairingQuery` there would
mean choosing an `IntegrationId` unrelated to the actual call — exactly the
"do not invent a source" rule the same packet stated for every other input.

**Consequence for the round's premise, recorded because it corrects the
orchestrator.** Batch 35's finding was "eighteen boxes wait on one missing
consumer." Half of that was right — the reserve policy — and half was too
coarse: Phase 9J's eleven need `InteractiveRouting` (the gateway session), a
different caller entirely, and no amount of work on `DisposableRouting` will
reach them.

Missing evidence:
- **1550's caller.** `evaluate_reserve_spend` is now called from `choose`'s
  metered-fallback branch and mutation-proven there, but
  `main.rs::disposable_candidates` **never builds a metered candidate** — it
  iterates `free_models()` only. The gate is reachable by type and dead in the
  shipped binary. Closing it needs a product decision the worker correctly
  refused to take alone: *which* metered model a provider falls back to has no
  source in `ProviderConfig`, and inventing a "disposable fallback model" field
  decides whether a background job may spend a user's paid quota unasked.
- **1530's caveat, stated rather than left implicit.** In the free branch,
  `choose` still returns at the first available candidate in the user's own
  order — the pre-existing tested algorithm — so `score` runs on the winner, not
  eagerly on every candidate. The metered branch does score and compare all of
  them. The worker reasoned that sorting by `.total()` would reproduce the same
  winner in every case the existing tests exercise, and said plainly that it did
  not prove it mechanically. A future reader should not take it as proven.
