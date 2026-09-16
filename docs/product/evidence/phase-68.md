# Phase 68 — Gateway: successors named by the 2026-09-16 cleanup

Map lines 2629–2630. Both were recorded on 2026-09-16 from the cleanup round's findings (`design-decisions.md`, *Glasshouse does not route*; `.agent-runtime/report-gateway-process-tests.md`; `.agent-runtime/report-cleanup-pool-duplicate.md`).

## Non-negotiables

- The gateway names no Glasshouse type or path (`gateway/tests.rs :: the_gateway_names_no_glasshouse_path`); anything shared lives in the gateway and is imported by the host, never the other way.
- The gateway may choose provider, account and entitlement; it never changes model or effort without the caller's policy.

## Gate

Targeted: the changed files' own targets plus the worker's quoted tests, on the merged tree. The twelve-cell GitHub sweep trails on push.

## Line 2629 — `gateway.toml` declares an api-key account's models

**State: OPEN.** Named by `GH-GATEWAY-PROCESS-TESTS` (2026-09-16): `pool.rs :: pool_from_catalogue` never calls `UpstreamBackend::with_models` for `kind = "api-key"`, so the migration refusal (`routing::interactive :: FailureResponse::OfferMigration`) is unreachable from the standalone binary and `tests/boundary.rs :: a_dead_account_request_is_refused_but_migration_refusal_is_unreachable_from_this_binary` is `#[ignore]`d with that reason in its name. A successor adds `models = [...]` per account and un-ignores it.

## Line 2630 — one ingress list, served by the embedded gateway too

**State: COMPLETE** (2026-09-16, `GH-GATEWAY-INGRESS-ONCE`, Sonnet medium, Amber; report `.agent-runtime/report-gateway-ingress-once.md`; integrated by the primary on the targeted gate).

**Contract.** Given a Pane launched by Glasshouse on a gateway-backed profile, when Pane posts `/v1/systemone` to the embedded gateway, the request is served exactly as the standalone binary serves it, while every other ingress target keeps its route and `GATEWAY_INGRESS_PROTOCOLS` exists once, in `crates/inference-gateway/src/pool.rs`.

**Production.** `crates/inference-gateway/src/pool.rs :: GATEWAY_INGRESS_PROTOCOLS` (five protocols, `TypesafeSystemOne` included) and `pool.rs :: gateway_routes` (now `pub`); `crates/glasshouse/src/profile/mod.rs :: gateway_upstream` imports both — the host's four-protocol copy and its private `gateway_routes` are deleted, with the doc comment that justified four ("no installed harness speaks TypesafeSystemOne at the ingress yet" — Pane does, Phase 66).

**Tests.** `crates/glasshouse/tests/gateway_translate.rs :: a_typesafe_provider_built_through_profile_gateway_upstream_serves_systemone` (new); `crates/glasshouse/src/profile/tests.rs :: {each_harness_is_given_the_protocol_it_speaks_not_the_first_one_served, the_ingress_target_table_covers_every_protocol_the_gateway_serves}` (now pin five and say why).

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| The embedded gateway serves the gateway's list (2630) | `pool.rs`: remove `WireProtocol::TypesafeSystemOne,` from `GATEWAY_INGRESS_PROTOCOLS` | `gateway_translate.rs :: a_typesafe_provider_built_through_profile_gateway_upstream_serves_systemone` | KILLED — `served_protocols() no longer contains typesafe-systemone` (gateway_translate.rs:430) |

**Gates.** `cargo test -p glasshouse --test gateway_translate`: 10 passed; `--lib profile`: 127 passed; `cargo test -p inference-gateway --lib pool`: 17 passed; the targeted gate (4 files) green on the worker's tree and again on the merged tree at integration.

**Limits.** The gateway's own conformance test builds its `Upstream` directly and does not kill this mutation (the worker's packet error; the host test does). The worker's limit "no `typesafe` provider template exists in `glasshouse::provider`" is contradicted by `crates/glasshouse/tests/provider_discovery.rs`'s sixteen-template matrix, which includes `typesafe` through the re-export — the route is reachable for a configured typesafe provider. Not exercised: a live decision model through an embedded gateway.
