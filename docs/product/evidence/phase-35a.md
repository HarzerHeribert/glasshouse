# Phase 35A — Candidate generation

**0 of 10 closed at the time of writing (1516 was already ☑ and is not in
this census). This file exists because the phase had no evidence-ledger entry
at all** — `discover.py --phase 35A` reported *"no file paths found"*. A
read-only census (`GH-RECON-35A`, 2026-09-02, Sonnet high; report kept at
`.agent-runtime/report-recon-35a.md`) established what blocks it, grouped by
root cause per practice §87, and the orchestrator verified its decisive claims
against `a79b276` before writing this.

**The ten lines reduce to three root causes.** One is a real, narrow mechanism
gap. One is a producer absence this project has already refused once. One is
"already true in production, never proven" — the cheapest shape — and it
covers seven of the ten.

| line | cause | verdict |
|---|---|---|
| 1517, 1518 | **1** — `hard_constraint` excludes on three axes; capability and provider availability exist only as scoring terms | **packageable (Amber)** — `GH-CANDIDATE-GEN`, dispatched 2026-09-02 |
| 1519 | **2** — the same missing spend counter as register row 1263 (Cluster M) | refused |
| 1511, 1512, 1513, 1514, 1515, 1520, 1521 | **3** — already true in production, never proven | **packageable (Green, proof-only)** — `GH-CANDIDATE-PROOFS`, dispatched 2026-09-02 |

---

## Cause 1 — the gate has a fixed, narrow exclusion set; two facts it could refuse on are only priced

`hard_constraint` (`routing/session.rs:4803`) is the **only** place a
destination leaves the ranked set before scoring — `choose`'s own doc says so
(`:4327-4336`). Its doc comment states its scope: entitlement rules, tool
semantics, protocol, and the line-1516 tier ceiling. Production chain: a launch
→ `launch_session` (`main.rs:~4562`) → `RouterInputs` → `SessionRouter::choose`
→ `gate` → `apply_hard_constraints`.

**1517.** `capability_fit` (`:1233`) prices an established-absent hard
capability at `CAPABILITY_ESTABLISHED_ABSENT = -0.4`, a bounded penalty a warm
candidate can outscore. The source names the gap as unfinished:
`TaskRequirements`'s doc (`:771-778`) says *"nothing in this package constructs
a `HardConstraint::Capability`"* — verified: `HardConstraint::Capability`
(`routing/mod.rs:545`) is constructed nowhere in the tree. `is_adequate`
(`:4759`) already answers the fact (false only on
`Declared::Verified { value: false }`; unverified passes) and is called only
from `decide_tier_movement` (`:3456`) and `alternatives_for` (`:4734`).
**Producer and propagation are live** — `launch_session` and
`route_recommendation` build `requirements` from
`classified.answer.requirements()` (`request.rs:644-648`); the doc comment at
`session.rs:1215-1223` claiming `main.rs` passes `TaskRequirements::default()`
is stale and the package corrects it.

**1518.** `provider_health` (`:2300`) prices a rejected credential or a
cooldown softly. `provider_available` (`:4779`) computes the same two facts as
a boolean and is called only from `decide_tier_movement` (`:3455`) and
`alternatives_for` (`:4735`) — never the gate. The disposable path already
hard-filters on `pool.is_available` (`disposable.rs:1188`, `:1222`, `:1495`);
the interactive path does not. **The asymmetry is the finding.**

