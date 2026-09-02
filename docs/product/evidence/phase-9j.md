# Capability evidence — phase 9j

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 9J — Pairing identity, 9 of 9 (lines 554–562)

Contract: Given a harness, a model and a route to it, Glasshouse says what the
pairing between the harness and the model *is* — keeping who publishes the
harness, who developed the model and who serves it as three separate answers,
saying `unknown` rather than reading an attribution out of a name, and letting
a person correct the metadata in a configuration file rather than in code.

State: COMPLETE

Production evidence:
- `crates/glasshouse/src/harness/pairing.rs: classify` — the one function that
  answers the question. Pure: it imports no configuration, for the reason
  `crate::profile`'s own module documentation gives, and the caller hands it
  resolved values.
- `crates/glasshouse/src/config/pairing.rs: report` — **the production
  caller**, and the single line `main.rs`'s `Command::Pairing` arm runs. It
  resolves the layered configuration, turns every configured launch profile
  into a `PairingQuery`, asks `classify`, and renders the answers.
- `crates/glasshouse/src/cli.rs: Command::Pairing` and
  `crates/glasshouse/src/main.rs` — `glasshouse pairing [--model ID]
  [--harness ID]`, the surface a person runs.
- `crates/glasshouse/src/harness/mod.rs: HarnessAdapter::official_model_support`
  — declared, with evidence, by `claude_code`, `codex`, `antigravity` and
  `cursor`; defaulted to `Unverified` by the other three, which is "nobody read
  this harness's model list" and not "this harness supports nothing".
- `crates/glasshouse/src/config/pairing.rs: PairingConfig` — the
  `[pairing.models."<id>"]` and `[pairing.harnesses.<slug>]` tables, layered
  project-over-user per key by `EffectiveConfig::pairing_overrides`.

Line by line:
- **554** — `Pairing` stores harness vendor, model developer, model family,
  serving provider, gateway and wire protocol as six fields, none derived from
  another, and `glasshouse pairing` prints six lines.
- **555** — the developer is read only from the catalogue or a correction,
  never from `ServingRoute::provider`. Proven with a provider literally named
  `anthropic`.
- **556** — `PairingClass` is exactly the six the map names. Five are produced
  by the classifier today; **`ProtocolTranslated` is representable and
  unreachable**, because `provider::translation_available` answers `false` for
  every pair in V1. That is the map's own stance on translation rather than a
  gap here, and the classifier *asks* the seam rather than assuming it — see M9.
- **557** — vendor-native requires both halves: the vendor declares the family
  as its own **and** the developer is that vendor's organisation. The
  `Vendor`/`ModelDeveloper` comparison happens in exactly one declared table,
  `vendor_organisation`, which is empty for Cursor, OpenCode, Pi and Hermes.
- **558** — vendor-supported rests on the harness vendor's own list and is
  deliberately reachable for a model whose developer is unknown:
  `gpt-oss-120b-medium` is `vendor-supported` in Antigravity with
  `developer: unknown`.
- **559** — three axes, three types that cannot substitute for one another:
  `ProtocolFit`, `ModelBehaviourFit`, `routing::ToolSemantics`. They disagree
  in practice, and the report prints them on three lines.
- **560** — an unattributed model is `unknown` even on the harness's own wire;
  the wire is still described separately.
- **561** — a correction is a TOML table; the next run of the binary reflects
  it; no router code changes.
- **562** — official support is one array per adapter, beside every other
  declaration that harness makes, each citing the artifact it was read from.

Regression evidence (macOS, `cargo test -p glasshouse`; twelve-job local gate
below):
- `crates/glasshouse/tests/pairing.rs` — twelve tests, **every one entering
  through `config::pairing::report`** against real configuration files on a
  real filesystem, so a test cannot pass against a build whose configuration
  resolution has been deleted (§35).
- `crates/glasshouse/src/harness/pairing.rs` unit tests — twenty-one, covering
  the ladder, the three axes, the catalogue and the corrections.

Mutation evidence — twelve run, twelve killed, each named test `ok` before,
`FAILED` mutated, `ok` restored:

