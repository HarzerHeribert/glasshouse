# Capability evidence — phase 9i

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 9I — free-pool routing, 9 of 14 (lines 527–540)

Contract: Given free and metered resources, Glasshouse tracks their allowances
and health per credential, prefers free ones for disposable work, cools down
what keeps failing, and never spends metered capacity on its own automated runs
without an explicit opt-in.

State: COMPLETE (five lines excepted — see below)

Production evidence:
- `crates/glasshouse/src/routing/free.rs` and `routing/disposable.rs` — kept as
  a **separate policy class** from `interactive`, which is line 533 and the
  load-bearing structural requirement of the phase.
- **Keyed per credential, not per provider** (lines 537/538): two keys for the
  same router are two allowances, and one key's exhaustion is that key's limit.
  Fed by real gateway exchanges — a real `429` records `remaining: Some(0)`
  against that credential.
- Health comes from real workload, never from probes (line 534): `FreePool::observe`
  is the only mutator of health and its input is a finished exchange, so the
  quota a health check would protect is never spent checking it.
- The Settings Routing screen renders order, disable and pin (line 536), and
  renders a choice's reason through `UseReason`'s own `Display` — one spelling,
  not two.

Regression evidence:
- M9 key free-pool state per provider instead of per credential →
  `exhausting_one_key_leaves_the_other_key_of_the_same_router_alone` FAILED.
- M10 one failure is enough for a cooldown → `one_failure_is_not_a_cooldown_and_two_are` FAILED.
- M13 count a token-priced allowance down like a request pool →
  `a_token_priced_allowance_is_never_asked_how_many_requests_are_left` FAILED.
- M14 → `the_users_order_wins_and_a_disabled_resource_is_not_offered` FAILED.
- M15 a pin silently falls back when it cannot serve →
  `a_pinned_free_resource_that_cannot_serve_fails_the_job` FAILED.
- **Line 539 is an acceptance condition with the user's money behind it, and it
  has two mutations, not one.** M11 accept any opt-in value →
  `only_the_exact_opt_in_value_counts` FAILED; M12 let an automated run inherit
  `MeteredUse::Permitted` → `an_automated_run_cannot_inherit_permitted` FAILED.
  An automated run must opt in explicitly and exactly; it cannot arrive at
  permission by inheritance.

Not closable, and **four of the five share one blocker**:
- **530, 531, 532, 540** — the disposable policy class has **no production
  caller anywhere in the binary**. `DisposableRouting::choose` does exactly what
  line 530 describes and nothing calls it; `ShellState::record_disposable_choice`
  is 540's seam and nothing calls it; 532's *"only when adequate"* half is
  enforced and proven while the *"free models can back a launch profile"* half
  is unreachable because `gateway_upstream` builds every backend `Cost::Metered`.
  One `ExtractionModel` implementation that routes through this policy closes
  all four. That is one batch, not four.
- **528** — PARTIALLY VERIFIED. `Allowance` has one variant per kind with no
  shared arithmetic (M13) and the request-pool half has a real production feed;
  the token-priced half does not, because nothing reads a provider's pricing.
  Deliberately **not** solved by parsing rate-limit headers on the forwarding
  path: the gateway forwards headers without reading them, and a parser there
  would make it a reader of the payload it exists to pass through.

---

### Phase 9I — the disposable policy gets its caller (lines 530, 531, 532, 540)

Contract: Glasshouse's own bounded support work asks the router which resource
to use, prefers a free one, allows a configured free model to back a launch
profile when the protocol is adequate, and says why the resource it used was
chosen.

State: COMPLETE

Production evidence:
- `memory::extract::disposable::RoutedNoModel` — an `ExtractionModel` that asks
  `DisposableRouting::for_support_work` for a resource before doing anything.
  `main.rs::report_hook` now passes it instead of `NoExtractionModel`, so the
  policy is consulted on **every completed task** in the shipped binary.
- 532 needed a second, smaller seam: `profile::gateway_upstream` built every
  backend `Cost::Metered` because `crate::profile` may not import
  `crate::config`, where the free marking lives. A caller-supplied predicate
  closes it without adding that dependency.
- **Still no model is called, and every new caller says so in words.** Lines
  809 and 817 are untouched and remain open.

Regression evidence — including the §35 check this packet required by name:
- **Deleting the new call.** Mutating `report_hook` back to
  `|| Box::new(NoExtractionModel)` makes
  `report_hook_routes_extraction_through_disposable_extraction_model` FAIL,
  naming the missing call. Restored: `ok`. The caller cannot be removed
  silently, which is what §35 exists to check.
- Mutating `main.rs`'s gateway `free` closure to `|_| false` →
  `a_configured_free_model_backs_the_gateway_at_no_cost` FAILED with
  `cost: "metered"`. Restored: `ok`.
