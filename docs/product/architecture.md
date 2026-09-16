# Architecture

Three components, each its own crate and binary. None of the three requires
another to exist except along the arrows below (the user, 2026-09-10).

```
Human → (model / effort / spend / fallback policy)
Pane ──────► Gateway ──────► Providers
 └─────────► Glasshouse (optional)
```

## The three components

**Inference gateway** (`crates/inference-gateway`, binary `inference-gateway`)
owns providers, credentials, entitlements, quota, free pools, subscriptions,
same-model failover, protocol translation and usage accounting. It chooses the
provider, account and entitlement for a request; it never changes the model or
effort unless the caller's fallback policy permits it. Pane spawns it
(`inference-gateway serve --listen 127.0.0.1:0`) or Glasshouse starts or
attaches to one. It is the bottom layer — nothing here may name Glasshouse
(the user, 2026-09-10).

**Pane** (`crates/pane`, binary `pane`) is the harness loop: one session,
cells, sandbox, supervisor, subagents, the completion gate. It sends the
model, effort, request and fallback policy to the gateway. It depends on
nothing to run — it reaches the gateway over HTTP and the `glasshouse` binary
only optionally, by shelling out (the user, 2026-09-06).

**Glasshouse** (`crates/glasshouse`, binary `glasshouse`) is the optional
control station for several sessions at once: the session list, launch and
resume of foreign harnesses, structured project memory (semantic search
wanted), checkpoints, the context firewall, the control API/MCP door,
cross-session claims and cost visibility. It never decides which model is
used — its routing recommendations are being removed (ruling 2026-09-16).

## The four hard rules — the user, 2026-09-10

- Glasshouse is *completely optional*.
- Pane sends model, effort, request and fallback policy.
- The gateway may choose provider, account and entitlement.
- The gateway may not change model or effort unless the caller's fallback
  policy explicitly permits it.

## The rule of thumb — the user, 2026-09-06

Anything that happens inside one turn or one session belongs to Pane.
Anything that spans sessions, projects, providers or machines belongs to
Glasshouse. The gateway sits below both and is reached by either.

## How it is enforced

| invariant | test |
|---|---|
| Nothing in the gateway crate may name Glasshouse | `crates/inference-gateway/src/gateway/tests.rs :: the_gateway_names_no_glasshouse_path` |
| The gateway imports none of the modules that would make it a harness | `crates/inference-gateway/src/gateway/tests.rs :: the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness` |
| A Pane session runs standalone against a gateway it started, no Glasshouse process anywhere | `crates/pane/tests/session.rs :: a_session_runs_standalone_against_a_gateway_it_started` |
| No routing policy inside Glasshouse can make a request | `crates/glasshouse/src/routing/mod.rs :: no_routing_policy_can_make_a_request` |

## Where the docs live

- `docs/product/pane/` — Pane's own product docs (exists).
- `docs/product/gateway/` and `docs/product/glasshouse/` — announced by the
  2026-09-16 ruling; created by a later package.
- `docs/product/capability-map.md` — the historical requirement ledger; its
  phase text predates the three-component split and should be read as
  history, not as the current shape.
- `docs/product/design-decisions.md` — holds the rulings this page quotes.

## Brand note — the user, 2026-09-16

The three components keep the Glasshouse name: `x-glasshouse-*` headers,
`GLASSHOUSE_*` environment variables, and the Glasshouse keychain service.
The separation between them is architectural, not nominal.
