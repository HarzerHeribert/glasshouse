# Capability evidence — phase 32G

Phase 32G — *provider-aware request-cost estimation*. Ten lines, and until
2026-09-01 all ten were unreachable for one reason: **there was no price data
anywhere in this build.** The census — which line is blocked by what, read
rather than assumed — is in `docs/process/refusal-register.md` under
*"Phase 32G — the census"*.

# Lines 1305, 1306 — mechanism landed, both boxes HELD OPEN (superseded below, same day)

Package `GH-PRICING-CHANNEL` (Sonnet, high, Amber; batch 76). §83's
*"attack the channel, not the lines waiting on it"*: the eight other lines
each need an estimate, and no estimate is possible without a price source.

**What shipped.** `provider/pricing.rs` — `PriceTable` with
`load_from_dir(dir)` reading `pricing.toml` (`PRICING_FILE_NAME`) out of the
runtime's config directory, `parse` rejecting negative, non-finite and
oversized values and oversized documents, and `price_for(provider, model)`.
`routing/session.rs` gains `SessionRouter::with_price_table`, defaulting to
`PriceTable::empty()` — what every candidate saw before this package — and
`expected_marginal_cost` now distinguishes three cases instead of two: a
**free** destination stays a known zero, a **metered** destination with a
price is priced from it, and a metered destination with **no entry** carries
a nonzero magnitude and says *unknown* in its evidence. A reader of the
routing explanation can tell *"costs nothing"* from *"nobody knows what this
costs"*.

**A production caller, added by the orchestrator at review.** The worker was
forbidden `main.rs` (two other workers held it), and said so plainly in its
own limits: *"no production caller wires `PriceTable::load_from_dir` into
main.rs yet."* `scripts/cluster-b.py`'s shape — a mechanism built, tested and
never installed — is behind **all ten** wrongly-ticked boxes in this
project's history, so the check was run before the ruling and confirmed it:
every reference to `PriceTable` lived inside the two new files or their own
doc comments. The wiring went into `session_router` (`main.rs`), the one
function all three ranking paths already share and whose doc comment says
so, so the path that acts and the path that reports read the same file.

**Why both boxes are HELD OPEN anyway.** The chain is complete on paper —
producer, production caller, propagation, consumer, tests, mutations — but
**nobody has yet watched a user's `pricing.toml` change what the shipped
binary prints.** The worker's own limit says its proof stops at the
`SessionRouter` public API (the boundary `interactive_score_terms` uses), and
the orchestrator's attempt at a live `glasshouse route --task` run produced
no ranked candidate for an unrelated profile-configuration reason and was
abandoned rather than dug into. Two independent signals of the same gap.
This project has ten precedents for ticking a box whose mechanism was real
and whose reachability was not, and every one was found by a later audit.
**One shipped-binary observation closes both**, and it is named as the
successor's first task.

**Gates (merged tree).** `routing_pricing` 6 passed / 0 failed;
`provider::pricing` 8 unit tests; `--lib provider` 358 passed / 0 failed;
`--lib routing::session` 1 passed / 0 failed; `interactive_score_terms`
7 passed / 0 failed; `route_command` **39 passed / 0 failed** with the wiring
in place, which is what says the added production call does not disturb the
binary's routing; clippy `-D warnings`, `cargo fmt --check`, rustdoc and
`check-doc-boundary.sh` all clean; `blast-radius.sh --targeted` — every
traced target passed.

**Two mutations, both KILLED.**

- *fake-zero-collapse*: the unknown-price arm's magnitude → `0.0`, the fake
  zero the line forbids — KILLED by
  `routing_pricing::a_metered_destination_with_no_price_entry_renders_as_unknown_not_free`
  (two further tests failed with it).
- *loader-ignores-user-file*: `load_from_dir` returns `empty()` before
  reading the directory at all — KILLED by
  `routing_pricing::an_unrecognized_providers_price_reaches_the_explanation_with_no_recompilation`.

**Recorded limits, kept.** No schema validation beyond TOML well-formedness
and per-field range checks: an unrecognized extra table is ignored rather
than refused, a design choice no test pins. No estimate is derived from a
price yet — that is 1298's work and 1298 has no input-size producer.