| id | mutation | test | result |
|---|---|---|---|
| M1 | an unattributed model answers `vendor-native` | `an_unattributed_model_is_unknown_even_on_the_harnesss_own_wire` | FAILED |
| M2 | the serving provider fills in the developer | `the_serving_provider_never_becomes_the_developer` | FAILED |
| M3 | vendor-native drops the developer half of line 557 | `a_family_name_alone_does_not_make_a_pairing_vendor_native` | FAILED |
| M4 | the protocol axis sets the model-behaviour axis | `the_three_compatibility_axes_are_answered_separately` | FAILED |
| M5 | `report` ignores `effective.pairing_overrides()` | `a_correction_in_the_configuration_file_changes_the_class_the_binary_prints` | FAILED |
| M6 | an adapter's `supported_models` declaration is ignored | `vendor_supported_stands_without_an_attributed_developer` | FAILED |
| M7 | a profile that names no model gets the publisher's own | `a_harness_default_model_is_not_the_harness_vendors_model` | FAILED |
| M8 | any declared protocol counts as native | `the_protocol_rungs_separate_native_from_compatible_from_incompatible` | FAILED |
| M9 | the translation seam is never asked | `the_classifier_asks_the_one_function_that_owns_translation` | FAILED |
| M10 | a harness support correction is ignored | `a_user_correction_can_add_official_support_a_release_has_not_shipped` | FAILED |
| M11 | a profile's provider is never resolved | `the_report_keeps_publisher_developer_and_server_apart` | FAILED |
| M12 | a behaviour correction is ignored | `a_behaviour_correction_moves_one_axis_and_only_one` | FAILED |

M5 and M11 are the §35 pair: they mutate the *call*, in the production
resolution path, rather than the callee.

Failure/isolation evidence:
- `a_correction_can_withdraw_an_attribution_glasshouse_got_wrong` — an empty
  `developer` returns a model to `unknown`. A correction that makes Glasshouse
  less certain is a correction.
- `an_unknown_harness_name_is_refused_by_name` — a `--harness` this build does
  not know is answered and the real names listed, not silently ignored.
- A `behaviour` value this build does not understand is ignored rather than
  refusing to load, the same visible-degradation rule `RoutingConfig`'s stale
  free-resource pin follows.