- Hardcoding `Cost::Metered` →
  `profile::tests::a_provider_the_caller_marks_free_backs_the_gateway_at_no_cost`
  FAILED. Restored: `ok`.
- `a_disposable_choice_cannot_become_an_interactive_assignment` holds line 533's
  separation of the two policy classes across the new caller.

---

---

## Line 531 — examined and REFUSED, 2026-08-29 (batch 49)

**531 is the only unticked line in its block** — 530, 532, 533, 534, 536, 537,
538, 539, 540 are all ☑ around it, including 540, which is its close relative
about per-credential pools. That asymmetry looks like an oversight and is not.

The section above records lines 530, 531, 532 and 540 as `State: COMPLETE` once
`RoutedNoModel` gave the disposable policy a production caller. **530, 532 and
540 were ticked on that basis and 531 was not, and 531 is right to stay open.**

The mechanism is real and is genuinely watched:
- `Allowance::RequestPool { limit, remaining, resets_at }` and
  `Allowance::TokenPriced` are separate variants with no shared arithmetic, and
  `Allowance::record` (`routing/free.rs`) early-returns unless the allowance is
  a request pool, so a pool reading cannot touch a token-priced credential.
- Mutation by the orchestrator: make `record` coerce a `TokenPriced` allowance
  into a `RequestPool` built from the reading. **KILLED** —
  `a_token_priced_allowance_is_never_asked_how_many_requests_are_left` FAILED at
  `free.rs:749`. The `--lib routing::free` target holds it; checked.

**But `declare_token_priced` has zero non-test callers.** `record_pool` has
exactly one, `free.rs:366`, inside this module's own observe path. So in the
shipped binary every credential is a request pool by default and **no
token-priced allowance is ever created**. There is nothing to track request
pools *separately from*.

That is §35 applied to a variant rather than a function: a branch no production
path can reach is, to the running program, not there. It is the same shape as
this ledger's own note on 528 — *"the token-priced half does not [have a real
production feed], because nothing reads a provider's pricing"* — and the same
shape as Phase 32F's 1289 and 1290, whose inputs are hardcoded at their only
caller.

**What would close it:** something that reads a provider's pricing and calls
`declare_token_priced`, so that a real metered credential and a real pooled one
coexist and are tracked apart. Until then the separation is structural, proven
by unit tests, and unexercised.

Recorded so the next reader does not repeat the path: mechanism present, tests
present, mutation killed — and still not closeable, because the caller check is
the one that decides.

---

## 531 — CLOSED 2026-09-02 (`GH-POOL-ALLOWANCE`): the refusal's producer and consumer both exist now

See `phase-32g.md`'s entry of the same day for the mechanism; the consumer,
`routing/session.rs::request_pool_cost`, landed in `e12c73e`.

### Track request-pool limits separately from token-priced limits when a provider exposes request quotas. (line 531)

Contract: Track request-pool limits separately from token-priced limits when a provider exposes request quotas.

State: **COMPLETE** — ruled 2026-09-02. The 2026-08-29 refusal named the missing caller and consumer exactly: nothing distinguished a request pool from a token-priced allowance and nothing consumed the distinction. Both exist now — the pool is recorded from the provider's exposed request quota, separately from the token-priced declaration the price table drives, and `request_pool_cost` behaves differently for the two. The line's *when a provider exposes request quotas* is the `Measured` check; a provider that exposes none leaves `unknown_pool()`, pinned by the third test.

Production evidence:
- `crates/glasshouse/src/main.rs` — `observed_provider_health`
- `crates/glasshouse/src/routing/free.rs` — `FreePool::record_pool (now has a production caller)`
- `crates/glasshouse/src/routing/free.rs` — `FreePool::declare_token_priced (now has a production caller)`
- `crates/glasshouse/src/routing/free.rs` — `Allowance::record (early-returns for a TokenPriced allowance; phase-9i.md's own KILLED mutation, unchanged)`

Regression evidence:
- `tests::pool_allowance_1302_531_a_measured_remaining_requests_becomes_a_request_pool_and_prices_the_term`
- `tests::pool_allowance_1302_531_a_pricing_toml_entry_with_no_quota_reading_becomes_token_priced`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| health.pool.record_pool(credential, &PoolReading { limit, remaining, resets_in }, now); -> let _ = (credential, limit, remaining, resets_in, now); | `skip-state-update` | **killed** | `tests::pool_allowance_1302_531_a_measured_remaining_requests_becomes_a_request_pool_and_prices_the_term` |
| health.pool.declare_token_priced(credential); -> let _ = credential; | `skip-state-update` | **killed** | `tests::pool_allowance_1302_531_a_pricing_toml_entry_with_no_quota_reading_becomes_token_priced` |

