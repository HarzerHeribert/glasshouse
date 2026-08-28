# Capability evidence — phase 33C

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 33C — failure, quota, and route correlation

Contract: Given a gateway-backed interactive session whose provider has just
failed, when Glasshouse ranks the compatible failover candidates, it scores a
candidate sharing the failed backend's failure domain below one that does not,
and records on every assignment change which domain actually moved — while
never treating a merely-different provider as *proven* independent, and never
letting a rate-limit on one credential become a claim about another upstream.

State: **COMPLETE** for map lines 1371, 1372, 1375, 1377 and 1378 — five of
seventeen. **NOT STARTED** for the other twelve, each with its missing source
named below.

Production evidence:

- `src/routing/domain.rs::FailureDomain` — three-valued (`Shared`, `Unknown`,
  `Independent`), and `FailureDomain::between` is the **only** producer of one
  in the crate. It returns `Shared` when two backends share a provider and
  `Unknown` otherwise; it can never return `Independent`.
- `src/routing/interactive.rs::failure_domain_contribution` →
  pushed into every candidate's `RoutingExplanation` inside
  `InteractiveRouting::on_provider_failure`'s per-candidate loop, before
  `best()` sums the magnitudes and ranks. **`gateway/session.rs:426` is the
  production caller** — the gateway accept loop's own `observe_exchange`, once
  per connection after the exchange is over.
- `src/routing/interactive.rs::AssignmentChange::benefit()` → `RoutingBenefit`,
  wired into `RoutingRecord::note`'s tracing line as `benefit = %change.benefit()`,
  so every recorded failover *and* every credential rotation carries it.
- The quota domain deliberately has **no new type**: it is `routing::CredentialId`,
  whose own `PartialEq` already answers "is this the same allowance", and which
  `routing::free::FreePool` is already keyed by. A wrapper would only ever
  compare the same way.

Regression evidence:

- `routing::domain::tests::two_credentials_of_one_provider_are_two_quota_domains_and_one_failure_domain`
  — line 1371 asserted on the types themselves.
- `routing::domain::tests::a_different_provider_is_an_unknown_failure_domain_not_a_shared_one`
- `routing::domain::tests::between_can_never_construct_independent` — line 1378,
  structural.
- `routing::interactive::tests::on_provider_failure_prefers_a_different_failure_domain_over_a_shared_one`
  — the load-bearing ranking test.
- `routing::interactive::tests::a_cross_provider_candidate_is_scored_unknown_not_independence`
- `tests/routing_policy.rs::failure_domain::a_candidate_sharing_the_failed_backends_own_provider_loses_to_a_diverse_one`
  — the same claim at the public-API boundary (§35).
- `gateway::session::tests::observe_exchange_records_a_credential_rotation_as_a_different_queue_not_independent_failure_handling`
  — lines 1372 and 1377, driving a **real `429`** through
  `SessionRouting::observe_exchange`, the accept loop's own function.

Failure/isolation evidence:

- **The integrator's own mutation, and it is sharper than the two the packet
  asked for.** The worker ran `remove-guard` (delete the contribution) and
  `invert-condition` (flip its sign); both kill. Neither distinguishes "the
  contribution decides" from "the candidate order decides", because both make
  the *wrong* candidate win outright. So the integrator **neutralised** the
  constant instead — `SHARED_FAILURE_DOMAIN_PENALTY: -1.0 → 0.0` — which makes
  the two candidates tie, and `best()` prefers the first on a tie. The test
  lists the shared-domain candidate **first**, so a tie returns the wrong
  answer and the test fails. It did, in **both** targets: the lib test and
  `tests/routing_policy.rs`'s, each read from its own result line (§5).
  Restored byte-identical.
- **Line 1378's structural test was re-verified under CRLF by the integrator,
  in both directions** (§14/§15). It uses `include_str!` and searches the
  multi-line literal `"\n    pub fn "` — the exact idiom that took Windows CI
  red once — so reasoning about it was not enough. A CRLF copy of `domain.rs`
  passes all three domain tests; the same CRLF copy **with the `alter-boundary`
  mutation applied fails**, and the assertion message printed the extracted
  function body, proving the scan located the real region rather than an empty
  one. The literal survives CRLF because `\r\n    pub fn ` contains
  `\n    pub fn `. Restored byte-identical, 0 CRLF bytes confirmed.