**The successor, and it is one package.** (1) The shipped-binary observation
that closes 1305 and 1306. (2) Line **1307**, *"record the estimated cost
used in a routing decision"* — `RoutingObservation` already carries
`pub cost: Option<ObservedCost>` (`routing/evidence.rs:451`) and
`EvidenceLedger::record` already accepts it; its production writers are
`main.rs:1678` and `:1730`. Both halves live in `main.rs` and both are
additive to the wiring above.


# Lines 1305, 1306 — COMPLETE 2026-09-01; the held boxes close on a shipped-binary observation

Package `GH-PRICING-RECORDED` (Sonnet, high, Amber; batch 77). **This package
changed no production code at all** — 161 insertions, all in
`crates/glasshouse/tests/route_command.rs` — which is exactly what the
holding ruling above asked for. The mechanism was already right; what was
missing was somebody watching it work in the real binary.

Four tests, on the shipped binary's own fixture (a planted harness on PATH,
an argv log, a real config dir), with `plant_pricing` writing `pricing.toml`
where `PriceTable::load_from_dir` actually resolves it — so the orchestrator's
`session_router` wiring is exercised end to end rather than asserted:

- **1306** — `a_pricing_toml_this_binary_was_never_compiled_with_reaches_the_real_route_output`:
  a provider/model this binary has no compiled knowledge of, and the real
  `glasshouse route` output contains *"its price is known"*, *"$3.00 per
  million input tokens"* and *"$9.00 per million output tokens"* — the exact
  figures from the planted file.
- **1306** — `correcting_the_price_in_the_file_changes_the_next_runs_real_output`:
  `$1.00`/`$2.00` before, `$5.00`/`$20.00` after, **and a negative assertion
  that the old figure is gone**. Updated independently of the router, with no
  recompilation, which is the line's whole claim.
- **1305** — `unknown_and_free_are_textually_distinct_in_the_real_route_output`:
  *"its price is unknown"* for a metered destination with no entry, *"is a
  zero-cost resource"* for a free one. The distinction the line exists for,
  in real output rather than at an API boundary.
- **1305** — `with_no_pricing_toml_the_base_fixture_still_says_unknown_never_a_fabricated_zero`:
  the default state of every user who has not written the file.

`route_command` goes 39 → **43 passed, 0 failed**. Mutations were not re-run
and correctly so: the report claims no production code changed, and
`git diff --stat` confirms one test file — so `GH-PRICING-CHANNEL`'s two
KILLED mutations still stand over the same production source.

**Recorded limits, kept.** Proven at the `SessionStart` moment (no `--task`,
`movement = None`); a tier-movement moment takes a separate documented
zero-priced early return (`session.rs:1254-1261`) and was not exercised —
expected behaviour, not a gap. macOS and Linux locally; the Windows VM leg
was not run.

**Why this is worth a paragraph in its own right.** The holding ruling cost
one extra package and produced a proof that the API-level tests could not
give. Eleven times in this project a box has been ticked whose mechanism was
real and whose reachability was not, and ten of those were found later by an
audit worker. This is the one that was caught first, and the follow-up that
closed it took a Sonnet under half an hour.

# Line 1307 — REFUSED 2026-09-01, and the refusal corrected the register

`GH-PRICING-RECORDED` was also asked to give
`routing_observations.cost_micro_usd` its first producer. **It refused, and
it was right to.** The orchestrator's own register row had called 1307
*"not refused, and closer than any row here"* because `RoutingObservation::cost`
and `EvidenceLedger::record` both already exist. They do, and it does not
help:

- `record_tier_movement` (`main.rs:~1651`) receives **no `Destination` at
  all** — `TierMovement` carries tier labels and reasons, nothing priceable.
- `record_entitlement_fallback` (`main.rs:~1698`) does receive one, so
  `PriceTable::price_for` answers there — **but a per-million-token rate is
  not a cost without a token count to multiply it by.** Writing a rate into a
  column documented as a monetary reading would misrepresent `ObservedCost`
  and make the line's own *"compare estimate against actual usage"*
  meaningless.
- A crate-wide grep for any reachable size estimator found none; the single
  hit, `firewall::store::original_token_estimate`, belongs to the context
  firewall and is not in scope at any routing call site.

