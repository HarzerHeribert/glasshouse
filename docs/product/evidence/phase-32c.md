# Phase 32C — Subscription capacity estimation

# Lines 1244, 1245, 1246, 1250, 1251, 1254 — COMPLETE 2026-09-01

Package `GH-SUBSCRIPTION-ESTIMATOR` (Sonnet, high, Amber; batch 74). The
phase became reachable this week: every input landed with 56A's telemetry.
An estimator **derived entirely on read** — no table, no migration, no
persisted state; today's history IS the ledger's rows in window.

`estimate_subscription_headroom` (`routing/evidence.rs`, beside the
credential readers whose widen-when-unsure narrowing it reuses verbatim):
accepted-request counts, throttle recency, the quota cache's reset reading,
and per-account session counts from `sessions.entitlement` (migration 22,
fail-soft). Returns `Option<SubscriptionHeadroomEstimate>` — a
`HeadroomBand` (Exhausted/Low/Moderate/Ample) + `Confidence` +
`HeadroomBasis` + `account_narrowed`. **The type structurally cannot carry
fictitious precision** (1250/1251): no numeric field on the band; a token
row changes only the basis label. An opaque-limit account estimates from
activity alone (1244). One contextless row widens the whole estimate to
provider scope (1246). Two accounts' rows never mix (1254 — flagship test
plus KILLED mutation `never-mix`).

Consumer: `populate_provider_facets` estimates whenever capacity is not
per-account authoritative — every reachable case today, and the guard steps
the estimate back the day per-account headers exist (the
authoritative-beats-estimate rule, KILLED mutation b). `to_routing` carries
the facet; `status`/`entitlements` render `headroom estimate:` as its own
segment, never merged into `capacity:`. Nothing scores on it yet —
`routing/session.rs` untouched; a scoring consumer is a later ruling.

13 new shipped-surface tests; targeted gate on the merged tree: 136+227+54
lib tests across the touched modules, 13/13 twice. Full sweep: the wave's
trailing run. Remaining 32C lines (1247–1249, 1252, 1253, 1255) need
plan-change detection, learned resets, multi-window distinction, and the
persistence/override/disable trio — each its own producer decision.
