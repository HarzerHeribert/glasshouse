# Capability evidence — phase 9g-refined

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 9G refined — the several-providers refusal is lifted, and why that is not a reversal

`profile::gateway_upstream` used to refuse a configuration in which more than
one provider served the gateway ingress. It now assigns the first in the user's
configuration order and keeps the rest as failover candidates.
`GatewayUpstreamRefusal::SeveralProvidersServeTheIngress` is **removed**,
because an error variant that can never be produced is decoration (§20 applied
to an enum).

**This is the guard being retired by the phase it was holding a place for, not
overridden.** 9G's objection was to a *silent* choice at a time when no phase
owned the decision. 9H owns it now, and the choice is announced in the launch's
own mechanism notes (`category: "gateway backend"`, carrying provider and model
names and never a credential), pinnable (518), migratable in principle (511),
and recorded on every change (515).

Kept as a guard, it would have done the opposite of its purpose: a user with two
configured routers could not start a gateway-backed session at all, and **every
9H failover line would be unreachable by construction** — a temporary
placeholder converted into a permanent block on the capability it was
protecting. The alternative the lead offered — keep refusing until a launch
profile can name its own gateway provider, at the cost of a field on
`BackendResource::GlasshouseGateway` — is defensible and remains available if a
later phase needs per-profile provider selection.

---
