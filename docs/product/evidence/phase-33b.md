# Phase 33B — Reliability-adjusted agent performance

**0 of 14 closed at the time of writing. This file exists because the phase had
no evidence-ledger entry at all** — `discover.py --phase 33B` reported *"no file
paths found"* — and nobody had ever established what blocks it. A read-only
census (`GH-RECON-33B`, 2026-09-02, Sonnet high) did that.

**The fourteen lines reduce to four root causes, and the causes are the package
boundaries** (practice §87). Two are refusals; two are packageable and one of
them was dispatched the same night.

---

## Cause A — the relay-path body-parsing wall. REFUSED; do not package.

**Lines: 1347, 1348, 1349, 1350, 1351, 1352, 1355, and 1354 in part.**

`gateway/ingress.rs:36` states the relay is *"structurally incapable of carrying
a body"* and `:647` declines to parse the response stream. `main.rs:~8612`
states `routing_observations`' token columns have had **no writer since
migration 11** for the identical reason. **Both re-verified word-for-word
against current source during the census** — neither has rotted.

TTFC, TTFT, decode throughput and rounds-per-minute are therefore not merely
unwired: **the producer absence is already documented and mutation-tested in
production.** `shell/state.rs:695-696` and `shell/view.rs:1017-1021` both say
verbatim that those five measures *"have no producer on this gateway at all,"*
and `shell/view.rs`'s `no_fabricated_columns_appear_in_the_route_evidence_table`
asserts none of those strings ever render. `cluster-b.py` finds no
`effective_ttfc`/`reliability_adjusted_latency`-shaped symbol — **these are not
built-and-unwired functions; the symbols do not exist**, which is a stronger
finding than Cluster B's shape.

This is the register's **Cluster L / row P1b** blocker — the same wall holding
Phase 33A's 1331-1334 and line 1263. Any package here fails Phase -1 on link 1.
**Successor: none. It needs the `ingress` design ruling first.**

**1354 is half-built and worth knowing about.** `FailureClass` already has
production-constructed, production-consumed `EmptyCompletion` and `StreamAbort`
variants (`gateway/session.rs:1178-1196`), classified from HTTP status and
framing — **no parsing wall applies to those two**. What is missing is *"unusable
tool calls"* and *"apparently successful but non-actionable turns"*, which need
body/tool-call content. The line cannot tick on two of four, but Cause A's
eventual package should reuse those two variants rather than rebuild them.

---

## Cause B — routing scoring weights are compile-time constants with no config surface. PACKAGEABLE.

**Lines: 1357 (in part), 1358.**

`QUOTA_PRESSURE_WEIGHT` (`routing/session.rs:948`),
`CLASSIFICATION_PREFERENCE_WEIGHT` (`routing/disposable.rs:404`), and
`HEALTH_FAILURE_PENALTY` / `HEALTH_PENALTY_FLOOR` / `HEALTH_UNAVAILABLE_PENALTY`
are all `const`. Nothing under `src/config` names a routing score weight —
contrast `routing.capacity_band_thresholds` (`routing/pressure.rs:63`), which
**is** user-configurable and is the pattern to copy.