The packet told the worker not to fabricate a second estimate when the value
is not in scope at the writer, and it followed that instead of producing a
green box. **1307 therefore joins 1298, 1299 and 1304 waiting on one thing:
an input-size producer at the routing decision point.** That single producer
unblocks four of this phase's ten lines. The register's Phase 32G census has
been corrected accordingly.

# Lines 1298, 1299, 1304 — COMPLETE 2026-09-01. **1307 HELD OPEN on a SURVIVED mutation.**

Package `GH-INPUT-SIZE-PRODUCER` (Sonnet, high, Amber; batch 77). The
producer this phase's census named as the single blocker behind four lines.

**Where the code actually landed, because the commit message lies.** The
implementation — 1005 insertions across `config/mod.rs`, `main.rs`,
`routing/{evidence,mod,session}.rs` and two test files — is in **`645d6cf`**,
whose message is entirely about correcting a measurements entry. The
orchestrator integrated this package, was interrupted mid-review, and then
ran `git add -A` for an unrelated docs commit, sweeping the whole worker
diff in with it. The code is correct and was gated (`blast-radius.sh
--targeted`, every traced target passed, 143+227+54+15+13 quoted); only the
message is wrong, and history was already pushed, so it is corrected forward
here rather than rewritten. **Anyone bisecting this phase should look at
`645d6cf`, not at this commit.**

**1304 — the estimate is measured, not modelled.** Project memory is counted
by calling `memory::inject::briefing` with the real task and running
`firewall::estimate::estimate_tokens` over the text it would actually
inject — a measurement of the real briefing, not a constant. Checkpoints are
measured from the real document via `checkpoint::store::latest_for`, never
from `MAX_BYTES` (a ceiling is not a size). **"Likely repository reads" is
deliberately OMITTED** and recorded as a limit: nothing in this build
predicts which files an agent will open, and inventing a figure there would
fabricate the largest component of the estimate. The line's own *"when
possible"* is what permits the omission. Mutation
*briefing-replaced-by-constant* — KILLED by
`estimated_project_memory_tokens_measures_the_real_briefing_and_changes_with_it`.

**1298 / 1299 — a cost only where both halves are known.** A metered
destination with a known price and a known size is priced; **unknown size
makes the cost unknown even when the price is known**, and free stays a
known zero. 1299's cold resume estimates from that session's own latest
checkpoint — the honest approximation the line's *"or approximated"* allows
— and a session with no checkpoint is unknown, not zero. `WarmSession`'s
standing refusal about accumulated context is untouched. Mutation
*fake-zero-on-unknown-size* (`total_tokens()?` → `.unwrap_or(0)`) — KILLED
by `routed_cost_is_none_when_size_is_unknown_even_with_a_known_price`,
*"unknown size must record no cost row at all, never a fabricated zero"*.

## 1307 — HELD OPEN, and the worker's own mutation is why

The worker returned `verdict: closed` for 1307. **The orchestrator overrode
it to OPEN**, on evidence the worker itself produced and reported honestly.

Its third mutation — `main.rs`, `record_entitlement_fallback`:
`.with_cost(cost)` → `.with_cost(None)` — **SURVIVED** against 130 tests
(`routing_pricing` 63, `routing_evidence` 39, `entitlement_broker` 15,
`--bin glasshouse` 13). Deleting the cost from the writer changes nothing
any test observes.

That writer matters more than the count suggests: `record_tier_movement`
receives no `Destination` and nothing priceable, so
**`record_entitlement_fallback` is the ONLY production path that can write a
cost row**. A SURVIVED mutation there means the one link the line is about —
*"record the estimated cost used in a routing decision"* — is unproven in
production. The surrounding facts are all tested (unknown-size ⇒ no row,
unknown-price ⇒ no row, free ⇒ known zero, a written cost survives its
process, an absent cost leaves the column absent); the delivery is not.

The worker named the reason precisely rather than dodging it:
`EntitlementFallback` has private fields and no public constructor, built
only inside `session.rs`'s fallback-decision logic, so proving the flow
needs a genuine fallback driven through a shipped-binary launch — and it
named the existing fixture shape that does exactly that,
`tests/entitlement_broker.rs::a_launch_that_falls_back_records_the_fallback_with_its_reason`.