- **A behavioral test cannot prove line 1378, and that is by design.**
  `FailureDomain::Unknown` and `FailureDomain::Independent` score identically
  (both `0.0`, both rendering "independence is not established"), because
  rewarding *any* unproven independence is precisely what 1378 forbids. So the
  `alter-boundary` mutation **survives** the behavioral test and is killed only
  by the structural one. The worker checked whether this was §41's "weak
  mutation, weak test in the same direction" and correctly concluded it is not:
  the indistinguishability is the requirement, so only a structural proof can
  ever kill this class.

Gates run by the integrator on the integrated tree, not taken from the report:
`cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
--all-features -- -D warnings` zero diagnostics; `cargo doc -p glasshouse
--no-deps` clean; routing lib 79 passed, gateway lib 74 passed,
`--test routing_policy` 25 passed, 0 failed.

**Why twelve stay open, grouped by the kind of missing thing.**

- **Needs a response body or stream framing Glasshouse cannot read** — 1364
  (the nine-way failure classification). `classify` in `gateway/session.rs`
  already distinguishes credential rejection (`401..=403`), throttle (`429`),
  upstream 5xx and unreachable, which is four of the nine. `timeout`,
  `stream abort` and `empty completion` need the response stream's framing, and
  `gateway::ingress` is structurally incapable of carrying a body by its own
  module doc — the identical blocker holding Phase 33A lines 1331–1334.
- **Needs temporal correlation across the evidence ledger** — 1370, 1373, 1374,
  1376. Phase 33A's ledger now exists and records per-exchange observations, so
  the *store* is there; what is missing is a component that measures temporally
  overlapping failures between routes and attaches a sample size. Line 1378 —
  now closed — explicitly makes this optional for V1, which is why the round
  closed the fail-closed default first rather than the analysis.
- **Needs a cadence signal that is deliberately not collected** — 1365, 1366,
  1367, 1368. `WorkloadOutcome::RateLimited { retry_after }` is constructed with
  `retry_after: None` at `gateway/session.rs:564`, with a comment saying the
  headers *are* readable but wiring `retry-after` into a routing decision is
  Phase 9H/9I's scope. Until something populates it there is no cadence to keep
  separate from long-window quota.
- **Needs a probe budget model** — 1369.
- **Needs the route-topology record** — 1377 is closed for the two categories
  this build can honestly produce; see the note below.

**Line 1377, and why it is closed with two categories rather than four.**

The map line names four: *independent capacity, independent quota, independent
failure handling, or merely a different queue onto the same upstream.*
`RoutingBenefit` produces `DifferentQueueSameUpstream` (the fourth, exactly) and
`UnconfirmedFailureDomainChange`, plus `NoChange` which `benefit()`'s match must
answer for completeness.

- **Independent quota is not missing — it is carried.** A credential change *is*
  the quota-domain change, and both producible variants encode it: a rotation
  changes the quota domain and not the failure domain, and a provider change
  changes both.
- **Independent capacity has no producer anywhere in this build.** Phase 32G
  (request-cost estimation) is 0/10 and Phase 33 (resource health) is 0/15.
- **Independent failure handling may never be asserted**, because line 1378 —
  in this same phase — forbids treating absent evidence as independence, and
  nothing here establishes it.

The honest user question (§33) is *can Glasshouse record whether a routing
change bought real resilience or merely a different queue onto the same
upstream?* It can, on every recorded change, and it refuses to claim what it
cannot establish. Two unreachable enum variants were offered by the worker and
**declined**: `FailureDomain::Independent` is unreachable-but-present because a
structural test proves nothing constructs it, which is load-bearing; a
`RoutingBenefit::IndependentCapacity` nothing constructs and no test needs would
be decoration.