Platform/external evidence:
- Declarations read on 2026-08-27 from installations on the development
  machine: Claude Code 2.1.246 (`claude --help`), codex-cli 0.149.1
  (`codex --help` and the `[tui.model_availability_nux]` table Codex wrote into
  its own `~/.codex/config.toml`), Antigravity CLI 1.1.21 (`agy models`, run
  against the user's own account), Cursor CLI 2026.08.11 (`cursor-agent --help`).
- End-to-end against the shipped binary in a real pty (`script -q`, `test -t 1`
  confirmed): `glasshouse pairing` printed `pairing class: unknown` for a
  profile naming `z-ai/glm-4.6`; adding a four-line `[pairing.models]` table to
  `config.toml` and re-running printed `protocol-native`, `developer: zhipu-ai`
  and `attribution: corrected in the user configuration file`.

Missing evidence:
- `ProtocolTranslated` has no reachable case, and cannot have one until a
  translation adapter exists for a concrete protocol pair.
- `ModelBehaviourFit` is `Unverified` for every catalogued model. Nothing in
  Glasshouse observes model behaviour; Phase 33A is what would feed it, and a
  user correction is the only thing that can move it today.

---

### Phase 9J — Pairing prior and evidence, 0 of 11 (lines 566–576)

Contract: Given a fresh session, Glasshouse gives a vendor-native pairing a
positive initial routing prior, applies hard constraints first, and lets
accumulated local observations for the exact harness/profile/model/backend
combination outweigh that prior.

State: NOT STARTED — assessed, not attempted.

**No production caller exists for any of the eleven, and none can exist in this
repository today.** There are exactly two routing callers in the binary:

- `gateway/session.rs` — `InteractiveRouting`, which keeps a live gateway
  session on its assigned backend and fails over on failure. It ranks nothing.
- `main.rs:812` → `memory/extract/disposable.rs` — `DisposableRouting`, which
  prefers a free resource for one disposable job. It ranks nothing.

`grep -rn 'fn score\|Score' crates/glasshouse/src` finds **no match**: there is
no scoring function, no weighted candidate function, and nowhere a prior could
be one term of. Phase 33A (routing evidence ledger) is 0 of 15 and Phase 35B
(candidate scoring) is 0 of 25, verified against the map on 2026-08-27.

- **566, 567, 569, 570, 571, 574, 576** — wait on a **prior existing at all**,
  which means Phase 35B's scoring function. 570 and 571 additionally wait on
  Phase 33A: "reliable local observations" is that ledger, and nothing today
  records a per-pairing observation.
- **572** — "keep evidence for the same nominal model distinct across
  harnesses, gateways, quantizations, model revisions, or protocol
  translations" is an evidence-storage requirement. It is Phase 33A's tenth
  line almost verbatim and belongs there.
- **573, 575** — see below; both have a *partial* answer already shipped and
  neither is closable.

Two are worth recording as partly built, because whoever closes them should not
start from zero:

- **568** ("apply hard protocol, tool, capability, privacy, and user
  constraints before applying the pairing prior") — the ordering half is
  already structural. `provider::ProtocolCompatibleProviders` exists so that,
  in its own words, *"a later model-quality scorer must accept this type, not a
  raw provider slice, so it has no unfiltered provider set to rank"*, and
  `profile::resolve` refuses rather than substituting. What is missing is the
  prior that would come second.
- **575** ("surface the pairing class, current evidence strength, and
  contribution of the pairing prior in routing explanations") — two of its
  three terms are surfaced by `glasshouse pairing` today, with citations. The
  third cannot be, and `glasshouse pairing` is a metadata report rather than an
  explanation of a routing decision, so the line stays open on both counts.

**576 was deliberately not built.** Four preference values
(`strong`/`weak`/`off`/`pin`) are half an hour of configuration plumbing, and
they would be a field parsed and never consulted — the packet's own named
failure mode and §35's. A preference over a prior that does not exist consults
nothing.

Missing evidence:
- Phase 35B: an inspectable weighted scoring function with a production caller.
- Phase 33A: a per-pairing observation ledger, keyed the way line 572 requires.
- `PairingClass::is_vendor_native` is the seam the prior attaches to. It has a
  caller for its own sake (the report) and no routing caller, which is the
  correct state for it until 35B exists.

### Phase 9J lines 566–576 — the pairing prior is built, and closes nothing

Contract: Given several eligible harness/model candidates, when Glasshouse
ranks them, a vendor-native pairing receives a positive but soft initial prior
that reliable local observations progressively overrule — while preserving the
rule that no candidate is ever *excluded* for being cross-vendor.

State: NOT STARTED, blocked. **All eleven lines stay open. The mechanism is
built, tested and mutation-proofed; it has no caller in the shipped binary.**

**Why, precisely.** Nothing in Glasshouse ranks routing candidates at all. The
binary has exactly two routing callers, and neither compares two eligible
options:

- `routing::interactive::InteractiveRouting` keeps a session on its assigned
  backend and fails over on a real provider failure. Its own
  `on_provider_failure` returns the **first** candidate for which
  `compatible()` is `Ok`, in the caller's order.
- `routing::disposable::DisposableRouting` picks a free resource for one
  disposable job.

`on_provider_failure`'s doc comment — written before this batch and unchanged by
it — says it takes the first survivor "rather than ranking them, because ranking
backends on quality is Phase 9J's job **and not this one's**." Phase 9J has
never had anywhere to put that job. **Phase 35B (candidate scoring) is 0 of 25
and Phase 33A (routing evidence ledger) is 0 of 15; lines 566–575 rest on one or
both existing.** This is §5 applied honestly: a mechanism proven only by its own
tests does not get its box.

Built and available for the caller that does not yet exist:
- `routing/mod.rs: Contribution`, `RoutingExplanation` — a general, ordered list
  of named signed contributions with `.total()` and `.render()`. **Deliberately
  not named after pairing**: Phase 32F line 1293 needs the identical shape for
  protected quota reserve, and a pairing-specific type would have to be rebuilt.
- `routing/mod.rs: HardConstraint`, `EligibleCandidate<T>`,
  `apply_hard_constraints` — line 568's ordering as a **type-level fact**. The
  only way to obtain an `EligibleCandidate` is `apply_hard_constraints` actually
  running the caller's check; the field is private and there is no other
  constructor. A scorer that takes `&EligibleCandidate<Pairing>` cannot be
  called before the filter ran.
- `harness/pairing.rs: EvidenceKey { harness, launch_profile, model, route }` —
  line 572. Two keys differing only in `route.gateway` or `route.protocol`
  compare unequal, so the same nominal model served two ways is never one body
  of evidence.
- `config/pairing.rs: decay_factor`, `evidence_signal`,
  `native_pairing_prior_contribution`, `ObservedEvidence`, `ObservationSource`.
  The prior decays to **exactly zero** at `FULL_DECAY_OBSERVATIONS = 20`, not to
  a floor.

Regression evidence (non-production; this is why the boxes stay open):
- `tests/pairing_prior.rs`, 12 tests, and unit tests in `routing::tests`,
  `harness::pairing` and `config::pairing::tests`.
- Seven mutations run, seven killed, each reverted — including M6, which is the
  one worth recording: the worker's own first draft compared candidates at 20
  observations, where the native prior is *already fully decayed to zero*, so
  deleting the entire evidence signal would have left the comparison tests
  passing. It caught that itself, lowered the count to 5, and re-ran. **A test
  that passes for the wrong reason is the failure §41 exists to prevent, and it
  was found by the worker rather than by review.**

Missing evidence:
- A candidate-scoring caller (Phase 35B) that builds a hard-constraint check and
  ranks `EligibleCandidate`s by `RoutingExplanation::total()`.
- An `ObservationSource` implementation backed by a real ledger (Phase 33A),
  replacing `NoObservations`.
- **Line 569 was not attempted at all**, and that is stated rather than folded
  into "no caller". A warm session is `crate::session` state, and
  `routing/interactive.rs` carries a test that scans for `crate::session` and
  fails the build if it appears — the routing policy must not become a session
  of its own. The continuation is a caller-supplied `Contribution`, the same
  shape as `ObservedEvidence`, once a session-lifecycle caller exists to ground
  what "continuity evidence" means.

**The packet's hypothesis was killed, and the kill is the round's finding.** It
claimed `StayReason` / `MigrationRefusal` / `ChangeCause` / `RoutingRecord` were
close enough to a routing explanation that line 575 was mostly rendering. They
are not: they explain a **first-match search**, never a scored one, and none of
them carries a magnitude. `RoutingRecord` is a log of assignment changes that
already happened. Line 575 asks "how much did each factor weigh," which had no
home. One new small general type, and no invented scoring subsystem.


### Phase 9J after batch 36 — the caller is `InteractiveRouting`, not `DisposableRouting`

Still 0 of 11, and the reason is now **narrower and more useful** than "no
component ranks candidates."

Batch 36 built exactly that component — `DisposableRouting::choose` now scores
candidates with a real `RoutingExplanation` reached from `main.rs` — and the
pairing prior **still cannot be wired to it**. `routing-score` checked before
building, per its packet's instruction, and found a structural bar rather than a
missing value:

- `native_pairing_prior_contribution` needs a `harness::pairing::Pairing`, built
  by `classify(query: &PairingQuery, …)`.
- `PairingQuery::harness` is a **required, non-optional `IntegrationId`**.
- `IntegrationId`'s ten variants are all third-party coding harnesses a user
  launches. There is no "Glasshouse itself" variant.
- `DisposableCandidate` carries provider, model, credential and cost — **no
  harness, and it cannot honestly gain one**: a disposable job (memory
  extraction, classification, reranking) is Glasshouse's own internal call to a
  model API, never run inside one of those ten harnesses.

Constructing a `PairingQuery` for a disposable candidate would mean choosing an
`IntegrationId` that has nothing to do with the actual call — the same
"do not invent a source" rule that keeps eight other Phase 35B lines open.

**So the caller Phase 9J needs is `routing::interactive::InteractiveRouting`**,
which serves real harness-backed sessions through `gateway/session.rs` and
therefore has a harness identity to classify. That is where the next attempt
belongs.

**One thing to check before sizing that package**, because it decides whether
Phase 9J is one caller away or blocked deeper: does a bound gateway session
actually know which `IntegrationId` it is serving? `SessionRouting::bind` binds
an `Assignment`; if the harness is not on it, Phase 9J needs that carried
through first.

**This corrects the orchestrator, not the worker.** Batch 36's packet listed
lines 1540 and 1541 as closable at the disposable caller. They were not, and the
worker was asked to check rather than assume — which is why this is recorded as
a finding instead of shipped as a wrong tick.

### Phase 9J after batch 37 — eight of eleven, at the caller that exists

State: **COMPLETE** for map lines 567, 568, 570, 571, 572, 573, 574 and 575.
**NOT STARTED, blocked** for 566, 569 and 576.

Production evidence:
- `gateway/mod.rs`'s accept loop → `gateway/session.rs::observe_exchange`
  (builds `ObservedEvidenceSource`, or `NoObservations` without a ledger) →
  `routing/interactive.rs::on_provider_failure`, which now **ranks** every
  same-model survivor and then every different-model one, instead of returning
  the first found.
- `gateway::conformance::a_real_provider_failure_with_recorded_evidence_prefers_the_stronger_candidate_over_order`
  — a real `Gateway`, a real accept loop, three loopback sockets and a real
  SQLite ledger. Seeds five failed observations for one candidate and five
  succeeded for another, sends one request that gets a real `503`, and asserts
  the session lands on the evidence-favoured candidate rather than the one
  configured first.

Failure/isolation evidence — five mutations, five killed, zero survivors, with
the semantic vocabulary `assurance-economics.md` defines:
- `bypass-fallback` on `best` (ignore every score) — killed.
- `remove-guard` on the prior's call — killed at **four** layers.
- `invert-condition` (`>` → `>=`) — killed by
  `routing_policy.rs::order_dependence::…`, a **pre-existing, author-independent**
  test from `lead-route`'s adversarial suite. Stronger than a self-written kill.
- `accept-stale-state` on `resolve_harness` — killed.
- `bypass-fallback` on the accept loop's own wiring — killed, and **re-run
  independently by the integrator**: replacing `evidence_ledger.as_deref()` with
  `None` turns the real-socket test red.

**The finding is worth more than the three boxes it costs, and it is the reason
566 stays open.** Within same-model failover survivors — the only group this
caller moves a session between automatically — **the native-pairing prior can
never be the differentiator, and that is a fact about `classify`, not a wiring
gap.** `harness/pairing.rs::classify` derives `PairingClass` from
`(harness, model attribution, harness vendor)`; `route.provider`, the one thing
that varies between two same-model candidates, feeds only `protocol_fit`, which
`native_pairing_prior_contribution` never reads. Every same-model candidate
therefore gets an identical prior magnitude, and **Phase 33A's local evidence is
the only signal this caller has that can separate them.**

The prior still varies across the `OfferMigration` group — different models can
hold different classes against one harness — but that group is only *offered*,
never taken automatically. So today the prior contributes a real, correct,
inspectable number to every explanation and **never once decides a real
failover**.

Missing evidence:
- **566** asks for a positive prior *for a fresh session with little local
  evidence*. This caller is failover, not session start, and the finding above
  shows the prior cannot differentiate there. It needs `next_turn` or a session
  start, not more work here.
- **569** (warm session continuity) — not attempted.
- **576** — the user's configured `PairingPreference` and corrections **do not
  reach this caller**. Every candidate is scored against a hardcoded
  `PairingPreference::Strong` and `PairingOverrides::default()`. The patch is
  located exactly: thread an `EffectiveConfig`-derived preference into
  `profile::apply_gateway` (`profile/mod.rs:1064`) and set it alongside
  `gateway.routing().bind(...)` at `profile/mod.rs:1127`. The worker did not
  write it because `profile/mod.rs` was outside its partition, and could not
  tell whether `Resolution<'_>` already carries what is needed.

**Two judgement calls the worker referred up, and how they went.**
`FAILOVER_EVIDENCE_WINDOW_SECONDS = 7 days` is its own choice with no map
guidance — accepted as reasonable for a cadence far slower than a disposable
job's, and recorded here as provisional rather than derived. And its
`EligibleCandidate` built through a trivially-true closure is acceptable: the
real hard constraints (`compatible()`) already ran on the `Backend` before
`score_candidate` is reached, so the type is documenting an order that genuinely
held rather than manufacturing one.

### Phase 9J line 576 — the configured preference reaches the scorer

State: **COMPLETE**. Phase 9J is **nine of eleven**; 566 and 569 stay open.

`Resolution` gains a resolved pairing value that `main.rs` fills from
`EffectiveConfig`, exactly as `provider: Option<&Provider>` already works —
**`profile/**` still imports no `crate::config`**, which its module doc requires
and which is why the packet settled this shape before dispatch rather than
letting a worker discover the ban.

Production evidence: `main.rs::resolved_gateway_pairing` →
`profile::Resolution` → `apply_gateway` → `SessionRouting::set_pairing_preference`
→ `gateway/session.rs::observe_exchange` → `routing/interactive.rs::score_candidate`.

Regression evidence, three mutations at three layers, all killed:
- pure function — `on_provider_failure_reads_the_callers_preference_not_a_hardcoded_default`
  asserts `Strong` scores nonzero and `Off` exactly `0.0` for the same pairing.
- production entry — replacing `state.pairing_preference` with a literal
  `Strong` in `observe_exchange` killed
  `observe_exchange_scores_a_real_failover_against_the_configured_preference`.
- **config resolution, re-run independently by the integrator** — replacing
  `resolved_gateway_pairing`'s body with `GatewayPairing::default()` kills
  `resolved_gateway_pairing_reflects_the_users_configured_preference`.

**The ruling, because the worker referred it up and it is not obvious.** The
preference reaches the scorer, varies the prior's magnitude and the rendered
explanation — and **still cannot decide a real same-model failover**, for the
structural reason recorded above: `classify` never reads `route`, so the whole
group moves together. It *can* differ across the `OfferMigration` group, which
is what the user is shown.

Line 576 asks that users be **able to prefer** — four values, settable,
reaching the decision, inspectable. That is the bar map line 1797 (its
configuration-side twin) and line 1287 were both ticked at, the latter while its
reserve policy had no caller at all. **Ticking it here is consistent with those;
refusing it would not be.** The inertness is recorded against **566**, which is
the line that actually asks the prior to matter, and 566 stays open.

---

### Phase 9J — 566 and 569 returned PARTIAL, with a structural reason (batch 51)

State: **NOT STARTED** for both. The mechanism is built, mutation-killed and
integrated; **neither box is ticked**, and the orchestrator's ruling is that
integrating it was still right. Both parts of that need saying.

**Why they cannot close.** `harness::pairing::classify` derives `PairingClass`
from the harness, the model and the user's corrections. **It never derives it
from the route.** Every candidate set the shipped binary can construct varies
only by route: `UpstreamBackend` carries a provider, routes, a credential and a
cost and **has no model field**, and the one model arrives at
`SessionRouting::bind` from `profile.model` and is applied to every backend.

So the native-pairing prior is **constant across every candidate set Glasshouse
ranks**, and a signal constant on the set being ranked cannot change the
ranking. 566 asks for a prior that gives a native pairing an edge; there is no
axis along which it can have one. 569 is unreachable for the same reason — a
warm session cannot outweigh a prior that never tipped anything.

Separately, **a fresh session does not reach the scorer at all**: `best` has
exactly two call sites, both in `on_provider_failure`. Session start runs
`assign`, which records a backend the caller already chose.

**This corrects an attribution in this file.** The earlier entry recorded the
constancy for same-model failover survivors and attributed it to *that caller*.
It is not about the caller. It is about `classify`'s inputs versus the axis
every routing decision varies along, and it applies identically to the session
start this ledger recommended trying next.

**Why the code was integrated anyway — a deliberate, tripwired Cluster B.**
This project's most common defect is a mechanism built and never installed, and
this batch closed four of them. Adding a fifth needs a reason. The reason is
`routing::interactive::tests::the_native_pairing_prior_is_constant_across_a_real_session_start_candidate_set`,
which builds the candidate set the way `Upstream` + `bind` do and asserts every
prior is identical **and positive**. If anyone ever makes `classify` read the
route, **that test fails**, and its failure means 566 has just become reachable.

That is a refusal that maintains itself rather than one that decays in a
document — which is precisely what the refusal register was created because
prose could not do. The cost is production code that provably cannot affect a
decision today, and it is recorded here so no later reader mistakes its presence
for progress.

**One stale doc comment left deliberately.** `best`'s own comment claims a build
with no evidence source reproduces "first compatible candidate" with every
contribution `0.0`. Two later contributions falsified it: a vendor-native
candidate's prior is 1.0 under `NoObservations`, and
`failure_domain_contribution` returns −1.0 for a shared domain. Outside the two
lines; someone should correct it deliberately.

---

## 2026-09-02 — the constancy proof is scoped to `routing::interactive`, and 566/569 are packageable through the session router

The refusal above is right about the type it examined:
`routing::interactive`'s `UpstreamBackend` has no model field, so its
candidate sets cannot vary on the vendor axis, and the tripwire keeps that
true. It was then cited by `routing/session.rs`'s module header as covering
the session router, and it does not: `routing::session::Destination`'s
`Backend` carries a model resolved per launch profile
(`main.rs::destination_backend` → `session_pairing`, `main.rs:11241-11254`),
so a set with two enabled profiles of one harness can hold a `VendorNative`
destination beside a non-native one. `GH-RECON-56` found it; `phase-56.md`'s
2026-09-02 entry has the path. **566 and 569 leave Cluster E for
`routing::session` only**, packaged with 1540 and 1923 as `GH-PAIRING-PRIOR`.
Nothing here changes for `routing::interactive`; do not edit its tripwire.
