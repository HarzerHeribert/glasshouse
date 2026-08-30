# Capability evidence — phase 34C

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 34C — line 1443 only: what "resource diagnostics" means

**No box in this phase is closed.** Phase 34C (automatic routing-model selection)
is 0 of 13, and twelve of its lines are blocked for one reason a read-only recon
established this round and a proof worker then confirmed independently:
**nothing in the shipped binary ever calls a routing model.** `RoutingModelChoice`
can be configured, resolved and rendered; no code path asks it to classify
anything.

This entry exists for **line 1443** alone, because a worker did the right thing
and refused to decide it.

Contract: Given a configured routing model, when the user inspects Glasshouse's
resource diagnostics, Glasshouse names the routing model currently selected —
while never implying a model is in use when nothing calls one.

State: **NOT STARTED.**

### The question, and why a worker was right to escalate it

`GH-PROOF-ROUTER` was asked to confirm or refuse eleven lines a recon called
closable. It closed seven, refused three, and for 1443 **reported both readings
and picked neither**:

- **TUI reading** — the Settings overlay's `RoutingRow` (`shell/mod.rs:1668-1687`,
  rendered in `shell/view.rs`) already shows the resolved routing-model choice.
  On this reading the line is closed today.
- **CLI reading** — `resources_report` (`main.rs:2233-2380`) renders no routing
  information at all; a grep for "routing" in that function's body returns
  nothing. On this reading the line is open.

That is exactly the escalation practice §33 asks for: *"a worker may hand you a
judgement; take it, and say which way you went."*

### The orchestrator's ruling: the CLI reading. The line stays OPEN.

**Three reasons, in order of weight.**

1. **A settings screen is where you choose a value, not where you diagnose one.**
   The Settings overlay showing your current selection is Phase 2D's settings
   capability — *"does using a control do something real and durable?"* — and
   those boxes are already closed on it. Counting the same rendering twice, once
   as configuration and once as diagnostics, would make the ledger say a
   diagnostic surface exists when the only thing that exists is a config editor.
2. **The map distinguishes surfaces deliberately.** Phase 41's line 1661 —
   *"Show the currently selected routing model **and its recent latency**"* — is
   the project-overview surface, and it is separately open. A map that names the
   same fact for two different views is not being redundant; it is naming two
   views. `glasshouse resources` is the third, and it is the one called
   *resources*.
3. **The line's purpose is answering "why did routing behave that way".** That
   question is asked at a diagnostic surface, next to capacity, health and quota
   — not on the screen where the user just set the value themselves.

**Closing it is cheap and is not blocked**, which is worth saying so nobody
records this as architecture: `resources_report` already receives the
`EffectiveConfig` that resolves the choice. It is a rendering line and a test.
It was not done in this batch only because `main.rs` was owned by another
worker's un-integrated diff.

**One honesty constraint on whoever closes it.** Nothing calls the routing model
(`routing::classify::classify` has one production caller, the `glasshouse
classify` CLI diagnostic; nothing constructs a `TaskClassification` outside
`#[cfg(test)]`). So the diagnostic must name the model that **would** be selected
and must not imply one is in use. Rendering a "currently selected routing model"
beside live capacity numbers, with no signal that it classifies nothing, is the
spectacle Phase 47 exists to prevent.

Missing evidence:

- A routing-model line in `resources_report` and its `api/unix.rs` twin, with a
  test entering through the shipped binary — the precedent is
  `provider_discovery.rs::a_planted_gateway_reading_now_reaches_the_shipped_binarys_report`.
- Wording that distinguishes *configured* from *in use* while line 1425/1426
  remain open.

---

# Lines 1431, 1433, 1443 — closed 2026-08-30; 1437 open, 1438 and 1440 refused

Package `GH-AUTO-ROUTING-MODEL`. **Six lines against one mechanism** — the
first package sized by mechanism rather than by line, after batch 55 measured
0.77 boxes per package (§87).

## 1431 — CLOSED. It was unevidenced, not unbuilt

State: **COMPLETE**

The chain was already production-wired: `main.rs::classify_with_routing_model`
reads `effective.routing_model_resolution()`, and its
`RoutingModelResolution::Automatic` arm calls `automatic_classification_model`
→ `automatic_classification_choice` → `DisposableRouting::choose`.

**The evidence that makes it non-vacuous asserts on the wire.**
`classification_call::automatic_routing_asks_the_resource_the_routing_policy_chose`
configures two free providers at one canned endpoint and puts the *second*
candidate first in `routing.free_resource_order` — an input **only `choose`
reads**. The assertion is that the request body names `zeta-model` and does not
name `alpha-model`. Anything that reached a model without going through the
policy would name the wrong one.

## 1433 — CLOSED, and it needed production code

State: **COMPLETE**

Killed by
`provider_discovery::an_unhealthy_resource_is_not_the_one_automatic_routing_would_select`.

**Limit, stated by the package and carried here:** the health pool reaches only
**free** candidates. `choose` never asks health about a metered one.

## 1443 — CLOSED, the CLI half

State: **COMPLETE**

