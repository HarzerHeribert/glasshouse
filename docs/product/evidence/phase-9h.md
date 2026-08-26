# Capability evidence — phase 9h

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 9H — sticky gateway routing, 13 of 14 (lines 505–518)

Contract: Given a gateway-backed interactive session, Glasshouse assigns it one
provider and model at start, keeps it there across normal turns, moves it only
on a real provider failure to a backend that can actually serve the harness,
and says so when it does.

State: COMPLETE (line 511 excepted — see below)

Production evidence:
- `crates/glasshouse/src/routing/interactive.rs` — the policy, a pure function
  of values with no clock and no network. `crates/glasshouse/src/gateway/session.rs`
  feeds every finished exchange back into it.
- **The assignment is made on the production launch path**, in
  `profile::apply_gateway`, which `main.rs::launch_session` reaches through
  `resolve_with_gateway`. `main.rs` was not modified.
- **Verified against the shipped binary**, driven in a real terminal (the
  binary refuses a piped stdin, correctly, so a cmux pane is the only way):
  `glasshouse run claude-code --profile free-gateway` recorded
  `gateway backend: nvidia/nemotron-3-ultra-550b-a55b:free on openrouter
  (openrouter/OPENROUTER_API_KEY over anthropic-messages)` in its launch
  mechanisms, then forwarded two real exchanges to OpenRouter over Anthropic
  Messages, and the provider's status reached the harness byte for byte.
- No credential reached the log, checked mechanically rather than by eye:
  `grep -F "$OPENROUTER_API_KEY" gateway.log` found nothing, and no `sk-or-`
  prefix appeared anywhere in it.

Regression evidence — 25 mutations, 25 killed, three of them only after the
survivor forced a fix. The load-bearing ones:
- M2 the pin no longer stops failover → `a_pinned_session_does_not_fail_over_even_when_a_perfect_candidate_exists`
  and `gateway::conformance::a_pinned_session_stays_on_its_failing_provider_and_never_reaches_the_other_one` both FAILED.
- M3/M4 failover ignores protocol / tool semantics → `failover_never_crosses_a_protocol`,
  `failover_never_weakens_what_is_established_about_tool_calls` FAILED.
- M5 a different model is taken as a failover rather than offered as a
  migration → `a_different_model_is_offered_as_a_migration_rather_than_taken` FAILED.
- M22 the policy is asked on every turn rather than only after a failure →
  three conformance tests FAILED. This is the stickiness line: a router that
  re-decides each turn is not sticky even if it usually picks the same thing.
- M25 → `every_turn_goes_to_the_assigned_backend_and_a_free_alternative_is_never_connected_to` FAILED.

**The finding of the batch — a caller that every test bypasses is not a
caller.** M18 deleted `apply_gateway`'s call to `Gateway::routing().bind` and
**broke nothing**: all ten gateway conformance tests bound the assignment
themselves in their own helper, so the whole suite passed against a build in
which the production launch path recorded no assignment at all. Fixed by
`profile::tests::resolving_a_gateway_backed_profile_assigns_the_session_a_provider_and_a_model`,
which goes through the function `launch_session` actually calls; the mutation
then FAILED. M24 was the same shape one layer down — `to_launch_profile`
dropping the stored pin broke nothing, because the profile-side test built its
`LaunchProfile` by hand.

**A defect only the live run could find.** `402 Insufficient credits` was first
classified as a healthy exchange: the first version mapped `401`/`403` to
`CredentialRejected` and everything else to `Served`. A `402` is neither a
provider outage nor a malformed request — it is *this account's key* being
unable to pay, and another key on another account would serve. It now rotates
like `401`/`403` (M20, M21). No fixture would have produced a `402`, because
nobody would have thought to write one.

Known limit, recorded rather than fixed:
- **No live `200` was ever obtained.** OpenRouter answers `402` for `:free`
  models on an account that has never purchased credits, so nothing in this
  batch proves a free model *answered*. The free-pool health path is proven
  against fixtures and against a real `402`, not against a real success. No box
  was closed as if it had been.

Orchestrator judgement, recorded because the lead asked for it to be overruled
if wrong:
- **Line 518 is closed on a profile-level reading of "pin".** The user records
  a pin in configuration; it reaches the launch profile, round-trips (M24), and
  turns automatic failover off at session start (M23). The line says the user
  may pin a gateway-backed session and disable automatic failover, and that is
  what happens. A pin typed at a *running* session is a richer capability the
  line does not require; it would need `cli.rs` or a shell surface holding a
  handle on a live gateway, and that work is scoped in the lead's §7.2.

Not closable:
- **511** *explicit session migration at a task boundary.* Built and proven as
  a mechanism — `InteractiveRouting::migrate`, `SessionRouting::migrate`,
  `SessionActivity`, mutation M8 killed by
  `a_migration_is_refused_mid_turn_and_allowed_between_tasks` — with **no
  production caller**. Nothing in the shipped binary can ask for a migration.
  §5.

---
