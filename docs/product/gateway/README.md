# Inference gateway — product docs

See [`../architecture.md`](../architecture.md) for the three-component split
this crate sits at the bottom of.

## What it is

`crates/inference-gateway` (library and the `inference-gateway` binary) is
the bottom layer of the three-component split: Pane and Glasshouse both sit
on it, neither is required by it, and neither is required by the other. It
owns providers, credentials, entitlements, quota, free pools, subscriptions,
same-model failover, protocol translation and usage accounting.

## The four hard rules

- Nothing in this crate may name Glasshouse — not a type, not a path, not a
  dependency.
- A caller sends a model, an effort, a request and a fallback policy.
- The gateway may choose the provider, the account and the entitlement.
- The gateway may not change the model or the effort unless the caller's
  fallback policy explicitly permits it.

## The standalone process

`inference-gateway serve [--listen 127.0.0.1:0] [--config <path>]
[--data-dir <path>]` prints one stdout line
`{"listening":"http://127.0.0.1:<port>","token":"<bearer>"}` and serves until
stdin reaches EOF or SIGTERM/SIGINT/SIGHUP; a fixed port is refused by name
rather than silently ignored. Its config is `gateway.toml` under the
platform config directory: `[accounts.<name>]` in the entitlement
catalogue's own shape, and `[providers.<name>]` with `base_url`/`protocol`
(or a `protocols` map) and `credential_env` names — never values.

## The CLI subcommands

- `serve` — run the standalone process.
- `entitlements --json [--refresh]` — what each configured account is and
  what it can serve.
- `subscriptions connect <provider> --entitlement <name> --json` — connect
  one configured account with the provider's OAuth flow.
- `routing-cost --json --since <unix>` — what routing has consumed, in the
  same JSON Lines a host emits (empty, exit 0 — no ledger standalone).
- `credentials` — provider API keys this gateway stores and resolves.

## The boundary tests

- `crates/inference-gateway/src/gateway/tests.rs ::
  the_gateway_names_no_glasshouse_path`
- `crates/inference-gateway/src/gateway/tests.rs ::
  the_gateway_imports_none_of_the_modules_that_would_make_it_a_harness`