**1357's term-preservation clause is already satisfied**: `Contribution` and
`RoutingExplanation` (`routing/mod.rs:444,481`) preserve every score's exact
inputs, terms and evidence string. Only the weight-configurability clause is
open. **1358** ("treat the OX gateway scoring model as evidence and a
configurable starting policy, not a universal constant") has no symbol of that
name anywhere in-tree and is the same underlying gap: treating the weights as a
starting policy requires them to be configurable first.

**Successor, named and ready:** add config fields for the routing score weights
(start with `HEALTH_FAILURE_PENALTY`/`HEALTH_PENALTY_FLOOR` and
`CLASSIFICATION_PREFERENCE_WEIGHT`, layered like `capacity_band_thresholds`),
and thread the configured value into `provider_health`
(`routing/session.rs:2267`, called at `:4634`) and the disposable scorer
(`routing/disposable.rs:1747-1828`). **The one test:** set a non-default weight
via config and assert the resulting `Contribution::magnitude()` for an otherwise
identical routing decision changes from the constant to the configured value —
proving the override reaches a real decision, not just a struct field.

---

## Cause C — already implemented in production, never ticked. PACKAGEABLE, and dispatched.

**Lines: 1353, 1359.** No production change needed; both need a test that
watches what already ships.

- **1353** — `provider_health` (`routing/session.rs:2267`, production call site
  `:4634`) already applies `consecutive_failures as f64 * HEALTH_FAILURE_PENALTY`
  floored at `HEALTH_PENALTY_FLOOR`. That is a genuine **additive per-failure**
  penalty folded into the routing decision, not a boolean.
- **1359** — the coarse fallback is not a special case, it is **the only path
  that has ever run**: Cause A means structured events never arrive.
  `main.rs:~4468-4471` already writes a `NewObservation` with
  `ROUTING_LATENCY_PURPOSE` and `with_timing`, and `gateway/session.rs:470-477`
  records dispatch/completion/outcome/failure-class/failovers/retries on every
  real exchange.

**CLOSED 2026-09-02 by `GH-COARSE-FALLBACK` (Green).** No production line
changed — both capabilities already shipped; what was missing was a test that
watches each.

| line | mutation | result | killed by |
|---|---|---|---|
| **1353** | price the penalty flat, ignoring `consecutive_failures` | **killed** | the additive-and-bounded test |
| **1359** | make `summarize` skip rows whose structured fields are all `None` | **killed** | the coarse-observation test |

> 1353 observed: *"two consecutive failures (-0.3) must price worse than one (-0.3) — an additive penalty, not a boolean"*
> 1359 observed: *"coarse timing alone must produce a duration aggregate, not a skip"*

The 1353 test asserts the penalty is **additive and bounded** — two failures
strictly worse than one, and clamped at `HEALTH_PENALTY_FLOOR`. A test that only
checked "failures make it worse" would not distinguish an additive penalty from
a boolean one, which is the distinction the line's own words turn on.

**Phase 33B now stands at 2/14.** Cause A's eight lines remain with the
`ingress` ruling, Cause B's two are packageable (successor named above), and
Cause D's two stay Cluster P.

---

## Cause D — restraint lines with no forbidden mechanism in the tree. REFUSED.

**Lines: 1356, 1360.** Cluster P/Q shape — *"name the code path that could do
the forbidden thing; if you cannot, the line is not closeable and saying so is
the finding."*

- **1360** (*never infer precise TTFC or token timing from terminal text*) — the
  census searched `harness/`, `shell/`, `pty/` and the gateway/routing tree for
  any terminal-text-to-timing inference and found **nothing**. `pty/mod.rs`
  exposes raw bytes; nothing parses them for timing or token boundaries anywhere
  in production or tests. The restraint passes **vacuously**, which is exactly
  the shape that un-ticked 1455/1456.
- **1356** (*do not compare TTFC across materially different tool requirements
  unnormalized*) — nothing computes or compares TTFC at all, so there is no
  comparison to be non-compliant. Filed here rather than under Cause A because
  its resolution differs: **even once TTFC exists, this line stays open until a
  comparison or normalization code path exists to test.** That is a second,
  later gap.

**Successor: none.** Re-check 1356 after Cause A's `ingress` ruling, since a
producer may introduce the first comparison path it could bind to. **1360 has no
dependency on Cause A** — closing it would require someone to *build* terminal-
text timing inference, which is the outcome the line forbids, so it is most
likely a standing restraint rather than a future package.

---

## A note on the restraint-line ruling, so it is not misapplied here

A ruling landed the same night (`phase-29.md`, line 1152) that **restraint lines
are mutation-proven by violating the restraint** — the defect a restraint line
forbids *is* an addition. That ruling closed 1152 and it is correct.

**It does not rescue 1356 or 1360.** The 1152 restraint had *both* things it
separates actually built (a memory store and a checkpoint store, each with a
live command), so the violation was a real defect a real test could catch. Cause
D's lines restrain a mechanism that does not exist, so the "violation" would be
building the forbidden feature in order to forbid it. **The distinction is
whether the restrained thing exists**, and it is worth stating because the two
shapes look identical from the map line alone.

---

# Cause B — lines 1357 and 1358 CLOSED 2026-09-02 (`GH-SCORE-WEIGHTS`)

`routing.score_weights` now carries the four constants
(`HEALTH_FAILURE_PENALTY`, `HEALTH_PENALTY_FLOOR`, `HEALTH_UNAVAILABLE_PENALTY`,
`QUOTA_PRESSURE_WEIGHT`) through the same field/getter/setter/layered-resolver
path `capacity_band_thresholds` already uses. An absent config resolves to
today's constants, so a run with no configuration scores byte-identically —
pinned by an acceptance test rather than asserted.

| mutation | vocabulary | result | observed |
|---|---|---|---|
| scorer reads the module const instead of the resolved weight | `configured-weight-ignored` | **killed** | `left: -0.3, right: -0.5` — the configured value never reached the scorer |
| alter one shipped default | `default-silently-changed` | **killed** | `left: -0.35, right: -0.3` — absent-config magnitude no longer matched the pinned constant |

The first is the load-bearing one: it is exactly the defect where a config field
exists, round-trips, and changes nothing.

**1358 has no independent mutation, and that is recorded rather than papered
over.** The line asks that the shipped weights be *"implementation evidence and
a configurable starting policy rather than a universal Glasshouse constant"*.
Its behavioural half — that they are overridable, therefore not universal
constants — is exactly what 1357's mutations prove. Its remaining half is
framing, and lives in the doc comments on `ScoreWeights` and
`ScoreWeightsConfig`. **A future audit that judges a doc comment insufficient
evidence for this line should un-tick it; the cost is one line and this
paragraph is the pointer.**

Recorded limits: the acceptance tests call `provider_health` and
`quota_pressure` directly rather than driving a full `SessionRouter::choose`.
`routing/disposable.rs`'s `CLASSIFICATION_PREFERENCE_WEIGHT` was deliberately
left out of scope; the worker reports the surface extends to it without
redesign, which is the next packet's Phase -1.

**Phase 33B now stands at 4/14.** Cause A's eight remain with the `ingress`
ruling; Cause D's two stay Cluster P.

## REOPENED 2026-09-02 — both lines un-ticked; the "recorded limit" was the defect

`GH-AUDIT-WAVE79`, an independent Sonnet audit of the five ticks in `9f513d9`
and `40ae89d`, found that **`SessionRouter::with_score_weights`
(`routing/session.rs:4223`) had zero call sites of any kind** — its only
occurrence in the crate was its own definition. `session_router()`
(`main.rs:3914`), the sole production constructor of `SessionRouter`, chained
`with_override … with_price_table` and never the weights, so every router the
shipped binary built scored with `ScoreWeights::default()` regardless of what a
user configured. The config layer (`EffectiveConfig::score_weights`,
`config/mod.rs:5576`) was correct; nothing read its output.

Three independent proofs, all reproduced by the orchestrator before un-ticking:

- `grep -rn 'with_score_weights('` — one hit, the definition.
- A tripwire test through the real constructor
  (`main.rs::tests::a_configured_score_weight_reaches_the_real_session_router`):
  a `health_failure_penalty: -50.0` override against a default of `-0.3`, one
  observed failure, real `session_router()` both times — **identical totals**
  (`left: -0.4, right: -0.4`). Red on `a79b276`.
- A mutation on `SessionRouter::choose` itself — `&self.score_weights,` →
  `&ScoreWeights::default(),` — **SURVIVED** the entire `routing_policy` suite
  (`37 passed; 0 failed`), which holds 1357's own acceptance tests. They call
  `provider_health`/`quota_pressure` by hand, so they cannot see whether the
  router uses its field.

**The "recorded limit" above — *"the acceptance tests call `provider_health`
and `quota_pressure` directly rather than driving a full
`SessionRouter::choose`"* — was the load-bearing defect filed as a footnote.**
Practice §36's question was not asked: does a caller *exercise* the policy? The
two recorded mutations were genuinely KILLED, by tests on a path the shipped
binary never takes. This is the twelfth wrongly ticked box in the project and
the shape is the same as the other eleven, with one twist worth recording:
`cluster-b.py` could not have found it, because its row filter
(`prod==0 and test>0`) hides a symbol with no callers of any kind. **A grep for
the builder's name is the check; the script is not sufficient for a
zero-caller symbol.**

1358 falls with 1357: its behavioural half rests, by this entry's own words,
on 1357's mutations proving an override reaches a real decision, and they did
not.

## RE-CLOSED 2026-09-02 — one line in `session_router()`, and the tripwire is the acceptance test

**Production:** `main.rs::session_router` now chains
`.with_score_weights(effective.score_weights().value)` — the resolved
project-over-user-over-default `ScoreWeights`, read in the one constructor
every real ranking path goes through (`glasshouse route`, `launch`, the
control door). Nothing else changed; the config layer was already right.

**Regression:** `main.rs::tests::a_configured_score_weight_reaches_the_real_session_router`
— the audit's tripwire, unchanged in substance. It builds two routers with the
real `session_router()` from a default `EffectiveConfig` and from one carrying
`health_failure_penalty: -50.0`, ranks the same one-failure destination
through `SessionRouter::choose`, and asserts the totals differ. Red on
`a79b276`; green with the line.

    test tests::a_configured_score_weight_reaches_the_real_session_router ... ok
    test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 63 filtered out

| mutation | vocabulary | result | observed |
|---|---|---|---|
| the wire hands the router `ScoreWeights::default()` instead of the resolved value | `wire-ignores-config` | **killed** | `panicked at crates/glasshouse/src/main.rs:15614:9` — identical totals with and without the override |

This is the mutation the original entry could not run: it is the defect
itself, reproduced on the path the shipped binary takes. The two earlier
mutations still stand for what they prove — the scorer reads the weight it is
handed, and the defaults are pinned — and this one proves the handing.

**1358** re-ticks with it, on the same reasoning as before: the shipped
weights are overridable by configuration *on a real decision*, so they are a
starting policy and not a universal constant. Its framing half is unchanged
(the doc comments on `ScoreWeights` and `ScoreWeightsConfig`).

Gate, hand-run because the wave-80 sweep held the tree's gate lock:
`cargo test -p glasshouse --bin glasshouse` — `65 passed; 0 failed`;
`cargo check -p glasshouse --tests` clean; `cargo clippy --all-targets
--all-features -D warnings` clean.

**Phase 33B stands at 4/14 again.** The lesson is in the reopen entry above;
the practice note is §36 applied to a footnote, and the tool note is that
`cluster-b.py` cannot see a zero-caller symbol.

---

# A gate gap this package exposed, and it is the orchestrator's error

`GH-SCORE-WEIGHTS` could not run `cargo check --tests` at all when it started:
`routing/session.rs:4916`, inside that file's own `#[cfg(test)]` module, called
`FreePool::adopt_observed` with **four** arguments after `GH-CADENCE-CROSSING`
made it take **five**. The worker fixed it with one added `None`, correctly
filed it as `packet_errors` (pre-existing, not introduced by its own work), and
confirmed it against `git show HEAD:`.

**It was pushed to `main` in `9f513d9` and the gate did not catch it.** The
mechanism matters:

- `cadence-crossing` changed the signature and updated the **production** call
  site (`main.rs:2747`) and every `GatewayHealthReading` literal it rippled
  into — but not this one, which is a *different* function in a *different*
  file's test module.
- `coarse-fallback` was authored against a tree without the new signature, so
  neither worktree was broken on its own. **The breakage exists only in the
  merge** — precisely the cross-patch interaction batching into one
  `integrate.sh` call is supposed to surface.
- **The targeted gate ran integration-test binaries** (`--test routing_policy`
  and friends). Those compile the library **without** `cfg(test)`, so a compile
  error inside the lib's own test module is invisible to them. Nothing in that
  gate compiled `--lib` with tests enabled.

**The cheap fix is one command in the blocking gate**: `cargo check -p glasshouse
--tests` before `integrate.sh` reports success. It is seconds when warm and it
closes the whole class — any signature change whose stragglers live in a
`#[cfg(test)]` module.

Two rules already in this project would each have caught it independently and
neither was applied: *"once a grep names a file, run its tests"* (§79), and the
integrator's own obligation to read what `integrate.sh` prints. The gate is
mechanisable and the discipline is not, so mechanise it.

## Line 1350 — CLOSED 2026-09-02 (`GH-TOOL-ROUNDS-ON-TRANSLATED`, Amber, Sonnet high): rounds per minute of serving time, printed as what it is

The census above filed 1350 under Cluster L — *TTFC/TTFT/decode-throughput/
rounds-per-minute symbols do not exist in the tree at all* — behind the
relay's parsing wall. The wall stands for relayed rows; the translated seam
counts tool-use blocks as they pass (`phase-33a.md`, 1334), and the reader
is a sum.

**Contract.** Given routing observations that carry `tool_rounds`, when
`glasshouse routing-cost` renders a group, Glasshouse prints the rounds
begun, the repairs, and rounds per minute over the group's summed exchange
durations — *not recorded* for a group with no counted row — as an
outcome-adjacent measure printed beside the latency figures and never folded
into a score (1355's own rule).

**Production evidence.** `routing/evidence.rs::PurposeConsumption`
gains `tool_rounds`, `repairs`, `serving_seconds` (`SUM`s, `None` on an
all-`NULL` group) and `tool_rounds_per_minute()`; `consumption_by_purpose`'s
SQL and row reader; `main.rs::render_routing_cost` → `render_tool_rounds`
(`tool rounds : N begun, M repairs, R.RR/min over Ss served`).

**Regression evidence.**
`routing_cost::two_seeded_rows_print_summed_rounds_repairs_and_a_real_per_minute_rate`
(rounds 2 and 1, repairs 1 and 0, 60 s each → *3 begun, 1 repairs, 1.50/min
over 120s served*; an untimed group *not recorded*). **Mutation**
`per-second-not-minute` (the `× 60` dropped) KILLED — *0.03/min* where
*1.50/min* was owed.

**Recorded limit.** A group whose rows carry `tool_rounds` but no
dispatch/completion pair renders *not recorded* by construction; not driven
directly. The serving time is seconds-resolution until `GH-STREAM-TIMING-MS`
lands (`design-decisions.md`, *Millisecond offsets on the routing row*).

State: **COMPLETE**. Phase 33B stands at 5 of 14; 1347, 1348, 1349, 1355
wait on the millisecond columns (their readouts are `GH-STREAM-TIMING-MS`'s),
1351/1352 on the term after them, 1353/1359 packaged separately, 1354 half,
1356/1360 Cluster P/Q.

## Lines 1347, 1348, 1349 — CLOSED 2026-09-02 (`GH-STREAM-TIMING-MS`, Red, Opus 5 high): millisecond offsets since the send, and four figures on four lines; 1355 stays OPEN on its *effective TTFC* clause

**The mechanism** (design note *Millisecond offsets on the routing row —
Cluster G's second column set*). Migration 25 adds four nullable
`INTEGER CHECK (col IS NULL OR col >= 0)` columns to `routing_observations`
— `first_byte_ms`, `first_token_ms`, `first_tool_call_ms`, `completed_ms` —
in migration 24's shape with migration 11's `CHECK` (a quantity with an
arithmetic floor, not a vocabulary word; the worker's reasoning, accepted).
Each gateway path takes its own monotonic `Instant` immediately before its
send — `ingress::forward` one line above `agent.run(request)`,
`translate::serve` one line above `agent.run(outbound)` — and
`ingress::millis_since` is the one saturating conversion. `FirstEvents::note`'s
clock closure now yields `(seconds, Option<ms>)` and stamps both; the document
case reuses the first-byte reading for both token offsets, as the seconds
do. The seconds columns are written exactly as before. `Exchange` carries the
four offsets, `None` on refusal; `record_routing_observation` writes them
through four builders. Readers: `RoutingObservation::duration_ms` prefers
`completed_ms` and falls back to the seconds difference;
`consumption_by_purpose`'s three means prefer the ms column per row and
carry `*_ms_sample_count` beside the seconds count so the readout can say
*(seconds only)*; `PurposeConsumption::decode_tokens_per_second` is output
tokens over the decode span (`completed_ms − first_token_ms`) summed over
rows carrying all three, `None` when either half is unmeasured — never a
rate from a second-resolution denominator. `render_routing_cost` prints
`TTFC (tool-using responsiveness)`, `TTFT (generation responsiveness)`,
`decode tokens/s (model serving)` and the tool-rounds line each on its own
line, *not recorded* per figure, *(seconds only)* per group.

**Contracts.** 1347: given a translated exchange with a tool-use block,
Glasshouse records the milliseconds from the send to that block and prints it
first and labelled as the responsiveness measure for tool-using work. 1348:
TTFT is its own column, its own line, its own label, never derived from or
folded into TTFC. 1349: decode tokens per second is a model-serving figure
on its own line, absent rather than zero, and in no score.

**Regression** (`tests/gateway_timing_ms.rs`, 4/4, a real socket with a
translated and a relayed exchange and planted pauses; `tests/routing_cost.rs`,
12/12, the shipped binary; `--lib gateway` 210/210; `--lib routing::evidence`
62/62; `--lib database`'s migration test): a translated stream with a 1.2 s
pause before the first real token and another before the tool-use block
measures `first_byte_ms < first_token_ms`, the two gaps ≥ 1000 ms, completion
after the tool call, and the seconds columns unchanged; a relayed exchange
with the pause planted in the *provider's* body measures first byte and
completion and neither token offset; a refused exchange measures none; a
version-24 database migrates, reads back four `None`s, and a new row carries
them; migration 25 undoes cleanly (whole-schema byte comparison); no line of
the readout carries two of the four figures (a structural test over the
rendered report).

**Mutations, 5/5 KILLED** (worker's `mutate.sh`, restores byte-identical):
`zero-at-handoff` (the `Instant` moved 1.2 s earlier — `first_byte_ms=1211`
where the planted pre-send wait must not be inside it), `wall-clock-offset`
(the offset floored to whole seconds — the value set two wall readings can
produce; a stand-in the worker named as such, and the independent verifier's
objective 5 rules on its faithfulness), `fallback-dropped` (`duration_ms`
without the seconds fallback), `headline-collapsed` (two figures on one
line), `migration-missing-check` (a column without its `CHECK` accepts −1).

**Packet errors, all five right**: `gateway/mod.rs` needed no change (the
design note's zero is each path's own send, not a hand-off instant threaded
down); twelve literal version pins, not eleven (`session/store.rs` ×4, and
`tests/session_context.rs` spells one across three lines); seventeen rollback
lists undo migration 24 and every one now leads with 25's four drops (the
migration ripple this project's memory records); a client-side pause on the
relay is *inside* the relay's time to first byte, because `ureq` reads the
harness body lazily — a real behavioural difference, recorded; the label
reading. Scope overflow: `session/store.rs`, `routing/burn.rs`,
`routing/interactive.rs` (test literals gaining four `None`s),
`tests/session_context.rs`, `tests/routing_session_column.rs` — all the
migration's own ripple.

**The one red, attributed twice**:
`database::tests::concurrent_first_bootstraps_serialize_on_one_database`
fails under `--lib database` on the worker's tree and identically on a
detached unmodified worktree at its base (§91), and passes alone — the
load-sensitive bootstrap race recorded against batch 87.

**Independent verification** (Red tier): `GH-VERIFY-STREAM-TIMING-MS`, an
Opus reader in the worker's worktree — rollback completeness enumerated,
the two send sites, the `CHECK` and saturation, the readers' unit mixing,
the stand-in mutation's faithfulness; its verdict is recorded below this
entry when it lands. The Windows VM leg and the Linux leg trail on the
merged tree.

**Recorded limits** (the worker's): one protocol pair driven
(anthropic-messages → openai-chat), the mechanism pair-agnostic; *primary*
proven as the label and the ordering, not a score; the support-work rows
`record_extraction_observation` writes keep their seconds (a Green successor:
that producer holds no `Instant`); a mid-stream client disconnect keeps the
offsets already noted, untested either way; macOS only here.

**1355 — OPEN by ruling.** The line names five figures — raw TTFC, effective
TTFC, TTFT, throughput, rounds per minute — and four exist and are proven
separate; *effective TTFC* exists nowhere in this tree. It is
`GH-RESPONSIVENESS-TERMS`'s (1351/1352, dispatched next), whose objective 4
prints it on its own line and closes this one. The worker returned `open`
for exactly this reason and was right to.

State: **COMPLETE** for 1347, 1348 and 1349. Phase 33B stands at 8 of 14.

**Independent verification landed** (`GH-VERIFY-STREAM-TIMING-MS`, Opus 5
high, read-only, 2026-09-02 late night): **ACCEPT**. It enumerated the
rollback sites itself, confirmed each path's `Instant` sits after the
harness body is read and immediately before the send, that no wall reading
feeds an offset, that a refused exchange and the relay's token offsets are
`None`, that the `CHECK` and the saturating conversion leave no negative or
overflow path, and that the readers never mix units within a mean; it
attributed the one red by an interleaved A/B/A/B against the base commit
(stronger than the worker's own, and agreeing). Two non-blocking findings,
both doc comments: `render_latency_ms`'s comment claims the count printed
beside a mixed group's mean is the count of measured rows, but the readout
prints the seconds counts and never the `*_ms_sample_count`s — and a mixed
group (some rows with offsets, some without) is untested. **Successor:**
`GH-RESPONSIVENESS-TERMS` prints `<ms> of <n> measured` beside each mean and
tests the mixed group (its objective 4). Ticked on the verdict.
