# Capability evidence — phase 32G

Phase 32G — *provider-aware request-cost estimation*. Ten lines, and until
2026-09-01 all ten were unreachable for one reason: **there was no price data
anywhere in this build.** The census — which line is blocked by what, read
rather than assumed — is in `docs/process/refusal-register.md` under
*"Phase 32G — the census"*.

# Lines 1305, 1306 — MECHANISM LANDED, BOTH BOXES HELD OPEN 2026-09-01

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