Built as this entry already ruled: `glasshouse resources`, not the Settings
overlay. `render_routing_model` is appended by `resources_report` in the same
shape the `PROBES` block uses, so `provider/resources.rs` is untouched.

**The rendering is honest about what it does not know**, which is why it
closes:

    in use    nothing yet. `glasshouse classify` is the only command that asks
              a routing model; no other Glasshouse decision calls one, so this
              names a choice rather than a habit.

**Why it was open until today** is recorded above: `main.rs` belonged to
another worker's un-integrated diff. It closed because §77 let this package
co-edit `main.rs` rather than queue behind it.

## 1437 and 1438 — the same vacuous qualifier, and both refused on it

**1437** *"prefer currently free candidates **after capability and latency
requirements are satisfied**"* — **OPEN.**
**1438** *"prefer local candidates **when they satisfy the configured latency
and quality requirements**"* — **REFUSED.**

The free preference in 1437 is **real**: `DisposableRouting::choose`'s free
loop returns before the metered loop. But `RouterLatencyMs` — a real, layered,
validated config value — has **exactly two consumers, both in the settings
overlay** (`shell/mod.rs:2230`, `:2628`). **No routing decision anywhere reads
it.**

So closing either line would be **closing a qualifier by absence**, which is
precisely why 1455 and 1456 were un-ticked the same morning. 1437 is `open`
rather than `refused` because its preference half is genuinely implemented and
one producer — a latency reading that reaches `choose` — would close it. 1438
is `refused` because locality is not represented on a candidate at all.

## 1440 — REFUSED

*"Avoid a scarce premium subscription session as the classifier when a cheaper
adequate routing resource exists."* See the package's report for the decisive
`file:line`; the reserve gate does not distinguish a subscription-backed
candidate in the way this line requires.

## A finding outside this package's diff, recorded so it is not lost

`routing/disposable.rs`'s `choose` doc cites
`tests::scoring_never_reorders_the_existing_free_selection` as holding the
invariant that scoring never reorders the free-tier selection. **That test does
not exist** — `grep` returns one hit, the citation itself. The invariant is real
and the code holds it (the free loop returns on the first available candidate in
`FreePreferences::arrange`'s output and never consults `score` for ordering),
but **nothing would fail if a future change broke it**. That is §49's
"doc-comment lines counted as call sites" in a new costume, and it is a Green
packet for somebody.

---

# Line 1434 — closed 2026-08-30; 1441 and 1442 built but NOT wired

Package `GH-ROUTING-STICKINESS`.

## 1434 — CLOSED

State: **COMPLETE**

*"Filter automatic candidates by minimum requests-per-minute headroom when
known."*

The signal already reached `DisposableRouting::choose`; what was missing was an
**eliminating** consumer. `GH-ROUTING-FILTERS` established the distinction and
it is the reusable part: the RPM figure was read in exactly one place —
`score()`'s ranking contribution — while every place `choose` *removed* a
candidate ignored it. A candidate at 0% headroom was ranked last and never
excluded, which is a different claim from the line's word *"filter"*.

`has_no_known_headroom` (`routing/disposable.rs:285`) is now called inside
`choose` immediately after `apply_hard_constraints`, before the free/metered
split, and `choose`'s ordered doc comment was renumbered to name it as step 2.

**The load-bearing part is the honesty rule, and it was reused rather than
invented.** `has_no_known_headroom` returns `false` for `None`: a candidate
nothing is known about is never removed. That is the **identical** rule
`CandidateCapacity`'s own doc and `score()`'s `None` arm
(`routing/disposable.rs:826-830`) already stated for the scoring path — this
package applied it to elimination rather than writing a second rule that could
drift. Eliminating on absence would turn *"we have no telemetry"* into *"this
provider is full"*.

Mutation: make the elimination step also drop candidates with an absent
reading — **KILLED** by
`an_absent_capacity_reading_never_eliminates_a_candidate`, and by every other
test in the target, since the mutation eliminates every candidate the file's
helpers build without a capacity reading.

## 1441 and 1442 — OPEN, and this is a deliberate Cluster B

State: **SCAFFOLDED**, and it must not be read as anything more.

The mechanism they need **is built and tested**: an on-disk
`RoutingStickyCache` (`provider/telemetry.rs`, following `GatewayQuotaCache`'s
shape — no migration) and
`DisposableRouting::choose_for_automatic_classification`
(`routing/disposable.rs`), with tests covering the retained pick, the health
re-check, and a missing or corrupt record deciding fresh.

**Nothing in the shipped binary calls it.** `main.rs::automatic_classification_choice`
still calls `choose` directly. The packet forbade `main.rs` — it was held by
another worker — and the worker correctly stopped at the boundary and named the
insertion point rather than taking an unclaimed file.

**This is the exact shape that un-ticked five boxes earlier the same day**: code
that is correct, tested, and never run. It is recorded here as `SCAFFOLDED`
rather than quietly left to look finished, and **1441/1442 must not be ticked
until the call site lands.** The follow-up is one call site in
`automatic_classification_choice`.

**The invariant the wiring must preserve**, already enforced by the built code
and its tests: *a retained pick is never returned without re-checking its
health.* Stickiness must not outlive the healthiness it was predicated on.