**The decision the package makes, ruled 2026-09-02:** the line's word is
*authoritative*. `CooldownCause` (`routing/free.rs:250`) distinguishes
`Declared` (the provider stated the wait, line 1319) from `Invented`
(Glasshouse's own bounded backoff, kept probeable by line 534). A rejected
credential or a `Declared` cooldown **excludes**; an `Invented` cooldown stays
priced. Excluding on Glasshouse's own guess would turn a probe-worthy resource
into an unreachable one.

## Cause 2 — 1519, the money budget is still not counted

Re-verified against current source rather than trusted from the register:
`provider/resources.rs:1101` still renders the configured budget with
*"Glasshouse does not count spend against this"*; `cost_micro_usd`'s only
production writer is memory extraction, `None` under the default configuration.
`EntitlementConfig::spend_ceiling_tokens` (`config/mod.rs:1976-1990`) is real
and hard-excludes via `Entitlement::spend_constraint` — but its own doc
comment says it is *tokens, not money*, and that the money ceiling
(`[providers.<name>.quota] budget`, line 1203) *"remains, by its own
documentation, uncounted."* Sharing a name is not sharing a fix. **Cluster M,
register row 1263; no successor until the `ingress` ruling.**

## Cause 3 — already true in production, never proven

Each line's production chain, from the census and spot-verified:

- **1511** — `routing_destinations` (`main.rs:1033`) pushes existing sessions
  (`:1093-1195`) before fresh ones (`:1198-1308`); `choose`'s doc (`:4314`)
  makes destination order the tiebreaker. No test pins the tie.
- **1512** — the same loop builds one fresh `Destination` per enabled profile
  via `destination_backend`'s Native arm (`:1607-1618`), enabled per
  `profile_enabled` (`config/mod.rs:5045`). Existing tests assert "a fresh
  destination exists", not a Native one from an enabled profile.
- **1513** — gateway-backed destinations pass the same protocol
  (`session.rs:4843-4847`) and tool-semantics (`:4838-4842`) checks as every
  candidate, before scoring. **Its capability half closes with 1517**; the
  protocol/tool half is provable today.
- **1514** — on the acting path, `session::select::select_with`
  (`session/select.rs:469-509`) resolves the executable and fails before
  `routing_destinations` runs; `Destination` cannot be built without a harness.
  **Recorded limit:** `route_recommendation` (`main.rs:3967-4008`, report-only)
  ranks config-enabled harnesses without an installed check. Whether a report
  is "generating a candidate" is a reading question; the proof is scoped to the
  acting path.
- **1515** — `disposable_candidates` (`main.rs:6901`) → `DisposableRouting::choose`
  (`disposable.rs:1107`); proven in substance by Phase 35B's suite, never named
  as this line's criterion.
- **1520** — disabled profiles are removed before generation (`main.rs:1233`);
  `Entitlement::constraint` (`routing/mod.rs:1321`, from `hard_constraint`) and
  the disposable side's `job_constraint`/`permits_metered` exclude what user
  policy forbids. Phase 56's tests prove the mechanism; none is framed as this
  line.
- **1521** — `profile_names` always holds the Native profile
  (`config/mod.rs:4972-4980`) and `profile_enabled` answers true for it
  unconditionally (`:5045-5048`), proven by
  `config::tests::the_native_profile_is_always_available_for_every_harness`;
  only a hard constraint removes a destination. **Recorded limit:** "usable"
  means passing the gate for the task in hand — a Native profile whose ceiling
  is below a classified minimum tier is not usable and the line does not claim
  it survives. The proof uses a task with no tier requirement.

The package's mutation table (one per line) is in the recon report's Cause 3
section and in `GH-CANDIDATE-PROOFS`'s packet.

---

## Recommended package boundary (as dispatched)

**`GH-CANDIDATE-GEN`** — 1517, 1518. Amber. `routing/session.rs::hard_constraint`
gains two arms reusing `is_adequate`, `provider_available`/`FreePool::health`,
and `HardConstraint::Capability`; `routing/mod.rs` gains one variant for
provider unavailability. Two mutations plus a third proving `Invented` still
passes. Co-edited with `GH-BURN-FORECAST` on both files (§77).

**`GH-CANDIDATE-PROOFS`** — 1511, 1512, 1513 (protocol/tool half), 1514, 1515,
1520, 1521. Green, tests only, one census mutation per line. Co-edited with
`GH-BURN-FORECAST` on `main.rs`.

**1519** stays refused.