**This is the same ruling 1305/1306 got hours earlier, applied to a package
that reported itself complete.** A mechanism that is real but whose
production reach is unproven does not tick here; that shape accounts for all
ten of this project's historical un-ticks, and holding it costs one small
follow-up. **Successor: one shipped-binary test on the `entitlement_broker`
fixture that drives a real fallback and asserts a non-NULL `cost_micro_usd`
with its confidence.** When that mutation is KILLED, 1307 closes.

**Phase 32G now stands at 5/10** (1298, 1299, 1304, 1305, 1306), with 1307
one test away and the remaining four blocked on signals the census names.

---

# Line 1307 — CLOSED 2026-09-01, by exactly the successor the hold named

The hold above asked for *"one shipped-binary test on the `entitlement_broker`
fixture that drives a real fallback and asserts a non-NULL `cost_micro_usd`
with its confidence."* That is what landed, and it took the **priced** path
rather than the free-model escape the packet permitted as a fallback — so the
limit the hold anticipated ("the priced path is unwatched") **does not apply**
and is not recorded.

`entitlement_broker::a_launch_that_falls_back_records_the_chosen_destinations_estimated_cost`
seeds a project checkpoint (so `latest_checkpoint_tokens` is `Some`), writes a
`pricing.toml` for `prov-b`/`shared-model` into the binary's own config
directory, records a throttled `prov-a` observation, then runs the **compiled
binary** as a subprocess — `glasshouse launch claude-code --headless` — and
reads the fallback row back out of `EvidenceLedger::recent`. It asserts
`cost.micro_usd > 0` and `cost.confidence == CostConfidence::Estimated`.

The production reach that was unproven at the hold is now proven by that
subprocess: nothing in the test constructs the row itself.

Mutation `drop-cost-from-fallback-row` (`.with_cost(cost);` -> `;` at
`main.rs:1869`) — the very mutation that SURVIVED and caused the hold — is now
**KILLED**, by that test:

    thread '...records_the_chosen_destinations_estimated_cost' panicked at
    crates/glasshouse/tests/entitlement_broker.rs:2411:28:
    the fallback row carries an estimated cost:

Recorded limits, stated rather than discovered later:

- only the priced/estimated branch of `estimated_cost` is watched; the
  free-model zero-cost branch (`micro_usd: 0`) is not asserted by this test;
- the exact `micro_usd` value is not pinned, only `> 0` — the rendered token
  count is an implementation detail of `Checkpoint::render()`, not a promise of
  this line;
- macOS only; the Linux and Windows legs were not run for this box.

**Phase 32G now stands at 6/10** (1298, 1299, 1304, 1305, 1306, 1307). The
remaining four are blocked on signals the census names.

---

# Independent audit, 2026-09-01 (`GH-AUDIT-BATCH-78`) — 1298, 1299, 1304 and 1307 CONFIRMED

A read-only auditor was dispatched to prove these four **wrong**, on the
standing evidence that all ten of this project's historical un-tickings were
found this way and every one was the shape *"production code whose only callers
are tests"*. It found none of it here.

Method, and it is the one that has actually worked: `cluster-b.py` over the
whole crate first — none of `record_entitlement_fallback`, `routing_destinations`,
`estimated_cost`, `session_checkpoint_tokens`, `estimated_project_memory_tokens`,
`latest_checkpoint_tokens` or `record_tier_movement` appears in its
zero-production-caller list — then each symbol traced by hand, every call site
compared against its file's first `#[cfg(test)]`.

The load-bearing finding for **1307**: `record_entitlement_fallback`
(`main.rs:1833`) is called once, at `main.rs:4928`, inside the **shared** launch
decision block — **not** inside a `--headless` branch. `main.rs`'s first
`#[cfg(test)]` is at line 12696. The routing and fallback block runs *before*
the later `if headless { .. } else { .. }` split, which decides only how the
session is attached. So the test's `--headless` subprocess exercises the same
production path a real launch takes; it is not a test-only door.

**One correction to the record:** 1298/1299/1304's implementation landed in
`645d6cf`, **not** `cd62e83` — `cd62e83` touches only `README.md`, `ORIENT.md`,
`capability-map.md` and `phase-32g.md`. The audit packet said `cd62e83` and the
auditor checked rather than believed it.

Note recorded for future auditors, not a gap: `EstimatedInputSize` has no field
for *"likely repository reads"*, grepped and confirmed absent — so the omission
this phase's entries record is real rather than asserted.
