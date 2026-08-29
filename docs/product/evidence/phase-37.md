# Phase 37 — basic session-aware router

### Eight lines closed, two returned open with measurements, one refused (batch 51)

State: **COMPLETE** for 1592, 1593, 1595, 1596, 1597, 1600, 1601, 1602.
**Open:** 1598, 1599. **Refused:** 1594.

Built in two packages, deliberately staged because `main.rs` was contended:
`GH-ROUTER` built and mutation-killed the scoring on eleven axes and could not
reach a production caller — the packet forbade `main.rs` while requiring a CLI
entry point, and `main.rs`'s dispatch is an exhaustive `match` with no wildcard,
so a new `Command` variant is a compile error until it gains an arm. That was an
orchestrator packet defect, not a design one. `GH-ROUTE-WIRING` then supplied the
callers, co-editing `main.rs` under §77.

Production: `routing/session.rs` (`SessionRouter`, `Destination`,
`RouterInputs`, `RoutingMoment`, `Routed`), the contributions in `routing/mod.rs`,
`glasshouse route` in `cli.rs`, and the `choose` calls in `main.rs`'s
`launch_session` and resume paths.

**Every `Consider X` line has a discriminating test** — two candidate sets
differing only in X, resolving differently. A test asserting X appears in the
explanation proves the renderer, not the router.

**The mutation that matters is on the call, not the callee.** The router's own
eleven mutations prove the scoring and none can prove the wiring, which is this
project's most common defect. `launch-path-ignores-the-user-override` re-run by
the orchestrator in the integrated tree: **KILLED**, by an assertion written to
say so — *"this is the assertion that fails when `launch_session` stops calling
`SessionRouter::choose`"*.

**1594 refused, premise-invalid**, with a tripwire test recording why; it would
not close even with the caller.

**1598 and 1599 returned open with measurements rather than excuses**: their
inputs have production callers, and neither input can take a non-empty value on
any path a test can reach.

**The §77 measurement, finally with two real edits.** `main.rs` carried three
claimants. One (`GH-WORKER-ACCESS`) declared done having never edited it — a
cost the protocol did not previously report, now surfaced by `coedit.sh done`.
The other two edited genuinely disjoint regions (one hunk at `:2930`; the rest
at `:146`–`:433`), so **both intents were preserved without a merge**.
`integrate.sh` correctly refused to choose a winner and handed reconciliation
back to the orchestrator, which is rule 5 working as written. The honest verdict:
co-editing cost nothing here and saved a queued round, but it has still not been
tested against edits that actually collide.
