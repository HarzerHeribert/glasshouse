# Capability evidence — phase 2c

Split from the single evidence ledger this project used to keep at the repository root (see `docs/product/evidence/README.md` for the full index and the entry template / evidence rules that used to precede it).

### Phase 2C — first-run onboarding, and the acknowledgement `setup` had been promising (six lines, plus a 9A gap closed)

Contract: Given a first run, when the user reaches the provider step,
Glasshouse offers provider configuration as an **optional** step with a clear
"Configure now" and an equally clear "Do later" — while preserving: "Do later"
completes onboarding with no API key of any kind and leaves a Glasshouse that
works against native subscription-backed harnesses; cmux is offered only when
detected or explicitly asked for; and reopening the wizard preserves prior
choices.

State: **COMPLETE** for the six lines. The four routing-model lines stay
unchecked — each needs a routing-model configuration field, and `config/mod.rs`
was owned by a concurrent worker.

#### The gap this batch closed, which was a promise the product did not keep

`profile::Refusal::BypassNotAcknowledged` told users, verbatim, to "acknowledge
the risk once (in `glasshouse setup`)" — and **`setup` had no such step**;
`grep -rn "bypass" src/onboarding/ src/shell/` returned nothing. Phase 9A's
resolution half was built and its human half never was. That is the fifth time
on this project a declaration has not matched its use.

The step now lists the harnesses that declare a bypass and **no**
automatic-review mode — derived from the adapters
(`approvals.automatic_review` unverified, `approvals.bypass` present), never a
hard-coded list, so it cannot rot when an adapter changes. It shows each
harness's **own declared argv and description**, defaults to not acknowledged,
and writes to the **user layer only**.

#### Regression evidence

`only_a_harness_with_a_bypass_and_no_automatic_review_is_offered_bypass_acknowledgement`,
`declining_leaves_bypass_acknowledged_unset_and_the_profile_still_refused`,
`the_bypass_step_is_skippable_and_onboarding_completes_without_it`, plus the
provider-step, cmux-detection and reopen-preserves-choices tests.

#### A survived mutation that was the mutation's fault, twice

The orchestrator's own non-vacuity check on the security-relevant line took
three attempts, and the practice file's rule held exactly:

- seeding the row from `Some(true)` instead of from config — **survived**; the
  decline test does not depend on the seed.
- forcing `set_bypass_acknowledged(true)` *inside* the `row.acknowledged !=
  row.seeded` guard — **survived**, because on a decline from a fresh config
  that guard is false and the write is unreachable.
- removing the guard and acknowledging **every offered harness** — **KILLED**,
  by two tests.

So silence genuinely cannot become consent. *"A `SURVIVED` mutation is more
often a weak mutation than a weak test"* — rewritten twice before the code was
doubted, and the code was right both times.

#### Judgement the worker got right against its own packet

It **did not build a gateway configuration screen.** `BackendResource::GlasshouseGateway`
is still refused by `profile::resolve`, so a gateway step would have been a
button leading nowhere. The Provider step configures providers, and the Summary
says so plainly. It also flagged that the module's own "out of scope" doc was
stale — providers had been built by 9C/9D/9F since it was written.

---

### Phase 2C — the routing-model step, and Phase 2C at nineteen of nineteen

Lines, quoted exactly:

- "Offer routing-model configuration as an optional first-run step after
  providers have been detected or configured."
- "Offer an Automatic routing-model choice that selects the cheapest
  sufficiently fast configured resource."
- "Offer a Choose model routing-model choice for users who want to pin
  classification to a specific model."
- "Offer a Do later choice for routing-model configuration and use
  deterministic routing heuristics until configured."

Contract: Given a user finishing first-run setup, when they reach the
routing-model step, Glasshouse offers exactly three choices — Automatic, Choose
model, Do later — records the one they picked and proceeds, while preserving:
declining is a first-class outcome that leaves a working system on deterministic
heuristics; the choice is stored as a reference, never a credential; and a
configuration naming a model that later disappears degrades to heuristics rather
than failing to start.

State: **COMPLETE.** Phase 2C is **nineteen of nineteen**.

#### Three states per layer, not two

`RoutingConfig` holds `Option<RoutingModelChoice>`, and the `Option` is load
bearing. Layering needs three states per layer — "this layer says automatic",
"this layer says deterministic", and "this layer says nothing, ask the next
one". Collapsing `None` into `Deterministic` would make a project that wants
deterministic-only classification *over* a user-level `Automatic`
inexpressible, which is exactly Phase 2D's third routing option. The same shape
`IntegrationConfig::executable` already uses.

It also buys the literal reading of "Do later": with `None` skipped on
serialise, a first run that declines writes **no `[routing]` table at all** —
verified against the shipped binary, not only in a unit test.

**`Automatic` carries no payload.** Every filter Phase 34C applies to that
selection is a live condition — provider health, RPM headroom, latency, marginal
cost — and 34C is required to re-evaluate it when a provider degrades.
Resolving a winner during a first-run wizard and writing it down would freeze a
decision the map explicitly wants re-evaluated, so `Automatic` stores the intent
and 34C resolves it.

#### The defect the binary found, which every rendering test missed

Seeded with a pin whose provider no longer exists, at **80x24**, on a machine
with ten harnesses installed and two of them under a ~90-character macOS
temporary path, the Summary screen rendered **without the `Routing model:` line,
its degrade explanation, or the gateway note at all.** `render_summary` draws a
wrapped `Paragraph`, which has no scrollback: content past the bottom edge is
simply not drawn and nothing says so. Ten long executable paths, each wrapping
onto three rows, pushed everything below them off the screen.

**Every rendering test passed throughout, because every fixture used
`/usr/bin/claude`.** The variable that broke it — how long an installed
harness's path happens to be — is set by the user's machine, not by any fixture.
This is practice §17 in its sharpest form yet: an absence assertion is bounded
by the viewport, and a presence assertion is bounded by the *content* it shares
that viewport with.

Fixed at the cause: each integration is bounded to exactly one row, eliding the
path from the left so the executable's own name survives, and one separator was
reclaimed. `every_summary_section_survives_the_worst_case_at_80x24` renders that
worst case and asserts every section is present *and* that each integration
occupies one row. Two mutations put each half of the fix back; both are killed.

**A constraint is handed forward rather than left as a trap:** the Summary now
has **zero** spare rows at 80x24 in that worst case. The next batch that adds a
line to it will fail that test. That is the screen saying it is full, and the
answer is real scrolling — not deleting the assertion. The worker correctly
declined to build the scrolling as outside its packet.

#### Evidence quality

Seventeen mutations designed, run and killed; none survived. Gates run from a
deleted `target/` rather than a warm one. 734 → 760 tests on the batch's own
branch, 776 with the concurrent Phase 9G batch merged.

The worker reported **one red run it could not account for** — a single lib
failure whose name it lost by not redirecting that run's output — and recorded
it rather than burying it, alongside sixteen subsequent green runs and four
concurrent copies of the lib binary run to force port contention. A
subcontractor independently captured `AddrInUse` in
`gateway::tests::dropping_the_gateway_releases_its_port`, and the worker
explicitly declined to claim that was the same failure. **That test is tracked
as flaky**; a suite with an unexplained red is worth less than its pass count
suggests.

---
