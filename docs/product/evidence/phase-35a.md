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

---

# Cause 3 — CLOSED 2026-09-02 (`GH-CANDIDATE-PROOFS`, Green, tests only)

Seven lines, nine tests, zero production lines changed, every census mutation
KILLED with its killing test named and its failure text quoted. Six tick in
this commit; **1513 stays open** until `GH-CANDIDATE-GEN` lands its capability
arm and the gateway-backed capability test with it. The worker read
`burn-forecast`'s `main.rs` diff once at finalization (§77) and adapted
nothing: the peer's edits are inside `routing_destinations`'s body and the
capacity helpers, and its tests call only signatures.

### Generate routing candidates from relevant existing sessions before considering fresh sessions. (line 1511)

Contract: Given a project with one existing session and the implied Native profile, when routing_destinations builds its candidate vector, existing sessions occupy earlier indices than every fresh destination, while no production behaviour changes.

State: **COMPLETE** — ruled 2026-09-02. The test drives `routing_destinations` on a real project store and the mutation reversed the passes; proof-only, no production change.

Production evidence:
- `crates/glasshouse/src/main.rs` — `routing_destinations`

Regression evidence:
- `main.rs::tests::routing_destinations_generates_existing_sessions_before_fresh_ones_1511`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| built fresh destinations into a separate `fresh_destinations` vec inside the offered-profile loop and changed the final `Ok(destinations)` to `Ok(fresh_destinations.into_iter().chain(destinations).collect())` | `reorder-generation-passes` | **killed** | `main.rs::tests::routing_destinations_generates_existing_sessions_before_fresh_ones_1511` |

> reorder-generation-passes observed: assertion `existing_index < fresh_index` failed: existing sessions must be generated before fresh ones

Recorded scope limits — stated by the worker, not discovered later:
- proves generation order only; the tiebreaker effect on SessionRouter::choose itself is documented at session.rs:4314-4315 and not separately re-derived here

---

### Generate fresh native-subscription session candidates from enabled harness launch profiles. (line 1512)

Contract: Given a project with only the implied Native profile enabled, when routing_destinations builds fresh candidates, a fresh Native-backed destination for the harness exists, while no production behaviour changes.

State: **COMPLETE** — ruled 2026-09-02. Proof-only.

Production evidence:
- `crates/glasshouse/src/main.rs` — `routing_destinations`
- `crates/glasshouse/src/main.rs` — `destination_backend`