> skip-state-update observed: panicked at crates/glasshouse/src/main.rs:16692:17: assertion `left == right` failed: the provider's own limit, nothing derived

> skip-state-update observed: panicked at crates/glasshouse/src/main.rs:16796:9: assertion `left == right` failed: a priced pair with no quota reading must declare token-priced, never a pool

Recorded scope limits — stated by the worker, not discovered later:
- the two allowances still coexist only by never being written for the same credential in one call; nothing reconciles a provider that somehow qualified for both signals in the same pass -- the if/else-if in observed_provider_health simply prefers the measured request pool

---

---

---

## 542 — audited 2026-09-02 on `cluster-b.py`'s census: it stays ticked, with the limit that was never written down

`scripts/cluster-b.py` (its test boundary fixed the same morning, `b6b4d17`)
lists `DisposableRouting::for_glasshouses_own_run` (`routing/disposable.rs:959`)
and `MeteredUse::for_automated_run` (`:134`) — the two constructors the entry
above closed *"line 539"* on (today's **542**) — with **no production caller**,
the shape behind every wrongly ticked box this project has corrected. Checked
before ruling, because the act is authority-carrying:

- **The run the line names is the project's own test suite**, and that is
  where the callers are: `tests/routing_live.rs:74` and `:164` build the
  automated-run policy through exactly these two constructors, and
  `a_free_model_answers_through_a_real_gateway` (`#[ignore]`d, credential-
  gated) would make a real request to a real router — so the code path that
  could do the forbidden thing exists (Cluster Q's own test), and the first,
  never-ignored test in that file asserts the policy refuses a metered
  resource and names `GLASSHOUSE_ALLOW_METERED_MODELS` as the switch.
  `tests/routing_score.rs:103` is the second caller. A "no production caller"
  finding is the *expected* shape for a line whose subject is the test runs.
- **The evaluation half has no producer.** `JobKind::Evaluation` is
  constructed only in `routing/disposable.rs`'s own tests (`:2093`, `:2120`);
  nothing in the binary dispatches an evaluation job (`cli.rs` has `Doctor`
  and no evaluation command; `evaluation/mod.rs` records observations from
  `glasshouse hook` and runs nothing). Until Phase 51 gives Glasshouse an
  automated evaluation run, that half of the line is satisfied by the same
  policy with nobody to apply it.

**Ruling: 542 stays COMPLETE**, on the test-run half, with the evaluation-run
half recorded here as the limit. When Phase 51's first dispatching package
lands, its packet owes this entry a production caller of
`for_glasshouses_own_run` — that is the moment the census line above stops
being expected and starts being the finding.

### Phase 9I — the disposable policy's caller now calls (lines 530, 531, 540; `GH-ROUTED-EXTRACTION-CLIENT`, Red, Opus 5 high, 2026-09-02)

The entry above closed 530/531/532/540 on a caller that consulted the policy and then dialled nobody — `RoutedNoModel`'s own module doc said so, and `phase-33c.md`'s census of 1367 recorded that `disposable_extraction_model` returned a configured extraction model *before* the router was consulted. This package makes both true in the other direction, and the boxes stay ticked on a stronger fact.

**The design, ruled by the worker and accepted by the orchestrator.** Four steps in `main.rs::disposable_extraction_model`: *consent* — `[memory] extraction_model` set means a model may be called at all, and its absence still means route, explain, record, call nothing (the doc comment on `configured_extraction_model` is a recorded decision that a free-model list is a statement about cost, not consent for a hook to dial out; kept); *local bypass* — a provider naming no credential variable cannot be a `DisposableCandidate` (a `CredentialId` carries a `SecretRef`), so the loopback runner is built directly, and nothing is lost because a free local model satisfies 530 trivially; *the choice* — `DisposableRouting::choose` over every configured candidate **plus** the configured extraction model, which is now one candidate among the user's free ones rather than a bypass (free first when adequate, the named model as `UseReason::Fallback`, metered only as `MeteredUse` and the protected reserve permit); *the client* — `extraction_client_for`, `classification_model`'s exact shape, resolving the exact `SecretRef` the winning candidate named through `PreferNativeSecretStore` into `ConfiguredModel::new`, whose one read of the value is the `authorization` header. The label (a provider and a variable *name*) travels as `ModelCall::credential_label` into `routing_observations.quota_context`, the column `gateway::session` already uses for the same fact. Health is durable: `persist_support_work_health` adopts this resource's `GatewayHealthCache` entry into a `FreePool`, observes the `WorkloadOutcome`, and writes it back, so `consecutive_failures` accumulates across the one-second processes that dispatch; `observed_health_of` (unchanged) is the read side. The two policy classes still do not name each other — the cache reader and writer live in `main.rs`.

**The precedence change is the one behaviour a user could notice**, and it is pinned: a user with a configured extraction model *and* a configured free model now gets the free one when it is adequate (`the_routed_free_model_receives_the_request_and_the_named_one_does_not`). A user who wants exactly one model has `routing.free_resource_pin`, whose refusal is the codebase's own statement that a pin that fell back would not be a pin.

Production evidence:
- `crates/glasshouse/src/memory/extract/disposable.rs` — `RoutedModel` (`choice`, `with_client`, `observing`, `complete_observed`, `workload_outcome`)
- `crates/glasshouse/src/main.rs` — `disposable_extraction_model`, `configured_extraction_candidate`, `extraction_client_for`, `persist_support_work_health`
- `crates/glasshouse/src/memory/extract/mod.rs` — `ModelCall::credential_label` → `NewObservation::quota_context`; `memory/extract/model.rs` — `RATE_LIMITED` (the one phrase a 429 produces, so an exchange translates to `WorkloadOutcome::RateLimited` without a new `ModelError` variant)

Regression evidence (`tests/routed_extraction.rs`, through the shipped binary against fixture upstreams):
- `the_routed_free_model_receives_the_request_and_the_named_one_does_not` — free chosen, one request at the free fixture, none at the named one, the ledger row names it
- `health_learned_in_two_processes_moves_the_third_to_the_configured_model` — two 429s in two processes; the third process chooses the configured model
- `no_adequate_resource_fails_in_words_and_dials_nothing`
- `the_credential_value_reaches_the_request_and_neither_the_ledger_nor_the_output`
- unit: `a_chosen_resource_with_no_usable_client_says_which_and_why`, `a_run_that_calls_nothing_reports_no_outcome`, `an_exchange_translates_to_exactly_one_workload_outcome`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| `RoutedModel::callable`: `Some(Ok(client)) => Ok(client)` → `Some(Ok(_)) => Err(ModelError::Unavailable)` | `choose-and-call-nothing` | **killed** | `the_routed_free_model_receives_the_request_and_the_named_one_does_not` |
| `persist_support_work_health`: `cache.store(..)` → discarded | `forget-the-outcome` | **killed** | `health_learned_in_two_processes_moves_the_third_to_the_configured_model` |
| `disposable_extraction_model`: `&& configured_candidate.is_none()` → `&& true` (the pre-batch early return) | `bypass-the-router` | **killed** | `the_routed_free_model_receives_the_request_and_the_named_one_does_not` |
| the `with_client` call site: the label → the resolved value | `leak-the-credential` | **killed** | `the_credential_value_reaches_the_request_and_neither_the_ledger_nor_the_output` |

> choose-and-call-nothing observed: assertion `left == right` failed: one extraction is one model call, no more and no fewer

> forget-the-outcome observed: assertion `left == right` failed: the third dispatch must not try a resource two earlier processes found cooling down

> bypass-the-router observed: … ConfiguredModel::describe with no routing rationale at all, which is the bypassed shape

> leak-the-credential observed: assertion `left == right` failed: the row must name which allowance paid -- the label, which is two names

Gates: `routed_extraction` 4/4, `--lib memory::extract` 85/85, `--lib routing::free` 9/9, `--bin glasshouse` 79/79, thirteen existing suites green with counts quoted in the report, `blast-radius.sh --targeted` exit 0, rustdoc clean. Scope overflow, all disclosed: the rename `RoutedNoModel` → `RoutedModel` ripples into `memory/mod.rs`, five test files' doc comments and literals, and one line of `free_resource_order` in `classification_call.rs` whose failure without it is the clearest evidence the bypass is gone.

Recorded scope limits — stated by the worker, not discovered later:
- `ConfiguredModel` reads no response headers, so a provider's declared `Retry-After` never reaches the pool on this path: every 429 is `RateLimited { retry_after: None }` and gets the invented backoff (map line 1319's authoritative half stays open here).
- The durable pool carries health only; the `Allowance` half of `FreePool::observe` dies with the process.
- A carried-forward health entry is re-dated by the cache's per-file timestamp (line 1854's *stale* half is weaker for entries this producer did not observe).
- `GatewayHealthCache` now has two producers and `write_json_atomically` uses a fixed `<path>.json.writing` temporary — two writers race on the name and a killed process leaves one behind that `load_all*` reads as a second reading; `observed_health_of` handles a contradictory pair fail-safe (the resource is left unobserved). Pre-existing; a Green successor is named in the register (`GH-ATOMIC-WRITE-UNIQUE-TEMP`).
- The four tests drive `memory commit`; the hook dispatcher shares the one function and is covered by its unchanged suites.
- macOS only.

---
