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