Regression evidence:
- `main.rs::tests::routing_destinations_offers_a_fresh_native_destination_from_the_enabled_profile_1512`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| for name in offered { -> for name in offered { if name == glasshouse::profile::NATIVE_PROFILE_NAME { continue; } | `skip-native-in-generation-loop` | **killed** | `main.rs::tests::routing_destinations_offers_a_fresh_native_destination_from_the_enabled_profile_1512` |

> skip-native-in-generation-loop observed: panic: the enabled implied Native profile must offer a fresh destination for this harness (the .find() returned None)

---

### Generate fresh gateway-backed session candidates only as installed-harness launch profiles whose protocol, model, tool semantics, and capability requirements match. (line 1513)

Contract: Given a destination whose backend's protocol the harness cannot speak (no translation pair) or whose tool semantics are established absent while the task needs tool calls, when SessionRouter applies its hard constraints, that destination is excluded before scoring, while a compatible sibling is chosen, and while no production behaviour changes.

State: **PARTIALLY VERIFIED** — ruled 2026-09-02. Protocol and tool-semantics halves proven through the real gate; the capability half is line 1517's arm (`GH-CANDIDATE-GEN`) and this line ticks with it, citing that package's gateway-backed capability test.

Production evidence:
- `crates/glasshouse/src/routing/session.rs` — `hard_constraint`
- `crates/glasshouse/src/routing/session.rs` — `classify_destination`

Regression evidence:
- `tests/routing_candidates.rs::a_protocol_incompatible_destination_is_excluded_before_scoring_1513`
- `tests/routing_candidates.rs::a_tool_incompatible_destination_is_excluded_when_the_task_needs_tool_calls_1513`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| if classify_destination(...).protocol_fit() == ProtocolFit::Incompatible -> if false && classify_destination(...).protocol_fit() == ProtocolFit::Incompatible | `drop-protocol-hard-constraint` | **killed** | `tests/routing_candidates.rs::a_protocol_incompatible_destination_is_excluded_before_scoring_1513` |
| if inputs.requirements.needs_tool_calls && ... == KnownAbsent -> if false && inputs.requirements.needs_tool_calls && ... == KnownAbsent | `drop-tool-semantics-hard-constraint` | **killed** | `tests/routing_candidates.rs::a_tool_incompatible_destination_is_excluded_when_the_task_needs_tool_calls_1513` |

> drop-protocol-hard-constraint observed: assertion left == right failed: the protocol-incompatible destination must be hard-refused, not scored

> drop-tool-semantics-hard-constraint observed: assertion left == right failed (HardConstraint::ToolSemantics expected, refused list was empty)

Recorded scope limits — stated by the worker, not discovered later:
- the capability half (line 1517, GH-CANDIDATE-GEN) is out of scope per the packet and not proven here

---

### Never generate a direct API or gateway endpoint as a first-class interactive session candidate without an owning installed harness. (line 1514)

Contract: Given a harness with no configured executable and no candidate name resolvable on PATH, when session::select::select_with resolves it for the acting (launch) path, selection fails with SelectionError::NotInstalled before routing_destinations is ever called, while no production behaviour changes.

State: **COMPLETE** — ruled 2026-09-02, on the acting path. The report-only `glasshouse route` ranking of a config-enabled, not-installed harness is recorded as a limit in the census entry above and is not this line's subject: nothing is generated as a session candidate there.

Production evidence:
- `crates/glasshouse/src/session/select.rs` — `select_with`
- `crates/glasshouse/src/session/select.rs` — `resolve_executable`

Regression evidence:
- `session/select.rs::tests::an_uninstalled_harness_is_refused_before_any_routing_candidate_could_exist_1514`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| Err(SelectionError::NotInstalled { id }) -> resolve_configured(Path::new("/bypassed-installed-check")).map(|executable| (executable, ExecutableSource::Path { name: "bypassed".to_owned() })).map_err(|_| SelectionError::NotInstalled { id }) | `bypass-installed-check` | **killed** | `session/select.rs::tests::an_uninstalled_harness_is_refused_before_any_routing_candidate_could_exist_1514` |

> bypass-installed-check observed: panic in the test's no_configured_lookup guard: "has no configured executable to resolve" — the mutated code reached the configured resolver instead of cleanly refusing

Recorded scope limits — stated by the worker, not discovered later:
- proof scoped to the acting (launch) path; the report-only `glasshouse route` gap (route_recommendation ranking a config-enabled-but-not-installed harness) is a recorded limit per the census, not addressed
- ResolvedExecutable has no public constructor outside platform::exec's real resolvers, so the mutation's KILLED verdict comes from a panic in the test's own guard rather than from a fabricated successful executable reaching a routing candidate directly

---

### Generate disposable-job candidates for tasks that do not need a first-class interactive session. (line 1515)

Contract: Given a configured provider naming both a free and a metered model with a resolvable credential, when disposable_candidates builds candidates, one DisposableCandidate exists per named model for that provider, while no production behaviour changes.

State: **COMPLETE** — ruled 2026-09-02. Proof-only.

Production evidence:
- `crates/glasshouse/src/main.rs` — `disposable_candidates`

Regression evidence:
- `main.rs::tests::disposable_candidates_builds_one_per_configured_free_and_metered_model_1515`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| the function's final `candidates` return value replaced with `Vec::new()` | `empty-disposable-candidates` | **killed** | `main.rs::tests::disposable_candidates_builds_one_per_configured_free_and_metered_model_1515` |

> empty-disposable-candidates observed: assert!(models.contains(&"free-model-1515")) failed: models was empty

---

### Exclude candidates explicitly disabled or forbidden by user policy. (line 1520)

Contract: Given a profile disabled by user policy, that profile never reaches routing_destinations's generated set; given a destination backed by an entitlement whose rules deny its harness, SessionRouter excludes it (not merely scores it lower); while no production behaviour changes in either case.

State: **COMPLETE** — ruled 2026-09-02. Both mechanisms proven; the disposable-side job-kind/metered axes stay covered by Phase 35B's own suite and are not re-derived here.

Production evidence:
- `crates/glasshouse/src/main.rs` — `routing_destinations`
- `crates/glasshouse/src/routing/mod.rs` — `Entitlement::constraint`
- `crates/glasshouse/src/routing/session.rs` — `hard_constraint`

Regression evidence:
- `main.rs::tests::routing_destinations_excludes_a_disabled_profile_before_generation_1520`
- `tests/routing_candidates.rs::a_destination_backed_by_a_harness_denying_entitlement_is_excluded_not_scored_1520`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| .filter(|name| effective.profile_enabled(name).value) removed from the profile_names() pipeline | `bypass-profile-enabled-filter` | **killed** | `main.rs::tests::routing_destinations_excludes_a_disabled_profile_before_generation_1520` |
| Entitlement::constraint's match on self.rules.refusal(...) replaced with an unconditional Ok(()) | `bypass-entitlement-constraint` | **killed** | `tests/routing_candidates.rs::a_destination_backed_by_a_harness_denying_entitlement_is_excluded_not_scored_1520` |

> bypass-profile-enabled-filter observed: assert! failed: a profile disabled by user policy must never reach generation

> bypass-entitlement-constraint observed: assertion left == right failed: a candidate whose entitlement forbids this harness must be excluded, not merely disfavoured by scoring

Recorded scope limits — stated by the worker, not discovered later:
- the disposable-side entitlement axes (job_constraint, permits_metered in disposable.rs) are a third mechanism the recon named but the packet scoped this file's 1520 test to the interactive SessionRouter path; not separately proven here

---

### Keep at least one deterministic fallback candidate when a usable native session exists. (line 1521)

Contract: Given a user config entry attempting to disable the implied Native profile by name, EffectiveConfig::profile_enabled still answers true for it, and a Destination built from it survives SessionRouter::choose end to end against a task with no tier requirement, while no production behaviour changes.

State: **COMPLETE** — ruled 2026-09-02, for a task with no tier requirement — the boundary the line's own word *usable* draws.

Production evidence:
- `crates/glasshouse/src/config/mod.rs` — `EffectiveConfig::profile_enabled`
- `crates/glasshouse/src/routing/session.rs` — `hard_constraint`
- `crates/glasshouse/src/routing/session.rs` — `SessionRouter::choose`

Regression evidence:
- `tests/routing_candidates.rs::the_native_profile_survives_as_a_deterministic_fallback_end_to_end_1521`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| profile_enabled's `if name == NATIVE_PROFILE_NAME { return Layered::new(true, Layer::Default); }` short-circuit removed | `consult-table-for-native` | **killed** | `tests/routing_candidates.rs::the_native_profile_survives_as_a_deterministic_fallback_end_to_end_1521` |

> consult-table-for-native observed: assert! failed on profile_enabled(NATIVE_PROFILE_NAME).value, because the planted [profiles.native] enabled=false entry was now consulted

Recorded scope limits — stated by the worker, not discovered later:
- proven only for a task with no tier requirement, per the packet's own instruction — a Native profile whose resolved model's ceiling is below a classified minimum tier is not claimed to survive by this line

---

## REVIEW — the orchestrator owes an answer to each of these

This section is the point of the generator. Everything above is the
worker's facts, transcribed. Nothing below is decided.

- **1511** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1512** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1513** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1514** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1515** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1520** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1521** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).

Gates the worker ran (re-run the decisive ones yourself):
- cargo test -p glasshouse --bin glasshouse routing_destinations: ok. 5 passed; 0 failed
- cargo test -p glasshouse --bin glasshouse disposable_candidates_builds_one_per_configured: ok. 1 passed; 0 failed
- cargo test -p glasshouse --lib session::select: ok. 16 passed; 0 failed
- cargo test -p glasshouse --test routing_candidates: ok. 4 passed; 0 failed
- cargo clippy -p glasshouse --all-targets --all-features -- -D warnings: clean
- scripts/blast-radius.sh --targeted crates/glasshouse/src/main.rs crates/glasshouse/src/session/select.rs crates/glasshouse/tests/routing_candidates.rs: every traced target passed

---

# Cause 1 — CLOSED 2026-09-02 (`GH-CANDIDATE-GEN`, Amber, Sonnet high): 1517, 1518, and 1513's capability half

Two arms in `hard_constraint`, a new `HardConstraint::ProviderUnavailable`
variant with a readable reason, and two stale doc comments corrected. Three
mutations KILLED. The worker ran the full workspace suite beyond its targeted
gate and found the two pre-existing test files whose fixtures the new
exclusions invalidated — reported them with the analysis instead of editing
`FORBIDDEN` files, which is the right call, and fixed them under the ruling
recorded at 1518 below.

**1513 ticks with this package.** Its protocol/tool half was proven by
`GH-CANDIDATE-PROOFS`; its capability half is
`routing::session::hard_constraint_tests::an_established_absent_hard_capability_excludes_and_an_unverified_one_passes`,
whose excluded candidate is a gateway-backed destination built the way
`destination_backend` builds one for `BackendResource::GlasshouseGateway`.

### Exclude candidates missing a hard required capability. (line 1517)

Contract: Given a classified task and a candidate set, when the router applies its hard-constraint gate, Glasshouse excludes a candidate established to lack a required hard capability, naming the reason in the refusal — while preserving that an unverified capability stays priced, never excluded, and a candidate set with no such candidates ranks byte-for-byte as it does today.

State: **COMPLETE** — ruled 2026-09-02. The arm is asked on both gate passes; `is_adequate` refuses only on `Declared::Verified { value: false }`, so an unverified axis still passes and is priced. The refusal renders as the bare category name (no `reason()`), matching its `Protocol`/`ToolSemantics` siblings — recorded as a limit, not a gap. `tests/routing_capability.rs`'s established-absent pricing test was rewritten under this closure to assert the refusal, which is the same ordering as a fact.

Production evidence:
- `crates/glasshouse/src/routing/session.rs` — `hard_constraint (new arm calling is_adequate)`
- `crates/glasshouse/src/routing/session.rs` — `is_adequate (pre-existing, now wired into the gate)`
- `crates/glasshouse/src/routing/mod.rs` — `HardConstraint::Capability (pre-existing variant, now constructed)`

Regression evidence:
- `routing::session::hard_constraint_tests::an_established_absent_hard_capability_excludes_and_an_unverified_one_passes`
- `routing::session::hard_constraint_tests::a_candidate_set_with_no_excluded_candidate_ranks_exactly_as_before_this_gate`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| if !is_adequate(destination, &inputs.requirements) { -> if false { | `skip-state-update` | **killed** | `routing::session::hard_constraint_tests::an_established_absent_hard_capability_excludes_and_an_unverified_one_passes` |

> skip-state-update observed: panicked at crates/glasshouse/src/routing/session.rs:5191:9: assertion `left == right` failed (rejected count / chosen id no longer matched)

Recorded scope limits — stated by the worker, not discovered later:
- capability_fit's own established-absent scoring branch is now unreachable through the production gate path (an established-absent candidate never reaches scoring) — it remains directly testable and is exercised by tests/routing_capability.rs, one of whose tests (an_unverified_axis_scores_strictly_better_than_an_established_absent_one) now fails for exactly this reason; not fixed here, tests/** is FORBIDDEN FILES.
- HardConstraint::Capability's reason() stays None (unchanged), matching the pre-existing Protocol/ToolSemantics siblings; glasshouse route's refused list renders it as the bare category name rather than naming which capability axis failed. Building a fuller sentence would need is_adequate to report the failing requirement, which the packet said to use as-is.

---

### Exclude candidates whose provider is unavailable or in an authoritative cooldown. (line 1518)

Contract: Given a classified task and a candidate set, when the router applies its hard-constraint gate, Glasshouse excludes a candidate whose provider is rejected or in a provider-declared cooldown, naming the reason in the refusal — while preserving that a Glasshouse-invented cooldown stays priced, never excluded, and a candidate set with no such candidates ranks byte-for-byte as it does today.

State: **COMPLETE** — ruled 2026-09-02, with the ruling the package carried: a rejected credential or a **provider-declared** cooldown excludes; a Glasshouse-**invented** cooldown stays `provider_health`'s soft penalty (its own mutation, `wrong-source`, is the one that proves the word *authoritative* is watched). **A second ruling followed from the sweep the worker ran beyond its gate:** three `tests/tier_escalation.rs` fixtures had modelled *struggling* (lines 1559–1565) as `CredentialRejected`, which this arm now removes from the candidate set before `decide_tier_movement` runs. Ruled: a rejected credential or declared cooldown is *unavailable*; *struggling* is a candidate that can still serve and is doing badly — repeated failures, an invented backoff, or an exhausted band, exactly the other three disjuncts of `struggling()`. The fixtures were changed to repeated failures; the escalation behaviour they assert is unchanged.

Production evidence:
- `crates/glasshouse/src/routing/session.rs` — `hard_constraint (new arm)`
- `crates/glasshouse/src/routing/session.rs` — `provider_unavailable_cause (new)`
- `crates/glasshouse/src/routing/mod.rs` — `HardConstraint::ProviderUnavailable (new variant)`
- `crates/glasshouse/src/routing/mod.rs` — `ProviderUnavailableCause (new enum + Display)`

Regression evidence:
- `routing::session::hard_constraint_tests::a_credential_the_provider_rejected_is_excluded`
- `routing::session::hard_constraint_tests::a_declared_cooldown_still_in_force_is_excluded`
- `routing::session::hard_constraint_tests::an_invented_cooldown_is_priced_softly_and_never_excludes`
- `routing::session::hard_constraint_tests::an_existing_warm_session_is_excluded_when_its_provider_is_unavailable`
- `routing::session::hard_constraint_tests::a_candidate_set_with_no_excluded_candidate_ranks_exactly_as_before_this_gate`

| mutation | vocabulary | result | killed by |
|---|---|---|---|
| if let Some(cause) = provider_unavailable_cause(destination, inputs.health, inputs.now) { -> if let Some(cause) = None::<ProviderUnavailableCause> { | `skip-state-update` | **killed** | `routing::session::hard_constraint_tests::a_declared_cooldown_still_in_force_is_excluded (and two others died the same run: a_credential_the_provider_rejected_is_excluded, an_existing_warm_session_is_excluded_when_its_provider_is_unavailable)` |
| if health.declared_wait_remaining(now).is_some() { -> if health.cooldown_cause().is_some() { | `wrong-source` | **killed** | `routing::session::hard_constraint_tests::an_invented_cooldown_is_priced_softly_and_never_excludes` |

> skip-state-update observed: 3 FAILED: "an existing session must not be favoured over the gate that excludes its unavailable provider"; two `assertion left == right failed` (rejected count no longer 1)

> wrong-source observed: panicked at crates/glasshouse/src/routing/session.rs:5327:9: assertion `left == right` failed (an Invented cooldown was wrongly excluded, proving the Declared/Invented distinction is watched)

Recorded scope limits — stated by the worker, not discovered later:
- A candidate-rejected-credential fact is now excluded on BOTH hard_constraint passes (matching tool-semantics/protocol, per the packet's explicit instruction), which removes such a candidate from decide_tier_movement's input set. Three pre-existing tests in tests/tier_escalation.rs used WorkloadOutcome::CredentialRejected as their stand-in for a 'struggling' (soft) candidate and now fail: an_escalation_with_no_healthy_target_is_held_and_says_so, every_struggling_candidate_at_the_classified_tier_escalates_the_preference_one_step, two_triggers_move_one_tier_and_the_second_is_named_as_capped (file: 10 passed, 3 failed). I believe this is the correct consequence of closing the disposable/interactive asymmetry FEASIBILITY names, not a defect — a rejected credential cannot serve at all, unlike the file's own still-passing CapacityBand::Exhausted 'struggling' case — but it is a behavioral change to lines 1559-1565, outside this package's box lines, and needs a ruling plus a test-fixture fix (swap CredentialRejected for a soft-degraded reading). Not fixed here: tests/** is FORBIDDEN FILES and this crosses into another feature's evidence.

---

## REVIEW — the orchestrator owes an answer to each of these

This section is the point of the generator. Everything above is the
worker's facts, transcribed. Nothing below is decided.

- **1517** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).
- **1518** — verdict `closed`. Re-run one decisive mutation yourself, then rule (§79: a worker's packet does not bind the integrator).

Gates the worker ran (re-run the decisive ones yourself):
- cargo test -p glasshouse --lib routing::session: test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 1905 filtered out
- cargo test -p glasshouse --lib routing::: test result: ok. 236 passed; 0 failed; 0 ignored; 0 measured; 1677 filtered out
- cargo test -p glasshouse --test routing_policy: test result: ok. 37 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo test -p glasshouse --test routing_score: test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
- cargo clippy -p glasshouse --all-targets --all-features -- -D warnings: clean
- scripts/blast-radius.sh --targeted crates/glasshouse/src/routing/session.rs crates/glasshouse/src/routing/mod.rs: every traced target passed (rustdoc clean after fixing two link errors my own edits introduced); 33 full-trace targets skipped by design
- known pre-existing red gateway::session::tests::observe_exchange_scores_a_real_failover_against_the_configured_preference: passed in every run here (only flakes under --targeted's concurrent plan)
- cargo test -p glasshouse --lib --tests --no-fail-fast (full sweep, beyond what this packet requires): 120 of 122 targets green (lib: 1913 passed); tests/routing_capability.rs 3 passed/1 failed and tests/tier_escalation.rs 10 passed/3 failed, both explained above and both outside FORBIDDEN tests/**


**1513 ticks here.** Its protocol/tool half: `tests/routing_candidates.rs` (the proofs package); its capability half: `routing::session::hard_constraint_tests::an_established_absent_hard_capability_excludes_and_an_unverified_one_passes`, whose excluded candidate is gateway-backed.

**Phase 35A stands at 10 of 11.** 1519 stays refused (Cluster M, the money-spend counter).
