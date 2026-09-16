# Phase 68 — Gateway: successors named by the 2026-09-16 cleanup

Map lines 2629–2630. Both were recorded on 2026-09-16 from the cleanup round's findings (`design-decisions.md`, *Glasshouse does not route*; `.agent-runtime/report-gateway-process-tests.md`; `.agent-runtime/report-cleanup-pool-duplicate.md`).

## Non-negotiables

- The gateway names no Glasshouse type or path (`gateway/tests.rs :: the_gateway_names_no_glasshouse_path`); anything shared lives in the gateway and is imported by the host, never the other way.
- The gateway may choose provider, account and entitlement; it never changes model or effort without the caller's policy.

## Gate

Targeted: the changed files' own targets plus the worker's quoted tests, on the merged tree. The twelve-cell GitHub sweep trails on push.

## Line 2629 — `gateway.toml` declares an api-key account's models

**State: COMPLETE** (2026-09-16, `GH-GATEWAY-ACCOUNT-MODELS`, Sonnet high, Amber; report `.agent-runtime/report-gateway-account-models.md`; integrated by the primary on the targeted gate).

**Contract.** Given `gateway.toml` with `[accounts.<name>] models = ["m1", "m2"]` on an api-key account, when a request names a model no configured account serves, the standalone `inference-gateway` answers an error and forwards to nobody, while an account without `models` keeps today's behaviour (serves any model) and same-model failover across accounts is unchanged.

**Production.** `crates/inference-gateway/src/entitlement.rs :: AccountEntry::models` (serde default, skipped when unset; `models()` / `set_models()`); `crates/inference-gateway/src/pool.rs :: pool_from_catalogue` (the api-key arm calls `UpstreamBackend::with_models` when declared; subscription-backed accounts keep `subscription_models`); `docs/product/gateway/README.md` names the field.

**Tests.** `crates/inference-gateway/tests/boundary.rs :: a_request_for_a_dead_model_is_refused_rather_than_served_by_another_model` (un-ignored and rewritten: account A serves `M` at a dead port, B declares only `N`; a request for `M` is non-2xx and B's fake records nothing); `config::tests::{an_account_declaring_models_parses_the_list, an_account_without_models_parses_to_none}`.

| Decision | Mutation | Killing test | Result |
|---|---|---|---|
| Declared models reach the backend (2629) | `pool.rs`: `Some(models) => backend.with_models(models),` → `Some(_) => backend,` | `boundary::a_request_for_a_dead_model_is_refused_rather_than_served_by_another_model` | KILLED — `a request whose account is unreachable must not read as served: HTTP/1.1 200 OK` |

**Gates.** `cargo test -p inference-gateway --test boundary`: 5 passed, 0 ignored; lib 528 passed; `tests/bin.rs` 10 passed; `cargo check -p glasshouse --all-targets --keep-going` clean with no host edit; the targeted gate (4 files) green on the worker's tree and on the merged tree at integration.

**Limits.** This pins *account selection by declared model* at the process boundary; whether `routing::interactive`'s `OfferMigration` arm is reachable from the binary is a separate question the worker names as orthogonal. Subscription-broker accounts are unchanged. `entitlements --json` builds its own object from the cached catalogue and does not render `AccountEntry`, so the field does not appear there (packet error, accepted). The worker reports one pre-existing red among the binary's four `main.rs` unit tests, unrelated to this change — to be attributed on main.

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
