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

### Phase 35B — evidence that varies where the prior cannot (lines 1541, 1548)

Contract: Given two routing candidates that a same-vendor pairing prior scores
identically, when Glasshouse has accumulated reliable observations for one of
them, Glasshouse ranks the candidate with better observed success above the one
the prior merely favours — while discounting observations drawn from small
samples or stale windows rather than treating a single lucky turn as
established evidence.

State: **COMPLETE** for map lines 1541 and 1548. **NOT STARTED** for 1542 and
1545, both declined by the integrator against the worker's proposal for 1542 —
reasons below.

**Why this package existed at all.** Phase 9J built the pairing prior, wired it
to `InteractiveRouting::on_provider_failure`, mutation-proved it, and it was
**inert**: `harness::pairing::classify` derives `PairingClass` from `(harness,
model attribution, harness vendor)` and never reads `route`, so every same-model
candidate scores an identical prior. The *evidence* does not share that defect —
`EvidenceKey` is built per candidate carrying `candidate.model()` and `route`,
so two same-model candidates on different providers get different observations.
**The fifth link passes for evidence exactly where it fails for the prior**, and
that is the whole basis of this package.

Production evidence:

- `routing/evidence.rs::ObservedEvidenceSource::observed` now supplies the
  `reliable_observation_count` that `config/pairing.rs::decay_factor` decays
  against, from a real `EvidenceLedger`, with a staleness discount built on
  `AggregateReading::freshness` — line 1541's *real* input and line 1548's
  stale-window half.
- `config/pairing.rs::SUFFICIENT_EVIDENCE_OBSERVATIONS`, a hard sufficiency gate
  in `native_pairing_prior_contribution` — line 1548's small-sample half.
- The scoring path itself is unchanged and pre-existing:
  `InteractiveRouting::on_provider_failure` → `score_candidate` →
  `native_pairing_prior_contribution` → `RoutingExplanation::total` → `best()`.

Regression evidence:

- `routing::interactive::tests::on_provider_failure_prior_decays_as_real_recorded_evidence_accumulates`
  — a **real** `EvidenceLedger`, 5 versus 15 fresh observations, strictly smaller
  prior magnitude at 15.
- `routing::interactive::tests::on_provider_failure_discounts_a_stale_observation_window`
  — the same eight successes, ten seconds old versus two days old.
- `routing::interactive::tests::score_candidate_does_not_let_a_thin_sample_outrank_an_established_one`
- Pre-existing: `tests/pairing_prior.rs::the_prior_contribution_decays_to_zero_as_observations_accumulate`.

**The new tests use a real ledger rather than the pre-existing hand-built
`ObservationSource` doubles**, which matters here more than usual: the doubles
can express counts and rates the shipped ledger can never produce, and one of
this batch's findings is precisely that the shipped ledger's range is narrower
than the doubles suggested.

Failure/isolation evidence:

- **Inertness check** (the one that mattered): `evidence_signal` neutralised to
  an unconditional `return 0.0`. The acceptance test failed — both candidates
  tie at their equal priors and `best()` falls back to caller order, returning
  the wrong candidate. **The term is load-bearing, not decorative.**
- Mutations, all killed: `bypass-fallback`, `remove-guard`, `invert-condition`,
  `alter-boundary`, `accept-stale-state`.

**A pre-existing mechanism found inert, and not fixed here.**
`CONFIDENT_AT_OBSERVATIONS = 5` (Phase 9J) scales confidence continuously, but
`routing/evidence.rs::MIN_SAMPLE_FOR_SUMMARY = 5` means `EvidenceLedger::summarize`
**never returns a count below 5**. So every real observation is already at
maximum confidence the instant it exists: **5 real samples and 5000 real samples
score identically.** That curve is reachable only through a hand-built
`ObservationSource`. This is why 1548 needed a hard gate rather than a second
curve layered on a saturated one, and it is recorded rather than repaired
because repairing it means choosing between lowering `MIN_SAMPLE_FOR_SUMMARY`
and raising `CONFIDENT_AT_OBSERVATIONS`, which is a policy decision this package
was not scoped to make.

**Three provisional constants, named as provisional.**
`SUFFICIENT_EVIDENCE_OBSERVATIONS = 5` (matched to the two existing constants
answering the same question), `EVIDENCE_STALE_AFTER_SECONDS = 86400` (chosen
clearly shorter than `FAILOVER_EVIDENCE_WINDOW_SECONDS`'s 7 days so *stale* and
*outside the window* stay distinct concepts), `STALE_OBSERVATION_DISCOUNT = 0.5`
(a fraction, not a curve — 1548 asks staleness to count for less, not to vanish;
zeroing would silently reproduce the "no evidence" case). None is measured. Same
standing as `RETRIEVAL_WEIGHT_FLOOR`, and recorded so none is mistaken for a
constant with a derivation behind it.

**Line 1542 was proposed COMPLETE by the worker and declined by the
integrator.** The line reads *"prefer observed success **and reliability** over
same-vendor alignment"*. `ObservedEvidence::reliability` is **`None` on 100% of
real production rows** — `ObservedEvidenceSource::observed` leaves it unset
because `RoutingSummary` has no field distinct from `failure_rate` that could
honestly fill it, and the worker correctly declined to duplicate
`task_success_rate` into it (both are summed as independent terms, so copying
would double-count the one real signal). The mechanism therefore closes on
`task_success_rate` alone.

**That is the identical standard by which line 1545 was refused in the same
round**, and consistency is the point: an input that is absent or constant
across every real observation cannot support a box that names it. 1545's
`ContextState` is `Unknown` on every real row because
`NewObservation::with_context_state` has zero non-test callers; 1542's
`reliability` is `None` on every real row for the same class of reason. Closing
1542 would retire a requirement that a second, independent reliability signal
ever be observed. It needs one — not more scoring code.

**Line 1541 is closed with one narrowing recorded rather than hidden.** The line
names the *"exact harness-profile-model-backend combination"*. The evidence key
carries harness, model, provider and protocol, but **not the launch profile** —
`ObservedEvidenceSource::observed`'s own doc states *"`key`'s launch profile is
not part of the query, because nothing this ledger stores carries one."* The
line's substantive requirement — decay as reliable observations accumulate,
scoped to a combination rather than global — is implemented, production-wired
and mutation-proven against a real ledger, so the box is ticked; recording the
narrowing here keeps the remaining dimension discoverable. Adding it means a
ledger column and a migration, which no phase currently asks for.

Platform/external evidence: pure computation, no `#[cfg]` added.

Missing evidence:

- **1542** needs a reliability signal independent of `task_success_rate`.
- **1545** needs a real producer for `ContextState`.
- **1541's launch-profile dimension** needs a ledger column, and with it a
  migration.
- `CONFIDENT_AT_OBSERVATIONS`'s curve needs `MIN_SAMPLE_FOR_SUMMARY` and it to
  stop being the same number before it can discriminate.
