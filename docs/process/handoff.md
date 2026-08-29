# Glasshouse implementation handoff

> This describes how Glasshouse is built, not what Glasshouse does. Nothing
> here is a product requirement. Capability requirements live only in
> `docs/product/capability-map.md`.

Last updated: 2026-08-29 (Europe/Berlin)

## Checkpoint — 2026-08-29, batch 49 landed: 734 / 1280 (57%)

**Four Opus team leads, each running its own subcontractors, 40 capability
lines in flight.** Thirteen closed. Eight phases fully closed today (2D, 3, 12,
18, 19, 21A, 32, 40).

### What the leads produced

| lead | phase | closed | the finding worth keeping |
|---|---|---|---|
| worker-loop | 15 + 16 | **7** (733–739) | `api::unix::spawn_session` was the **one launch path in the binary installing no lifecycle hooks** — an orchestrator's own worker could finish a turn and leave no trace |
| placeholders | blocker-resolution | **3** (1288, 1291, 1319) | the root-cause framing paid, measurably — see below |
| capacity | 32A | **1**, 9 refused | caught an error the orchestrator wrote into its own packet as "established, do not re-derive" |
| memory-ladder | 21E + 21G | **0**, 12 refused | found a **live project-isolation defect** on the read side |

### The measurement that justifies packaging blockers by root cause

Batch 49 took five lines that four separate phases had refused for "no
production caller", found they shared **one hardcoded struct literal**, and
closed three. **1291 closed as a side effect of fixing 1288** and was on no
plausible task list.

The proof is not rhetorical. The mutation disabling
`evaluate_reserve_spend`'s imminent-reset branch is recorded in `phase-32f.md`
as **SURVIVED** — run by the orchestrator, fifteen tests genuinely running.
Re-run against the same function, unchanged, with only the *caller's* input made
real: **KILLED**. Nothing about the branch moved; it now decides something.

This is §83, and `docs/process/refusal-register.md` is the artifact that makes
it repeatable: every refusal with its missing link and one column — is the
missing thing inside this repository?

### The defect nobody was looking for

`MemoryStore::with_status` selected `WHERE status = ?1` with **no project
predicate**, and had three production callers — `glasshouse memory revalidate
--list` and both reads behind the project-knowledge panel, *the panel whose
keyboard route this same batch closed as line 234*. Against a planted foreign
row the shipped binary printed another project's memory body verbatim.

Phase 21G had hardened five `UPDATE`s because a `get(id)?` guard is one line a
future edit can silently drop. **The reads were never covered — and a listing
query has no guard to drop, because it takes no identifier. The `WHERE` clause
is the entire boundary.**

### Four new practice sections, all paid for

**§80** four ways a mutation lies — *now five*, the fifth being a KILLED
delivered through a fixture's own timeout: a **true** verdict credited to
assertions that never ran, which passes all three of §80's existing checks.
**§81** never mark a recon's claim "established, do not re-derive" — the
orchestrator did, and it would have returned a closeable capability as
impossible. **§82** a lead pays when the work is code, not judgement.
**§83** gather refusals by root cause.

### The process failure of this batch, and it was the orchestrator's

**The continuity watch was never armed.** The session ran to 81% context and
was told by the user, not by the machine. `.agent-runtime/continuity-watch.sh`
exists for exactly this. Arming it is now the first instruction in
`CONTINUATION.md`.

## Checkpoint — 2026-08-29, batch 48 landed: 721 / 1280 (56%)

Five capabilities, **four phases finished** (3, 12, 19, 21A). Four editing
workers plus two recons. Green on all three platforms.

### What landed

| worktree | line(s) | note |
|---|---|---|
| `p3-memory-view` | 234 | new overlay; **Phase 3 at 12/12** |
| `auto-checkpoint` | 802 | **Phase 19 at 14/14** |
| `api-events` | 701 | **Phase 12 at 8/8** |
| `reserve-reset` | 1292 | 1291 refused — see below |
| — (orchestrator) | 862 | no code; **Phase 21A at 12/12** |

### The ruling I got wrong, and how it was caught

Batch 48's own triage section in `phase-32f.md` reported that mutating
`reset_urgency`'s distant branch changed nothing, and concluded that nothing
watches distant-reset conservatism. **The mutation was irrelevant.**
`reset_urgency` is not on the reserve gate's path at all — its only caller is
capacity scoring, and `evaluate_reserve_spend` compares the thresholds inline.

A SURVIVED verdict there meant *"wrong mutation site"*, not *"unwatched code"*.
The `test result:` line was honest, the target was right, and the site was
wrong — a third distinct way to be fooled by a mutation, after a zero-match
filter and a target that lacks the killing test.

The worker sent to satisfy that bad acceptance bar read the call graph instead
of obeying it, ran diagnostics on the real gate, and reported both. Mutated
correctly, the distant branch is killed by three tests and **1292 closes**.

**1291 stays open with an exact blocker.** Disabling the imminent branch changes
nothing, because the tail falls through to `Allow` whenever
`cheaper_adequate_resource_exists` is false and the sole production caller
hardcodes it false. The imminent `Allow` and the default `Allow` are the same
decision with different reason strings, so "more permissive" cannot be observed.
Same blocker as 1288–1290, not a separate one.

### A packet premise that was wrong, again mine

`GH-P3-MEMORY-VIEW`'s FEASIBILITY claimed `MemoryKind` has seven variants
including `Invariant`, and wrote the acceptance test against it. It has six;
`Invariant` belongs to `MemoryAuthority`. I had read one enum's range into the
next. The worker led with the correction, refused to add a seventh kind — which
would have needed a migration for a variant the map never asks for — and built
the view kind-agnostically, which satisfies the contract as written.

### Windows: green, and the one failure is the known 33% flake

`--windows-vm` run alone: build and MSRV 1.88 pass; `test (windows) / test`
failed once on
`session::api::tests::interrupting_through_the_api_is_recorded_as_machine_initiated`,
a 45-second PTY timeout in a file no worker touched. **This handoff already
records that test measured at 33%** (1 of 3 on a pre-batch tree). Re-run per the
two-run rule: **1456 passed, 0 failed.** Not a regression.

**Run `--windows-vm`, not the bare gate.** The bare form prints "Windows was not
exercised", which is a flag not passed rather than a fact. And do not pipe the
run through `tail` — batch 48 did, and threw away the failure detail it needed.

### Also closed without code

**862** — `memory/extract/authority.rs`'s own header ends *"That is Phase 21A's
last line."* Nobody had followed it through. The word doing the work is
*Require*, enforced at the parse boundary: `declared_authority` is not an
`Option`, so a memory declining the distinction does not parse.

### Leads left in `.agent-runtime/CONTINUATION.md`

**372** — `phase-9a.md:482` says it is blocked because `grep 'fn score\|Score'`
is empty. It is not empty any more. §14 still applies: nothing selects among
launch profiles, so the blocker is gone and the work is not done.
**290** — blocked on external evidence; `--help` has failed twice and the entry
says to read a different artifact next.
**327/310** — need a `CHECK`-constraint widening SQLite cannot do in place.
Design, not Sonnet.

## Checkpoint — 2026-08-29, batch 47 landed: 716 / 1280 (56%)

Five capabilities closed, three commits pushed, **two phases finished**
(Phase 32 at 12/12, Phase 40 at 9/9). Six workers dispatched, four editing and
two read-only, plus a seventh recon and the freebuff side task.

### What landed

| worktree | lines closed | note |
|---|---|---|
| `api-routing` | 1680 | first production caller of a producer nothing had ever asked a question |
| `extract-switch` | 1791 | tests only; no production code — the switch already worked, nothing proved it |
| `mem-settings` | 190 | the section existed and lied; the old test asserted the defect |
| `session-record` | 1646 | migration 12; finishes Phase 40 |
| — (orchestrator) | 1183 | no code: a caveat in `phase-32.md` had expired |

Recons: `ledger-sweep`, `ledger-sweep-2`, `session-link` (tiered 1646 before it
was dispatched), `freebuff`.

### Rulings owed and paid

**1377 and 1541** were ticked with `PARTIAL` entries and had been inherited
through three orchestrators. Both ruled: **tick stands**, both recorded as
closed with the reasoning, so a fourth reading is not needed. 1377's entry also
carried a statistic six ticks stale (Phase 33 is 7/15, not 0/15) — corrected;
the claim it supports survives.

**1183** — `phase-32.md` already said "CLOSED, with the enumeration-caller
caveat" and named what would discharge it. Two surfaces now do. Closed on the
existing mechanism, no code written.

### Three lines refused at the Phase −1 gate, and why that is the win

- **1745, 1746** (Phase 46's last two). `CMUX_*` is read **only** in
  `integrations/mod.rs` for presence detection, and MCP appears only in doc
  comments. No cmux-metadata path reaches project-scope validation and there is
  no MCP surface. The evidence entry's blocker is **current, not stale** — a
  grep for "cmux|mcp" hits doc comments and would have sent a worker to write
  tests proving a surface that does not exist.
- **1239** — its consumer belongs to routing phases still at zero. Same shape
  as 1661, which a previous orchestrator killed.
- **1681** — no recommendation producer exists to inspect;
  `routing/disposable.rs:469` is the only chooser and it chooses by executing.

### Two leads for the next batch, from the sweeps

- **1293 / 1288–1294** — `evaluate_reserve_spend`'s "no production caller"
  blocker is gone (`routing/disposable.rs:568`), but that caller is on the
  *disposable* path and the lines may mean the interactive one. Needs a read.
- **566 / 569** (Phase 9J) — `phase-49.md:352`'s "nothing ranks candidates at
  all" was made false the next day by `routing/interactive.rs::score_candidate`.
  Whether the prior is *positive-initial* and whether a warm session can
  outweigh it are unchecked.
- **1203 / 1209** — the config field named as the blocker now exists, but a
  different real blocker replaced it: nothing wires `QuotaOverride::budget`
  into a `CapacityState`. Stale *reason*, not a stale open state.

### Tooling: five defects, four of them mine, all fixed in `a84424b` and after

1. **`.agent-runtime/` is gitignored, so a packet does not exist in a worker
   worktree.** All three editing workers stopped on it at once; two burned
   ~100k tokens exploring first. `new-worker.sh` now resolves the packet path
   to an absolute one. The delivery proof could not catch this — the prompt
   landed; the path inside it was unusable.
2. **Same bug, other direction:** `new-packet.sh` emitted a relative
   `REPORT TO`, which lands a report in the worktree where the watch cannot see
   it. Now absolute. It still happened once *with* an absolute path, because a
   worker shortened it — worth watching.
3. **`worker-watch.sh` false idle on a thinking worker.** §28's growth guard is
   present and cannot help a tests-only worker that has written nothing. The
   token counter now folds into the fingerprint; it fired correctly within
   minutes of landing.
4. **Acking a worker ends its watch.** Ack was used to silence a stale nag on a
   worker that was still running, and its watch died. Ack only what is finished.
5. **`evidence_from_report.py` and `new-packet.sh` disagreed about the facts
   block.** Four workers were asked for one and all four invented a flat
   `key: value` form; the tool refused every one with `missing top-level lines`.
   The schema lived only in the consuming script's docstring, which workers are
   told not to read. `new-packet.sh` now emits it. The script was also not
   executable despite `CLAUDE.md` invoking it directly.

Two more, not fixed: `mutate.sh` anchors to its own checkout, so a worktree
mutation needs that worktree's own copy (`./scripts/mutate.sh`); and
`integrate.sh` takes worker **names**, not paths, which `CLAUDE.md`'s
`<worktree>...` does not convey.

### The packet defect two workers found independently

Four packets wrote `--test 'cargo test -p glasshouse --test X'`. `mutate.sh`
prepends `cargo test` itself and treats everything after `--test` as separate
arguments, so the quoted form is **a single test-name filter matching zero
tests** — a SURVIVED verdict indistinguishable from a real one. That is §68's
trap inside the tool that exists to guard against §68. Both workers caught it by
reading the `test result:` line, exactly as instructed. Unquoted form:
`--test -p glasshouse --test X`.

### One rescue

`session-record` edited the **main checkout** rather than its worktree, because
the packet gave it absolute main-checkout paths for its packet and its input
recon and it stayed there. Caught by a `git status` during an unrelated
mutation. Work captured as a patch, applied into its worktree, main restored
with `git restore -- crates/`; the resulting diffstat matched the worker's own
`+207/-12` exactly, so nothing was lost. **Do not point a worker at absolute
main-checkout paths for anything but reading.**

## Checkpoint — 2026-08-29, batch 45 landed: 710 / 1280 (55%)

**Green on all three platforms.** macOS and Linux through `scripts/ci-local.sh`;
Windows for real on the ARM64 VM, 3/3. Six parked worktrees integrated, 23 boxes
closed, six commits plus this one.

### What landed

| worktree | lines closed | note |
|---|---|---|
| `health-cache` | 1311, 1321, 1322, 1324 | the "one consumer" Phase 33's wall needed |
| `phase-41-overview` | 1657, 1658, 1659, 1660, 1663 | Phase 41 now 14 of 15; only 1661 open |
| `proof-router` | 1413–1418, 1424 | tests-only; 7 of 11 candidates survived, 4 refused |
| `handoff-checkpoint` | 1638, 1639, 1640, 1642, 1643, 1644, 1645 | new `phase-40.md` |
| `codex-hooks` | none — wiring only | PreCompact/PostCompact now requested |
| `classify-caller` | none by design | mechanism kept, **wiring refused** — see below |

New ledger files: `docs/product/evidence/phase-34b.md`, `phase-40.md`.

### The three rulings this checkpoint owed, and how they went

**1. `handoff-checkpoint`'s six lines: accepted, and upgraded from behavioural to
mutation-proven.** The worker closed them on a shipped-binary integration test
but could not mutate `main.rs` or `session/**`, because its own packet forbade
the files the evidence lives in (§78, my predecessor's defect). **A worker's
packet does not bind the integrator.** I ran the four mutations it could not:
wrong target harness (killed at `handoff_lines.rs:314`), a full `Debug` replay in
place of the bootstrap prompt (`:326`), closing the source session on launch
(`:362`), and a second `store.create` per launch (`:268`). All killed; `main.rs`
restored byte-identical after each. The six lines are closed on mutation proof,
not on judgment.

**2. `classify-caller`'s wiring patch: refused, for a reason one level below the
one it found.** The worker was right that `disposable_extraction_model` is called
before the job text exists, and wrote a `main.rs` patch reordering the path so the
chunk is built first. I am not applying it. The chunk is a **transcript**;
`classify_heuristically` is documented as classifying a **request**; and the tier
it produces feeds `evaluate_reserve_spend`, whose distant-reset branch releases
protected premium reserve only for `WorkloadTier::Heavy`. Wiring the chunk would
let a cheap extraction job spend the reserve because the conversation it is
summarising happened to contain demanding-sounding words — the tier would vary
with **conversation topic** rather than with the job's own demand, inverting what
the gate protects.

That is the fifth link failing on **semantics**, not plumbing: the input varies,
but with the wrong thing. Worth adding to the Phase −1 vocabulary — "does it
vary?" is not enough; the question is "does it vary with the thing the consumer
is actually measuring?"

The mechanism is kept (every call site passes `None`, reproducing the old fixed
`Leaf` exactly) and `new_for_request`'s doc comment now states all of this, so the
next agent does not wire it. It is correct and ready for a `JobKind` carrying a
real user request — `Classification`, `Reranking`, `Evaluation` — none of which
has a production caller.

**3. Map lines 1377 and 1541 (ticked but PARTIAL): still open, not ruled on.**
Inherited from my predecessor and deliberately not resolved in this batch. **This
is the first thing the next orchestrator owes.**

### What the gate caught, and the rule it argues for

**`session::select::tests::codex_hooks_are_written_where_codex_reads_them` failed
on both macOS and Linux.** `codex-hooks` added two events to `REPORTED_EVENTS`;
that test hardcodes the expected list. The worker's blast-radius grep (§69) *did*
name `session/select.rs` — it read the file and concluded nothing there asserted
an event count, which was wrong.

**The cheap rule: once a blast-radius grep names a file, *run* that file's tests
instead of reading them.** `cargo test --lib session::select` would have caught it
in seconds. Reading a file to decide whether it is affected is the step that
failed; running it is not a judgment call.

The test it broke is the *better* evidence of the two, because it asserts on the
`.codex/hooks.json` Codex actually loads rather than on the adapter's constant.
Fixed by adding the two events to its expectation.

### A Windows failure that was a flake, attributed with two runs

`session::api::tests::interrupting_through_the_api_is_recorded_as_machine_initiated`
timed out on the first `--windows-vm` run — in a file **no worker in this batch
touched**. Per §40 a FAIL is attributed with two runs, not one; the second run
passed 3/3 including that test. Recorded as a load-sensitive flake, not a
regression. No run was killed, so §72's orphan hazard does not apply.

### Two ledger entries that had outlived their own truth

Both found in one batch, both the same failure mode:

- **1663** — said "nothing in `crate::routing` reads `premium_reserve_percent`".
  Batch 42 wired `metered_models` and made that false; nothing re-read the entry,
  so it went on telling three later packets the box was blocked.
- **1657–1660** — said `NOT STARTED, blocked` on Phase 32A/32B. Those shipped.
- **phase-33's "Correction" paragraph** — still described wiring quota into
  `main.rs` as future work, after it was already wired.

**The rule, now recorded in both files: when a batch removes a blocker, grep the
ledger for entries that named it.** An evidence entry that records a blocker does
not expire when the blocker does.

### Worker packet corrections: thirteen consecutive rounds

`phase-41-overview` corrected 1663's stale feasibility note and was right.
`classify-caller` refuted its own packet's central premise and was right.
`proof-router` closed 1424 through a different production caller than its packet
cited, and refused four lines its packet expected it to close.

One integrator correction in the other direction: `proof-router` argued 1427 and
1457/1459 from "`classify_heuristically` has zero callers outside its module".
That grep was too narrow — `main.rs:144` calls `routing::classify::report` for
`glasshouse classify <text>`. The refusals survive in a sharper form (**the
classifier's only production caller is a manual CLI diagnostic, not a routing
decision**), and the same miss was caught independently by `classify-caller` in
the same batch.

### Next exact action

1. **Rule on map lines 1377 and 1541** — ticked but PARTIAL, inherited, unresolved.
2. **1641** is the cheapest real line left in Phase 40, and it is an
   implementation packet, not a patch: `Handoff` gains a `memory` field, which
   breaks two `Handoff` literals (`main.rs::checkpoint_command`,
   `api/unix.rs::request_checkpoint` — the second is a caller nobody had
   recorded). Verified breakage, not assumed: `cargo check` fails `E0063` at both.
3. `.agent-runtime/WAVE-2-PLAN.md` still holds the gated leads from six recons —
   treat its remaining candidates as ~65% reliable, since `proof-router` tested
   eleven and only seven survived.
4. **Convergent co-editing (§77) has still not been used in anger.** Batch 45's
   partitioning was fully disjoint — verified at integration, zero file overlap
   across all six worktrees — so the hard case remains untested.

## Checkpoint — 2026-08-28, batch 43 landed: 687 / 1280 (53%)

**Green on all three platforms** — local 13/13, 3855 passed / 0 failed,
`--windows-vm` **3/3 first try**, no flake. Two implementers plus a read-only
recon. Four boxes: 1762, 1764, 1314, 1315.

### The round's finding is about the gate, not the product

**`validate_round.py` check 3 had been verifying nothing.** It matched only lines
whose *first* character is `☐` — the shape the map uses. Every packet dresses the
quote as `- **1311** ☐ …`, so it saw **zero box lines** and reported PASSED, for
at least four rounds.

A worker found it by hand: **five of its packet's nine quoted Phase 33 lines were
wrong at the same line numbers**, two with their topics swapped. Fixed in
`5971e9f`, two regression tests, verified both ways — seven mismatches now caught
in the bad packet, zero false positives on six good ones.

**That is practice §68's defect inside the gate built to catch it**, and the fifth
instance of one shape: `blind` not zero (§54), `unknown` not a session id (§67),
*"0 tests matched"* not *"tests passed"* (§68), your own recommendation not the
user's decision (§70), and now a check that matched nothing reporting PASSED.
**A mechanical gate needs its own non-vacuity check** — this project
mutation-tests its product code as routine and had never asked whether its
*process* checks could fail.

### Two closures declined, deliberately

1320 and 1323 rest on existing tests, 1323 partly on a source-scan proof of
absence — §14's trap, and map 1748's history. They need a mutation check.
**Declining costs a round; ticking wrongly costs the ledger's credibility.**

### Phase 33's thirteen open lines are one wall

`ResourceHealth` is real and written for **every** exchange, paid included — but
**nothing outside the `gateway` module can observe it**. `free_pool()` has zero
callers in `src/`; the one router reading `FreePool::is_available` builds an
always-empty pool by its own doc; `api/unix.rs:331` says no live health signal is
exposed. **1311/1321/1322/1324 need one consumer, not four packages.**

### The recon's "cheapest way into Phase 51" was two-thirds wrong — gated before dispatch

The Phase 51 recon recommended populating `purpose`, `cost` **and three timestamp
fields** that `gateway/session.rs::record_routing_observation` (`:278-321`) never
sets, calling it six lines of caller-side work with no migration. **Verified
against source before any packet was written:**

- **`purpose` is genuinely real and unset.** `NewObservation.purpose:
  Option<String>` (`evidence.rs:247`) defaults to `None`; the writer sets route,
  harness, quota context, timing and outcome, and never touches it — though the
  gateway does know the job's nature. **This part holds.**
- **`cost` cannot be filled honestly.** `ObservedCost` needs pricing, **Phase 32G
  is 0/10**, and map line 1305 requires unknown pricing be treated as *unknown*
  rather than assigned a number.
- **The three timestamps are structurally unavailable, not merely unset.**
  `evidence.rs`'s own module header (`:42`) calls `first_byte_at` /
  `first_token_at` / `first_tool_call_at` *"not merely unavailable to this
  round's partition — structurally unavailable to the ingress design itself"*,
  because a pass-through gateway cannot read a response body. **The same blocker
  that keeps five of map line 1762's seven columns unrenderable** — recorded in
  `phase-47.md` this same batch.

**So the package is `purpose` alone.** Still real, much smaller, and it must not
be dispatched as six lines. **A recommendation from a recon is a lead, exactly as
a recommendation in a handoff is** — the Phase −1 gate applies to both, and this
one was caught by re-reading two files.

### Phase 51 is blocked on a primitive, and it is the user's decision

All 37 lines blocked, one shared cause: **Glasshouse cannot count occurrences of a
decision or effect over time.** The schema is built for current state. Every
feature-on-vs-off comparison the alpha directive asks for is an event count. The
cheap way in is caller-side (populate `purpose`/`cost`/timestamps that
`record_routing_observation` never sets — six lines); the memory cluster needs a
**new event-log table**, a migration and Red tier. Recorded in
`docs/product/design-decisions.md`.

## Checkpoint — 2026-08-28, batch 42 landed: 683 / 1280 (53%)

**Green on all three platforms** — local gate 13/13, 3811 passed / 0 failed,
`--windows-vm` **3/3 on a clean VM**. Three Sonnet implementers plus a second
read-only recon. **Three boxes closed, two returned as premise-invalid**, and the
returns cost less than the closures.

### The reserve gate was never a policy gap

Four checkpoints called 1293/1550 blocked on policy. It was **candidate
generation**: `disposable_candidates` hardcoded `Cost::Free`, so
`disposable.rs:558`'s filter was always empty, `evaluate_reserve_spend` never
ran, and a reserve contribution correct since batch 36 could never appear. Fixed
with `ProviderConfig::metered_models`, symmetric to `free_models()`.

**`metered_models` IS the control — there is deliberately no boolean above it.**
Empty is the off state, populated is on. A second switch would be a source of
truth able to contradict the list. The user's decision governs the default:
metered use is *permitted*, so nothing gates it beyond having configured a model.

### Two boxes returned, and the packet was at fault both times

**1762/1764.** The table needs one row per *observed* identity. `EvidenceLedger`
exposes only `record`/`recent`/`summarize`, and `ObservationQuery` requires
`provider` and `model` as `&str` with no wildcard — **there is no way to ask which
identities exist.** The packet's five links all held for a *lookup*; none asks
whether data can be **enumerated**. Now practice **§71**.

**The worker could have shipped it** by reconstructing identities from config —
rendering *configured* routes beside *observed* ones as if both were measurements,
inside a phase called *observability without spectacle*. It stopped instead. **A
fabricated row and a real row render identically, so no test would have caught it.**

### The Windows hang was a flake, and the second failure was the integrator's

Run 1 hung on `gateway::conformance::an_unreachable_provider_...` — silent 458s
at 2.7% VM CPU. Run 2 then failed **all three legs before any test ran**:
`Could not replace CI source tree ... used by another process`. **`TaskStop`
kills the local driver and nothing on the VM** — the hung binary and its
`cargo.exe` were still holding the lock. Killed over ssh; run 3 clean, the test
passing, the standing 33% flake passing too. Practice **§72**.

**That is now two hang-flakes in `gateway::conformance`**, both driving real
sockets, joining the 33% `session::api` interrupt flake and the 1-in-37
`pty_smoke` `SIGABRT`. Four unowned Windows flakes; none is anyone's job.

### Recon-2 corrected recon-1 in both directions

See the batch-41 section. The transferable part: **a recon worker is cheap enough
to run twice** — two cost ~$7 and between them refuted a standing architectural
claim, caught an inert input in a live packet, and prevented a misdirected
package.

### Next

1. **Phase 33 ticking pass** — 1314/1315 closable with zero new files, per
   recon-2. **1315 carries a proof gap**: it wants a test asserting rendered reset
   text, and the orchestrator found **no literal `resets` string in `src/`** —
   what exists is `seconds_until_reset()` and a JSON field. Settle the wording
   before writing the packet.
2. **Phase 47 1762/1764** — one small additive `EvidenceLedger` method listing
   distinct observed identities, then the overlay. Both close together.
3. **Phase 51 (Evaluation hooks, 0/37)** — now strategically important: the
   user's alpha needs A/B measurement of feature usefulness, and every line of
   that phase is a *"measure how often…"* question.

## Checkpoint — 2026-08-28, batch 41 landed: 680 / 1280 (53%)

**Three Sonnet workers in parallel on provably disjoint partitions. Seventeen
boxes** — Phase 21E 914-918/924, Phase 35B 1541/1548, Phase 25 1098-1104/1106/1107.
Local gate **13/13**, 3789 passed / 0 failed.

### Windows: 3/3 on the second run, and a NEW flake found and attributed

**Do not read this as an unqualified three-platform green.** The first
`--windows-vm` run on this exact tree **failed one test**:

    gateway::conformance::a_real_forwarded_exchanges_rate_limit_headers_reach_the_gateway

The second run of the **identical tree** passed 3/3, that test included. Same
tree, two runs, two answers is §40's definition of nondeterminism, and three
independent checks agree it is not this batch's:

- it **passed** on batch 40's Windows run;
- batch 41 touched **no** `gateway/**` or `provider/**` file at all;
- `SessionRouting::observe_quota_headers`, the seam it exercises, lives in
  `gateway/session.rs:245` — untouched — and the only `impl` this batch added to
  `routing/interactive.rs` is a `#[cfg(test)]` `ObservationSource`.

**It is a newly-observed flake with no prior record**, and it has the same shape
as the other two: a **real TCP exchange** driven through the real accept loop
against a `FixtureUpstream`, which on Windows is where port binding and
connection timing bite. Observed rate so far: **1 failure in 2 runs**, which is
far too small a sample to quote as a rate — batch 35 needed nine runs to
establish the `session::api` flake at 33%, and quoting a rate from two runs is
exactly the error that cost four Windows runs then.

**It joins the standing Windows flake debt**, beside the 33% `session::api`
interrupt flake (which **passed on both runs here**) and the 1-in-37 `pty_smoke`
`SIGABRT`. Nobody owns any of the three. The cost is now visible: this batch
spent an extra full Windows round trip to attribute one, and every future
orchestrator pays that toll before it can trust a red Windows result.

### A second recon corrected the first in both directions, for ~$3.24

Recon-1's Phase 33 verdicts contradicted themselves (one read `CLOSABLE` while
its own consumer field read `MISSING`). A second read-only worker re-verified
them, and the orchestrator re-checked each correction against source.

**The expensive one it prevented:** recon-1 said line 1313 had no consumer
because *"no resource-health surface reads `EvidenceLedger`"*. That is false —
`gateway/session.rs:420` constructs `ObservedEvidenceSource` and its own comment
calls it *"Phase 9J and Phase 33A's one production consumer"*. The verdict stays
blocked, but the real gap is that the **latency fields have no field on
`ObservedEvidence` to land in**. A package built on recon-1's reason would have
added a redundant shell overlay instead of a duration field and a two-line
mapping change.

It also found **1314 and 1315 closable** (recon-1 never checked
`main.rs::resources_report` or `api/unix.rs::resource_capacity`, the shipped
`glasshouse resources` command), and that `ResourceHealth`'s writer covers
**paid** assignments too, not just the free pool.

**The lesson is not "recon is unreliable" — it is that a recon worker is cheap
enough to run twice.** Two of them cost ~$7 together and between them refuted a
standing architectural claim, caught an inert input in a live packet, and
prevented a misdirected package. That is the cheapest verification tier this
project has.

### The round exists because the user corrected the cost model

Batch 40 ran one worker and justified it by the weekly window. The user's
correction: *"sonnet workers are incredibly cheap, use two or even three — you
are the expensive part."* The numbers were already here — a Sonnet package
~$7.53, this orchestrator ~$17.50 by integration, a long predecessor pane ~$62 —
and **every cost-per-box figure in the measurements ledger counted worker compute
and omitted the orchestrator**, which is both the larger term and the *fixed* one.
Gating, packets, review, mutation re-runs, evidence, map, commits and the platform
gate are paid once per **round**, not once per package.

**Gating three packages took one orchestrator turn.** That is the whole answer to
"but the gate is expensive".

### The third worker was not an implementer, and it paid for itself twice

Only two implementation packages were gateable on disjoint partitions. Rather
than drop to two, the third was **read-only recon**: run the Phase −1 five-link
check across five unassessed phases and report verdicts. 14 minutes, **~$3.90**.

- **It refuted a claim two checkpoints carried.** Phase 47's routing debug views
  were recorded as blocked by a cross-process boundary. `gateway::start_if_required_with_telemetry`
  is a direct call from `main.rs:534` — **same OS process**. They are blocked on
  something fixable instead: `_gateway_guard` is *"never read again, only held"*,
  and the `RoutingExplanation` goes only into a `tracing` field.
- **It caught an inert input in a live worker's packet.** `ContextState` is
  `Unknown` on 100% of real rows, so map line 1545 fails the fifth link. Verified
  independently and **relayed mid-round** — the Phase 9J failure caught
  prospectively for the first time rather than after three rounds and ~$39.
- Its recommendation (Phase 25) became the third implementation worker.

**When only two implementation packages are gated, make the third a recon
worker.** It cannot be premise-invalid by construction.

### Three boxes were proposed COMPLETE and declined

- **35B 1542** — names *"observed success **and** reliability"*, and
  `ObservedEvidence::reliability` is `None` on 100% of real rows. **The identical
  standard by which 1545 was refused the same round**, where `ContextState` is
  always `Unknown`. Consistency is the point: an input absent across all real
  data cannot support a box that names it.
- **1541** — closed, with its narrowing recorded rather than hidden: the line
  names a launch-profile dimension `ObservedEvidenceSource` states plainly the
  ledger stores nothing for.
- **Phase 25's 1105** — not attempted by instruction; a second interaction shape.

### The ladder changed shipped ordering and broke a Phase 21B test

`thin_and_well_proven_decisions_of_different_authority_classes_keep_bm25_order`
asserted two decisions of different authority classes keep BM25 order. **Map line
918 requires exactly that reordering.** Both cannot hold.

Neither was discarded. The old test's real subject is the *thin-decision demotion*
rule and its scoping, which the ladder does not contradict — it just could no
longer isolate it, because its pair straddled two rungs. The pair now sits on one
rung (`Preference` and `Idea`) where the ladder is neutral and the thin rule is
again the only thing that could reorder them.

**Found by the integration gate on both platforms, not by the worker, and that is
a packet defect**: the packet's test list omitted `memory_provenance`. **A change
to global search ordering can break any test that asserts an order** — scope its
verification by blast radius, not by which target names match the feature.

### A mutation survived, and it is recorded rather than worked around

Removing `Action::OpenProjectKnowledge`'s `Err` arm kills no test: it lives in
`shell::run()`, the real event loop with a live terminal, which **nothing in this
codebase unit-tests**. Phase 41 has the structurally identical untested arm. The
state layer *is* covered; only the run-loop wiring is not. Closing it needs a way
to force `ProjectMemory::open` to `Err` — new test infrastructure that would close
Phase 41's gap at the same time.

### Also found: a Phase 9J mechanism that has been inert since it shipped

`CONFIDENT_AT_OBSERVATIONS = 5` scales confidence continuously, but
`MIN_SAMPLE_FOR_SUMMARY = 5` means the ledger never returns a count below 5 — so
**5 real samples and 5000 score identically**, and that curve is reachable only
through a test double. Recorded, not repaired: fixing it means choosing which
constant moves, which is a policy call.

## Checkpoint — 2026-08-28, batch 40 landed: 663 / 1280 (52%)

**Green on all three platforms on this exact tree** — local gate **13/13**
including the ubuntu clippy leg, `--windows-vm` **3/3 for real on the ARM64 VM**,
3723 tests passed / 0 failed, zero slow-test warnings, and the standing 33%
`session::api` flake did **not** fire.

One Sonnet, three boxes, two commits. `3492459` reworded map line 1748 and closed
it; `35fb3ac` closed Phase 21G 948/949/950 and hardened five SQL statements.

### The round began by discovering a second orchestrator in the same checkout

`CONTINUATION.md` said `main` was `696b368`, clean, no workers live. It was
`1233ed9` by the time the round was gated: a **predecessor Opus session was still
alive in this same working directory** and pushed two commits mid-round. Both
were docs-only and additive, so nothing conflicted — but that was luck, not
design. Its own heartbeat watch exists precisely to nudge an idle orchestrator
back into work, which in a shared checkout means a second integrator.

**Verify `git log`, not just `git status`, against the checkpoint's stated
commit.** The predecessor had finished its handoff and gone idle; it was closed
(`cmux workspace close`), which the user's recorded correction authorises once a
successor is running.

### `glasshouse memory revalidate` — the loop the binary advertised and could not close

`memory challenge` has always printed *"it will not be returned as current until
the challenge is resolved"*, and nothing could resolve it. `MemoryStore::reaffirm`,
`::supersede`, `::set_status` and `::with_status` had **zero non-test callers**.
Phase 21F created `NeedsReview` memories and gave them no exit.

949 (the four outcomes), 948 (an automatic reviewer refused on a high-impact
**or unclassified** memory, reusing Phase 22's own gate) and 950 (`--list
[--limit N]`, `with_status`'s first production caller) are closed. No migration —
every outcome was already a `MemoryStatus` variant.

### The hardening, and why it took two mutation runs rather than one

Five `UPDATE memories … WHERE id = ?1` statements carried no `project_id`,
protected only by a leading `self.get(id)?`. **All five guards were present —
there was no live defect** (the previous checkpoint said six statements; it is
five, checked one by one). Each now also carries `AND project_id = ?N`, guards
kept.

The integrator re-ran the decisive mutation **both ways**, and only the pair
proves anything:

- guard removed, hardening kept → isolation test **passes** — the scoped `WHERE`
  now carries the protection alone. That is what the change bought.
- guard **and** hardening removed → isolation test **FAILS** — so the test is not
  vacuous and would have caught the original shape.

**A hardening change that makes a mutation survivable needs two runs, not one.**
One run showing "still passes" is indistinguishable from a test that cannot fail.

### A box was proposed closed and declined

**951** (*"avoid automatic revalidation work when the memory is not about to
affect any current task"*). The worker's only evidence was that no sweep code
exists — its own report called that *"a structural/negative check, confirmed by
reading the diff rather than a runtime test."* The SDLC's rule decides it:
regression evidence must **fail if the behaviour were removed**, and nothing
would. Same shape as line 1748, un-ticked once for exactly that reason.

### Two corrections the integrator made to the worker's diff

- `MemoryStoreError::ReviewRequired` read *"so its **conflict** may not be
  resolved automatically"* — right for Phase 22, misleading for a revalidation
  where no conflict exists. The worker reused the variant as instructed and
  **escalated the wording instead of redesigning it**, which was the right call.
  Generalised to *"so it may not be settled automatically"*, correct for both
  callers, confirmed in the running binary.
- The packet's own `cargo test … project_isolation` matches **zero tests** and
  still prints `test result: ok`. The worker caught it and used `--test`.
  **Sixth consecutive round a worker corrected its packet and was right.**
  Recorded as practice **§68**.

### Line 1748 was reworded, not forced

The user chose rewording over building a deletion command. It now names what its
test actually guards — physical separation in
`paths.rs::RuntimePaths::project_state_dir`, which holds against a deletion by
anyone. The old §33 argument is preserved in `phase-46.md` rather than deleted,
and a future deletion *command* still owes its own test through that caller.

### Phase 47's "cross-process blocker" was wrong, and a $3.90 recon worker settled it

Two checkpoints recorded that Phase 47's routing debug views (1757, 1766) were
blocked because the routing explanation lives *"in the gateway's process"* while
a debug view runs in another. **Refuted.**
`gateway::start_if_required_with_telemetry` is a direct call from `main.rs:534`
and `main.rs:1076` — **same OS process** as the shell. (`glasshouse api serve` is
genuinely separate; so is memory extraction, line 1769.)

They are still blocked, but on something fixable: the `Gateway` is bound into
`_gateway_guard` (`main.rs:2113`, *"Never read again, only held"*), and the
`RoutingExplanation` is rendered **only** into a `tracing` field at
`gateway/session.rs:437-460`, never captured into a structure. `BLOCKED: no
consumer`, not an architecture problem.

Underneath it, a finding that narrows the box even after a consumer exists: **the
first assignment at session launch never scores candidates at all** —
`profile::apply_gateway` calls `InteractiveRouting::assign`
(`interactive.rs:423`), a plain wrap. An explanation exists only for mid-session
failover or migration.

And one that killed a stretch line already in flight: `ContextState` is `Unknown`
on 100% of real rows (`with_context_state` has zero non-test callers), so map
line 1545 fails the fifth link. Relayed to the live worker mid-round.

**The method is the point.** This was a read-only worker running the Phase −1
five-link check across five phases — 14 minutes, ~$3.90, and it corrected a
standing claim in two checkpoints. That work does not need an Opus orchestrator's
context, and doing it there is how a round's fixed cost gets spent on gating
instead of building.

### Next, and one of them is a lead that needs checking before it is trusted

1. **The metered-quota package.** The user's decision is recorded in
   `docs/product/design-decisions.md` (`1233ed9`): a background job may spend
   metered quota, bounded by **proportion** to the task. `main.rs::disposable_candidates`
   builds only `Cost::Free`, which is why 1293 and 1550 cannot close.
   **Gated this round:** `DisposableRouting::for_support_work` **is** called
   (`main.rs:1233`), so `MeteredUse::Permitted` is live in production — the gap
   is candidate *generation*, not the policy. It must not fake the proportion
   (Phase 32G is 0/10; line 1305 says treat unknown pricing as unknown).
2. **Line 542 — checked this round, and it is not a defect.**
   `DisposableRouting::for_glasshouses_own_run` has **zero non-test callers**,
   which looks alarming and is not. `main.rs:1233`'s `for_support_work`
   (`MeteredUse::Permitted`) is reached from `disposable_extraction_model` <-
   `report_hook` (`main.rs:1177`) — the post-turn memory-extraction trigger,
   which is *ordinary support work* and exactly what the decision permits to
   fall back to metered. `for_glasshouses_own_run` has no caller because
   **Glasshouse ships no automated evaluation or test-run feature**; its
   behaviour is mutation-tested and the mechanism is correct. It is moot today
   anyway, since only `Cost::Free` candidates are built. **The risk to carry
   into item 1:** once metered candidates exist, nothing structurally stops a
   future eval/test runner from reaching for `for_support_work` by mistake.
3. **Phase 21E line 924** (stronger review before superseding invariants) is the
   cheapest remaining memory box: `supersede` now has its first production caller,
   and `require_reviewed_for_high_impact` is the gate to extend to it.
   **Line 925 needs a migration** — there is `superseded_by` but no reason column.
   Red tier.

## Checkpoint — 2026-08-28, batch 39 landed: 658 / 1280 (51%)

**Green on all three platforms on this exact tree** — local gate 13/13 including
the ubuntu clippy leg, `--windows-vm` **3/3 for real on the ARM64 VM**, 1358 lib
tests, zero slow-test warnings, and the 33%-rate `session::api` flake did not
fire.

Two Sonnets on disjoint partitions, thirteen boxes. `failure-domain` closed
Phase 33C 1371/1372/1375/1377/1378 and Phase 35B 1547 (`8e14f4f`);
`memory-retrieval` closed Phase 21F 929/931/933/935/936/937/938 (`00bd59d`).
~$18.70 for 13 boxes, $1.44 each.

### The Phase −1 gate refused this file's own previous recommendation

The batch-38 checkpoint opened with map line 1293 as the cheapest next win. It is
not closable: `main.rs::disposable_candidates` builds only `Cost::Free`
candidates, so `disposable.rs:558`'s `filter(|c| !c.cost().is_free())` is always
empty in the shipped binary and `evaluate_reserve_spend` never runs. **1293 is
blocked on the same product decision as 1550** — may a background job spend paid
quota unasked? — and neither closes alone.

**Three rounds running, the recommended next step was structurally impossible.**
Do not treat anything below as cleared; treat it as a lead to be gated.

### The fifth link chose the round, and it was right both ways

Line 1547 was dispatched *because* its signal varies across the candidates being
ranked; 1293 was declined because its consumer is unreachable. The dispatched one
now changes which candidate a failover picks — proven by neutralising the penalty
to `0.0`, which makes the pair tie, and `best()` prefers the first, which the test
lists as the wrong answer. Phase 9J's prior at the same caller remains inert.

### The finding worth more than the boxes: a write nobody was watching

Deleting `mark_for_review`'s leading project-scope guard flips a **foreign
project's row** while the call still returns a correct-looking error, because the
trailing `self.get(id)` re-checks scope *after* the write. A test asserting only
the error would have passed.

**Six `UPDATE memories … WHERE id = ?1` statements carry no `project_id` in their
own WHERE clause.** All five enclosing functions currently have the leading
guard — checked one by one; a heuristic that flagged two of them was wrong — so
**there is no live defect.** The existing triggers do not cover it:
`memories_reject_foreign_project_update` is `BEFORE UPDATE OF project_id`, so a
status-only write to a foreign row fires nothing. **This is the next round's
red-tier package**, and the closest existing work to Phase 1 line 110, which is
unstarted and has no ledger entry.

### Both workers corrected their packets, and both were right — six rounds running

- `rustfmt <a mod file>` recursively formats every submodule it declares; it
  reached two FORBIDDEN files. Reverted in seconds. Practice §37 has an addendum.
- Bare `rustfmt` ignores `Cargo.toml` and hard-fails on this crate's let-chains.
  **Packets must say `rustfmt --edition 2024 <path>`.**
- `api/mod.rs:35-37` requires that door be proven by running the shipped binary,
  and the packet's file list had nowhere to do it. The worker added
  `tests/memory_query_api.rs` on the `capacity_api.rs` precedent. Right call.
- The `glasshouse` dev shim silently ran the **main checkout's** binary from a
  scratch project, because its fallback cannot warn when the wrong path is the
  default. Fixed and verified in three directions; practice §19 has an addendum.

## Checkpoint — 2026-08-28, batch 38 landed: 645 / 1280 (50%)

**Green on all three platforms on this exact tree** — local 13/13 including the
ubuntu clippy leg, `--windows-vm` 3/3, 1337 lib tests, zero slow-test warnings.

One worker, one box — map line 576, the user's configured `PairingPreference`
now reaching the scorer. Three mutations at three layers, all killed; the
config-resolution one re-run independently by the integrator.

### Phase 9J is nine of eleven, and the last two need a different caller

566 (a prior for a **fresh session**) and 569 (warm-session continuity) both need
a caller where the prior can decide something. **It cannot decide a same-model
failover** — `classify` derives `PairingClass` without reading `route`, so the
whole group scores identically however the preference is set. The preference does
differ across the `OfferMigration` group, which is what the user is shown.

**Do not send another package at `on_provider_failure` for those two lines.**
`next_turn` or session start is where they live, and `next_turn` is deliberately
sticky (lines 508/509) — making the prior participate there is a materially
larger change the map does not currently ask for.

### Phase −1 did something it was not designed for

Checking link 2 surfaced that **`profile/**` must not import `crate::config`** —
`Resolution`'s own doc says the caller does the lookup, and
`provider: Option<&'a Provider>` is that rule in practice. The packet therefore
told the worker the shape rather than letting it discover the ban mid-package.

**The gate does not only refuse impossible packets; it surfaces the
architectural constraint that decides how a possible one must be built.** Look
for that deliberately when writing the FEASIBILITY block.

### §66 — a worker killed mid-report has not lost its work

This one died to `API Error: Connection lost mid-response` **after** finishing its
code and running the full suite. Recovery was one message — *"write the report
now, do not redo any code work"* — against a 29-minute, ~$13 re-run. **Establish
what a dead worker actually lost before deciding what to do.**

## Checkpoint — 2026-08-28, batch 37 landed: 644 / 1280 (50%)

**Green on all three platforms on this exact tree** — local gate 13/13 including
the ubuntu clippy leg, `--windows-vm` 3/3, 1335 lib tests, zero slow-test
warnings. The standing flake did not fire this run.

**Half the map.** `gateway-evidence` closed thirteen — Phase 9J eight of eleven,
Phase 33A's five aggregate lines — plus a Windows fix (`0898efa`) the round
before it needed.

### The three-round arc finished, and its conclusion is negative

Batches 35–37 built the pairing prior, the evidence ledger, the explanation
surface, and finally a caller for all three at `gateway/mod.rs`'s accept loop.
The prior now runs in production, is mutation-proven, and **is structurally
inert where it runs.**

`harness/pairing.rs::classify` derives `PairingClass` from `(harness, model
attribution, harness vendor)`. `route.provider` — the only thing that differs
between two same-model failover candidates — feeds only `protocol_fit`, which
`native_pairing_prior_contribution` never reads. **Every same-model candidate
gets an identical prior magnitude**, so Phase 33A's local evidence is the only
signal that can separate them. The prior varies across the `OfferMigration`
group, but that group is only offered, never taken automatically.

**Only building the caller could have found this**, and it is why map line 566
stays open: it asks for a prior on a *fresh session*, and this caller is
failover.

### Two boxes with patches located exactly

- **576** — the user's configured `PairingPreference` never reaches this caller;
  everything scores against a hardcoded `Strong`. Thread an `EffectiveConfig`-derived
  preference into `profile::apply_gateway` (`profile/mod.rs:1064`) and set it
  alongside `gateway.routing().bind(...)` (`:1127`). **Check first whether
  `Resolution<'_>` (`:856`) already carries it.**
- **1293** — reserve inspectable in routing explanations. The explanation
  surface now exists and is reached in production; wiring `ReserveDecision::reason()`
  into it should be small.

### The process changes landed and immediately paid

`docs/process/assurance-economics.md` + `CLAUDE.md` + `validate_round.py`'s
Phase −1 gate (`f25ba9e`). **It refused two packets on first use**, in six
commands, before either reached a worker. Both were structurally impossible; one
of them was the "cheapest next win" this very handoff had recommended a
checkpoint earlier. **A recommendation in a handoff is not a feasibility
argument.**

Batch 37 also ran the first Phase-1b report: structured facts with flagged
`decisive_claims` rather than a narrative. It worked — review targeted the one
claim that mattered. Self-reported split: **60% implementation, 25%
verification, 15% report.**

### §65 — a change no test can observe may be doing something nothing is watching

The Windows hang (`0898efa`) is the entry worth reading. The integrator opened a
SQLite handle unconditionally on every launch; macOS never noticed, Windows hung
six tests for 37 minutes with no output. Hours earlier the same wiring had
survived a mutation with the whole suite green, and the conclusion drawn was
"add a guard proving it exists" — the wrong lesson. **Presence was never in
doubt; safety was.**

## Checkpoint — 2026-08-28, batch 36 landed: 631 / 1280 (49%)

Three Sonnet workers, twenty-seven boxes. Phase 14 **eleven of eleven**, Phase 16
three of seven, Phase 33A six of fifteen, Phase 35B seven of twenty-five.

### Batch 35's headline finding was half wrong, and batch 36 corrected it

"Eighteen boxes wait on one missing consumer" folded together **two different**
missing consumers:

- **The reserve half was right.** `evaluate_reserve_spend` is now called from
  `DisposableRouting::choose`, mutation-proven, reached from `main.rs:1120`.
- **The pairing half was wrong.** `PairingQuery::harness` is a required
  `IntegrationId`; all ten variants are third-party coding harnesses a user
  launches; a disposable job is **Glasshouse's own internal call** and carries no
  harness. **Phase 9J's eleven need `InteractiveRouting`**, not the disposable
  router — a different caller in a different file.

**Before sizing that package, check one thing:** does a bound gateway session
know which `IntegrationId` it serves? If `SessionRouting::bind`'s `Assignment`
does not carry it, Phase 9J is blocked deeper than one caller.

### The cheapest next package is a reader for the ledger

Phase 33A's five aggregate boxes (1335, 1336, 1339, 1340, 1341) are all
`summarize` having no production consumer, and `ObservedEvidenceSource` already
implements the trait `routing-score` stubbed with `NoObservations` only because
`evidence.rs` belonged to another worker that round. **Those two halves are one
small package now that both files are free.**

### What is blocked and on what

- **Phase 16's four** — a cross-process architecture decision. `glasshouse api
  serve` is a *separate process* from the TUI and `session/attach.rs` says its
  input path is not reusable inside a longer-lived interface. Red tier, product
  question first. See `docs/product/evidence/phase-16.md`.
- **Phase 35B line 1550** — the reserve gate is dead in the binary because
  `main.rs::disposable_candidates` never builds a metered candidate. Closing it
  decides whether a background job may spend paid quota unasked; no source for
  "which metered model" exists in `ProviderConfig`.
- **Phase 33A's other four** (1331–1334) — the gateway cannot read a response
  body **by design**. Needs a component that reads the response stream's framing.

### A near-term win nobody has taken

**Line 1547**, failure-domain diversity. `routing::free::FreePool`'s allowance is
already keyed per credential, so "two models sharing one exhausted credential" is
a signal that exists in this build today. Left only for time.

### Two defects in the integrator's own work, both found by self-mutation

`EvidenceLedger::open(runtime)?` ran on every launch and would have turned a
telemetry failure into a failed session — fixed to warn and continue. Then the
wiring proved **invisible to every test**: removing it left the suite green, so
`every_gateway_the_binary_starts_is_given_the_evidence_ledger` now scans for it.
**Apply the mutation discipline to your own integration edits, not only to
workers' diffs.**

## Checkpoint — 2026-08-28, batch 35 landed: 604 / 1280 (47%)

`2d8e569` pushed, tree clean. Local gate **13/13** including the ubuntu clippy
leg. Three Sonnet workers, thirty-five boxes, three commits.

| package | closed | what it settled |
|---|---|---|
| `pairing-prior` | 1 / 12 | no router exists to rank candidates |
| `mem-validity` | 22 / 24 | Phase 21C 11/11, Phase 21D 9/9, migration 10 |
| `phase-32d` | 12 / 20 | capacity score, bands, fail-closed thresholds, capacity API |

Also landed `db5510e`: `check-evidence-coverage.py` now validates the ledger's
state vocabulary (warn-only, §51). **Twenty** declarations are outside the
SDLC's six states, not the twelve an earlier checkpoint estimated.

### The next round should build a consumer, not another producer

**Eighteen boxes in this round were blocked by one missing thing**, and two
separate packages hit it independently: Glasshouse has no component that ranks
candidates and decides.

- Phase 9J, all eleven — `native_pairing_prior_contribution` has no caller.
- Phase 32F, seven of eight — `evaluate_reserve_spend` is called only from
  `tests/capacity_score.rs`.
- Plus map line 1293 and Phase 9J line 569.

Both workers built their half correctly and stopped, which was right. A third
package building a fourth unreachable mechanism would be waste. **Phase 35B
(candidate scoring, 0/25) or Phase 37 (basic session-aware router, 0/11) is what
unblocks those eighteen plus its own.** Its dependency is Phase 33A (routing
evidence ledger, 0/15), which supplies the `ObservationSource` the prior decays
against.

The seams are written down and should be quoted into that packet:
`report-PAIRING-PRIOR.md`'s last section gives the exact scorer signature;
`docs/product/evidence/phase-32f.md` gives the reserve half. `routing/mod.rs`
now has `Contribution`, `RoutingExplanation`, `EligibleCandidate<T>` and
`apply_hard_constraints` waiting for exactly that caller.

Full partition plan, with the files each package needs, is in
`.agent-runtime/plan-batch-36.md`.

### `discover.py --seam`'s verdict line can be wrong — read it as "look here"

It reported three call sites for `evaluate_reserve_spend` and concluded a box
could close. All three were inside `quota.rs`: two intra-doc links and the
definition itself. §49 already says a match is a lead rather than proof; this is
the first time that changed a tick. **A one-line fix is available to whoever
next owns `scripts/`: exclude the definition and `///` lines before counting.**

### Mutate what a report calls load-bearing, not what it lists as proven

All three review findings this round came from that, and none was a gate
failure — every worker's gate numbers were exactly right. `mem-validity` called
the search over-fetch load-bearing in its own prose and had no test for it;
removing it left all 1750 tests green.

### Settled, stop re-checking

- **Map line 1210** (quota window *start*) — five packages now. No host
  publishes one.
- **Phase 20 lines 828/829 and Phase 21A line 862** — all three ask for a
  judgement about the project that the storage layer cannot make. A keyword
  heuristic would refuse real memories and admit fake ones. Recorded in the
  ledger with the reasoning; do not re-derive it.

### Windows: the batch is green apart from one flake, now MEASURED at 33%

`--windows-vm` build and msrv pass. `test (windows) / test` failed on **both**
runs of this tree, always the same test:

    session::api::tests::interrupting_through_the_api_is_recorded_as_machine_initiated

**Batch 35 was suspected and is exonerated, by measurement rather than by
assumption.** A worktree was cut at the pre-batch commit `9f60f07` and the
Windows suite run against it three times:

| tree | windows lib runs | fails |
|---|---|---|
| `9f60f07` pre-batch | pass 4.86s, pass 4.78s, **FAIL 47.99s** | 1 of 3 |
| `2d8e569` batch 35 | **FAIL 48.16s**, **FAIL 48.01s** | 2 of 2 |
| historical (earlier checkpoints) | — | 2 of 6 |

**Three failures in nine runs of unchanged code — 33%**, matching the rate
earlier checkpoints recorded. Two consecutive failures is what a 33% rate
produces about one time in nine. Not a regression.

**The suspicion was reasonable and worth recording**: the batch's lib suite
appeared 10× slower (4.86s → 48.16s), which looked like a performance
regression. It is not — a pass costs ~5s and a failure costs ~48s because the
test waits out a 45-second deadline. **The suite time and the failure are the
same fact**, so wall-clock here says only which way the coin landed.

### This flake should stop being nobody's job

It is now the single reason this project cannot claim three green platforms,
and it has cost real time: attributing it this round took **four Windows runs
and about forty minutes**. At 33% it will redden roughly one gate in three,
forever, and every future orchestrator will pay the same attribution cost
before trusting a red Windows result.

What is known, from the test's own comments and this round's runs:
- Windows-only. The harness is a `.cmd` script; `cmd.exe` intercepts Ctrl-C
  rather than dying, so the assertion waits for `^C` to appear in the
  scrollback rather than for process death.
- The deadline is already 45s, raised from 15s once before. **Raising it again
  is §57's "one more string" and should not be the fix.**
- It fails by the deadline expiring, not by a wrong value.

**Red tier** — Windows console control events, PTY, and a real child process.
Give it a package of its own with the brief: *find out whether `^C` genuinely
never arrives or merely arrives late, and if late, what it is waiting on.* A
test that cannot distinguish "the interrupt did not work" from "the console was
slow" is the actual defect.

Beside the 1-in-37 `pty_smoke` `SIGABRT`, still unowned.

**`RETRIEVAL_WEIGHT_FLOOR = 0.15`** is tuned against one ordering scenario, not
derived. Recorded in `phase-21d.md` as provisional so it is not mistaken for a
constant with a proof behind it.

## Checkpoint — 2026-08-28 early, batch 34 landed: 569 / 1280 (44%)

`7215d3d` pushed, clean, green on all three platforms — local 13/13,
`--windows-vm` **3/3**.

**`glasshouse resources` computed a real percentage for the first time.** Map lines
1199, 1211, 1217 and 1218 close together: a Groq registry template plus the three
`main.rs` edits `bridge-quota` had located. Neither half closes anything alone —
Groq is the only host ever observed sending both halves of a pool, and this build
shipped no template for it.

**1217/1218 had been open across four consecutive packages**, each declining to
tick a structural guarantee that had never fired in the shipped binary. It fires
now.

### What is next, in order

1. **Nothing in the 32-family is blocked on evidence any more.** 32C (subscription
   estimation), 32D (normalized score), 32E–32G are all unblocked by a live
   reading existing. 32D in particular now has real input.
2. **The TUI pty harness.** Still unbuilt, still Red tier, and now wanted by a
   third thing: `quota-live` noted a CLI-level write-side mutation needs a harness
   that drives a gateway-backed session through the binary. Named twice, wanted
   three times.
3. **`check-evidence-coverage.py` should validate the state vocabulary.**
   Twelve entries use states the SDLC does not define. Warn-only first (§51).
4. **`worker-watch.sh` needs §28's worktree-growth signal.** Now wanted by two
   mechanisms — see the measurements entry on the heartbeat blind spot.
5. **Settled, stop re-checking: map line 1210.** Five packages have now reported
   that no host publishes a quota window *start*, only resets.

### Standing debt

`session::api::tests::interrupting_through_the_api_is_recorded_as_machine_initiated`
— **2 failures in 6 Windows runs**, unchanged code since `d35fe6a`, nobody's
regression. Beside the 1-in-37 `pty_smoke` `SIGABRT`.

## Checkpoint — 2026-08-27 night, batch 33 landed: 565 / 1280 (44%)

`c40194e` pushed, tree clean. Local gate 13/13; `--windows-vm` build+msrv pass with
one **pre-existing** flake (below). Phase 48 closed eight of eight — six were
already shipped, and two of those six had a caller with no test entering through it
(§35). `glasshouse status` is the one new command.

**`bridge-quota` closed zero boxes and is the round's real output.** The gateway→
registry bridge needs a durable store (built: `GatewayQuotaCache`) plus three
`main.rs` edits (located exactly, in `report-BRIDGE-QUOTA.md`) — **and even then
the four boxes would not close**, because Groq is the only host observed sending
both halves of a pool and Glasshouse ships no registry template for it. **The
blocker moved from "no bridge" to "no host with both a template and both halves."**

### The next package, and it is well specified

**Ship a Groq registry template *and* land the three `main.rs` edits together.**
Either alone closes nothing. That package closes map lines 1199, 1211, 1217 and
1218 — including the first live percentage this product would ever compute.

### Settled, stop re-checking

**Map line 1210 (quota window *start*).** Four consecutive packages — 32A, 32B,
QUOTA-FOLLOWUP, BRIDGE-QUOTA — have now reported that no host anywhere publishes
one, only resets. Treat it as blocked on evidence that may never arrive.

### Standing debt with a measured rate

`session::api::tests::interrupting_through_the_api_is_recorded_as_machine_initiated`
— **2 failures in 5 Windows runs (~40%)**, unchanged code since `d35fe6a`, nobody's
regression and nobody's job. Beside the 1-in-37 `pty_smoke` `SIGABRT`.

## Checkpoint — 2026-08-27 late, batch 32 landed: 557 / 1280 (43%)

Pushed, clean, **green on all three platforms on this exact tree** — local 13/13,
`--windows-vm` 3/3. Two Sonnet workers, 18 boxes.

**Phase 35 closed fourteen of fourteen for about twenty lines of production code.**
The classifier shipped in batch 30 and closed nothing; its only possible caller
lives in `main.rs` and `cli.rs`, which that packet's partition excluded. Adding
`glasshouse classify` closed every box. Second time in one day that the same lever
moved — `glasshouse resources` did it for Phase 32.

**The gateway now reads its own rate-limit headers** (map 1229), never a byte of
the body, proven through a real TCP exchange and the accept loop's unmodified call.
Plus 1230 (OpenRouter's `/key`, parsed with a real absent/null/number distinction),
1200, and 1202 which turned out already closed.

### Four boxes stayed open on one honest finding, and it is the next real job

`quota-followup` built a working reader for Groq's rate-limit headers and proved it
produces a real `Percentage::Exact(99)` — **the first live percentage this product
has ever computed** — then left map lines 1199, 1211, 1217 and 1218 **open**,
because **nothing in the shipped binary bridges a gateway-captured reading into
`glasshouse resources`'s registry loop.**

**That bridge is the highest-value next package.** It needs `main.rs` or `shell/**`
plus `provider/**` in one partition, and it closes four boxes that are otherwise
permanently stuck behind a reader that already works.

### Two gate findings, neither of them a worker's fault

**`lint / clippy` and `lint (ubuntu) / clippy` are not peers.** The container
installs clippy **1.98.0** per run; this machine's `stable` is **0.1.96** with
nothing newer installed. A `question_mark` lint failed only on Linux. **Read a
green local clippy as provisional until the ubuntu leg agrees**, and consider
having `ci-local.sh` print both versions when they differ. §20's family, third
instance.

**Twelve evidence entries use states the SDLC does not define** — five `CLOSED`,
three `VERIFIED`, two `NOT ATTEMPTED`, two `BLOCKED`. `check-evidence-coverage.py`
asks only whether a phase *has* an entry, never what state it declares, so
`CLAUDE.md`'s one rule about the ledger is unenforced at the entry level.
`phase-35`'s was corrected by hand this round. **Extend the checker to validate the
vocabulary, warn-only first** (§51 — a gate that starts red gets overridden).

### Cost per box is not a tier property, and batch 31 read it as one

$0.26 and $6.39, same tier, same effort, same day. It measures **how much of the
work was already built**, not the model: `phase-35-classify` inherited batch 30's
717 lines, `quota-followup` wrote 1,932 of its own and then declined to overclaim.
Open question 1 goes back to open — see `orchestration-measurements.md`.

## Checkpoint — 2026-08-27 evening, batch 31 landed: 539 / 1280 (42%)

`c25448b` is pushed, the tree is clean, and the gate is green on **all three
platforms on this exact tree** — local 13/13 (macOS + Linux) and `--windows-vm`
3/3 on the ARM64 VM.

Two commits: `8b4c982` (three workers, 25 boxes, 514 → 539) and `c25448b`, a
Windows build fix for a defect the **previous** round shipped.

### What landed

| package | tier | closed | left open |
|---|---|---|---|
| `phase-32b` | Opus, high | **16** — 11 of Phase 32B, 2 of 32A, both Phase 49 quota-config lines, map line 1761 | 1229, 1230, 1239 |
| `phase-47` | Sonnet, high | **4** | 1763, 1769 |
| `phase-46` | Sonnet, high | **5** | 1745, 1746, 1748 |

**`glasshouse resources` is the round's real deliverable.** `discover.py --seam
ResourceRegistry` answered ZERO before dispatch: Phase 32 and 32A — about 2,000
lines — were reachable from nothing in the shipped binary, which is why 32A closed
3 of 21. The package owned `cli.rs` and `main.rs` for that reason alone, and the
command it built is what carries the other fifteen boxes. Run against the real
machine it reports the harness's own plan as authoritative and prints
`capacity unknown` wherever nothing was read.

### Four boxes were left open against their worker's own COMPLETE

Reasoning is in each ledger entry, not just here.

- **1229** names *"API **and** gateway responses"*. The API half is built and
  proven; the gateway half is not. A ticked box stops being scheduled.
- **1748** wants tests that a deletion touches one project. **No deletion exists**
  — `remove_dir_all` is absent from `src/`, no CLI subcommand deletes anything.
- **1763, 1769** stop at partition edges their packet predicted.

### The orchestrator was wrong twice, and both corrections are recorded

- **Design note D2 is withdrawn.** It forbade `phase-32b` the gateway response
  path, citing Phase 9I's *"a parser there would make it a reader of the payload
  it exists to pass through."* **Reading a response header is not reading the
  payload** — the gateway already parses the header block to forward it. The
  worker declined to reverse a decision it was told not to reverse, correctly.
- **"H1 is dead" generalised six sampled hosts to a population.** Six hosts send
  no rate-limit header on `/models`; the worker probed **eight** unauthenticated
  and found AnyRouter sends five. Reproduced independently, cache-buster and all.
  That is §63's error committed by the orchestrator who quoted §63 in the packet.

Full probe record, including all three probes the worker requested and their
answers, is in `.agent-runtime/probe-quota-headers-2026-08-27.md`.

### Two Windows findings, and only one of them is fixed

**1. `60f8c9f` shipped a Windows build break, now fixed in `c25448b`.**
`api/mod.rs` gated `mod unix;` but left `mod protocol;` ungated; `unix.rs` is
protocol's only consumer, so on Windows the whole module was dead code and
`-D warnings` made it a hard error. Four errors, no tests ran at all. Reproduced
and fixed locally with §18's cfg flip — **the pre-fix file flipped to the Windows
shape gives the identical four errors** — so no second VM round trip was needed.

**It shipped because `--windows-vm` was not run after `60f8c9f`.** The rule this
buys: **run it on every round that lands, not every round that feels risky.** The
local gate was 13/13 on that commit and 13/13 on this one, and neither says
anything about Windows. Fifth Windows-only defect here to survive a green local
gate.

**2. An unowned Windows flake, attributed and left open.**
`session::api::tests::interrupting_through_the_api_is_recorded_as_machine_initiated`
failed once and passed once on the **identical tree** (1240/1 then 1241/0). Same
tree, two runs, two answers is §40's proof of nondeterminism. Batch 31 touched
`session/api.rs` zero times and `60f8c9f` touched no `session/` files, so it is
nobody's regression. **Rate: 1 in 2** — high, on an *interrupt* test spawning a
real Windows child, which the handoff already records as proven by nothing. It
joins the standing flake debt beside the 1-in-37 `pty_smoke` `SIGABRT`.

### Next, in the order that actually unblocks things

1. **Line 1229's gateway half**, now unblocked by D2's withdrawal. Read the
   response header block on the gateway path, never the body. Small.
2. **Line 1230 is ready to close.** OpenRouter's `GET /api/v1/key` was probed:
   `200`, with `usage`, `usage_daily/weekly/monthly` and
   `rate_limit.{requests,interval}` — and **`limit`, `limit_remaining` and
   `limit_reset` are all `null` on this account**, which is exactly the trap a
   parser assuming a number would hit. Schema is in the probe record.
3. **`profile/mod.rs`'s three lines, now twice deferred.** Phase 32A was blocked
   on it, `phase-32b` was blocked on it, and the patch is written verbatim in
   `report-PHASE-32B.md`. **Put that file in someone's partition next round.**
4. **A pty harness for the interactive TUI.** No such harness exists, so *every*
   TUI contract in the map rests on render tests alone. Red tier, and it would
   serve far more than the one line that exposed it.
5. **`glasshouse classify <text>`** still closes most of Phase 35's fourteen —
   blocked this round only because `phase-32b` held `cli.rs` and `main.rs`.

## Checkpoint — 2026-08-27 midday, round closed and the next one prepared

**`7325a9b` is pushed, the tree is clean, and the gate is green on both
platforms on this exact tree** — 13/13 Unix, 3/3 Windows. **441 / 1280 mandatory
(34%)**, README in sync, no worker waiting to be acknowledged.

The previous round's four workers — `windows-defects`, `mio-spin`,
`provider-probe`, `launch-overlay` — are all integrated. Phase 0 closed at eight
of eight, Phase 10 at fourteen of fourteen, Phase 9K groups 1–2, Phase 9J group
1, and Phase 4's interrupt box, which had been open since Phase 4 for no reason
except that no Windows machine existed to test it.

**The Windows VM changed the evidence base more than any single fix did.** It
turned "1,069 passed, 1 failed" into the discovery that **281 tests had never
executed there at all**: `cargo test` stops after the first failing test
*binary*, and behind that one failure sat two more red binaries.

## The next round is prepared and not dispatched

`.agent-runtime/ROUND-BRIEF.md` is the incoming orchestrator's first read. Four
packets are written, four worktrees exist at `7325a9b`, and
`scripts/validate_round.py` passes on the set — the file partitions are provably
disjoint and every quoted box line matches the map verbatim.

| worker | tier | kind |
|---|---|---|
| `typing-throttle` | Opus specialist | **defect** — one keystroke per 16ms tick |
| `windows-truth` | Sonnet | **defect** — four ways Windows coverage overstates itself |
| `phase-10a` | Opus lead | **forward** — session supervision, 13 boxes |
| `phase-9a-facts` | Sonnet | **forward** — lines 353 and 368, gaps located |

Two defects and two forward packages, deliberately. `shell/state.rs` and
`shell/view.rs` are left unassigned so **Phase 11's display half is free the
moment this round lands.**

**TYPING IS THROTTLED TO ~59 KEYS A SECOND AND IT IS A REGRESSION THIS PROJECT
SHIPPED.** Since batch 26's quiet-tick short cut, a 200-character paste takes
3.4 seconds. `EventSource::next` waits on the *descriptor* before it consults
*crossterm*, and crossterm buffers — so after the first key of a burst the rest
are inside crossterm while the descriptor is empty, and the loop sleeps out the
whole tick before asking. That is a hypothesis with the right arithmetic, and
the packet requires it to be tested before it is fixed (§58).

**The three things not in anyone's packet, and why:** the 1-in-37 `SIGABRT`
lives in `tests/pty_smoke.rs`, which `windows-truth` holds; the residual
terminal-loss spin (~2 in 60) is `typing-throttle`'s to *re-measure*, not to
fix; and whether Phase 9J line 572 belongs in Phase 33A is a map edit and the
user's call.

**Fifteen provider API keys** are in `.agent-runtime/provider-keys.env`, mode
600, gitignored, and not given to any worker in this round. The brief carries
the redaction rule that travels with two of them.
## Current capability / phase

**Phase 10 is fourteen of fourteen** and **Windows is real evidence now.** A
Windows 11 ARM64 VM runs this project's tests natively; `scripts/ci-local.sh
--windows-vm` drives it through `glasshouse-windows-ci`, and the gate's
`NOTE  Windows was not exercised at all` is now printed only when it is true.

**The first Windows run found four defects that had never been seen**, and the
reason they were invisible is worth more than any of them: `cargo test` stops
after the first failing test binary, so a single failing library test meant
**three integration suites had never executed on Windows at all.** "1069 passed,
1 failed" was a truncated run reported as a near-perfect one.

**Two are real product defects and neither is fixed** — see the next-action
list. Two more are measured facts about what Windows coverage means here: two
`session::api` tests **pass against a child that never started and produced not
one byte**, and 42 tests are `#[cfg(unix)]` and never run on Windows at all.
Interrupt delivery to a real Windows child, resize reaching one, and session
resume are proven by **nothing**.

**Phase 9K is twenty-one of thirty-seven; Phase 9J is nine of twenty**, and the
eleven in 9J's second group are blocked with the phase each waits on named.

**Phase 0 is closed, eight of eight**, after its dependency line was found to be
unsatisfiable by any tree that also satisfied its own phase and the user
reworded it.


**Phase 9K is twenty of thirty-seven.** Groups 1 and 2 — the profile model and
harness-native application — are closed and proven in the shipped binary:
`glasshouse response` reports the resolved profile with the precedence layer
each of the five axes came from, and `glasshouse run --response-profile` puts
an output style in the session's settings document and appends to the system
prompt without ever replacing it. Of the seventeen not owned by that package,
**ten are blocked** (four on Phase 47 and Phase 51, both at zero; one on a
schema migration; the rest on there being no in-session profile-change surface)
and seven are argued line by line in `docs/product/evidence/phase-9k.md`.

**Two probes against Claude Code 2.1.247 are load-bearing and were re-run by
the orchestrator.** `--settings` is **last-wins, not merge**: with a malformed
document first, `claude doctor` reports nothing; with the same document last it
reports the error. A response profile that appended its own `--settings` would
have silently switched off every lifecycle hook in the session. The keys are
merged into one document instead.

**Phase 9J is nine of twenty** and its other eleven are blocked, none of them
on each other. There is no scoring function anywhere in the crate, so a routing
prior has nothing to be a term of.

**Two Phase 0 boxes were unticked** when its evidence entry was written; one is
back, one is still the user's call. `check-evidence-coverage.py --strict` is in
the gate, so a box ticked without a ledger entry fails it.


**Phase 9J is nine of twenty and the other eleven are blocked, which is the
round's real finding.** Group 1 (pairing identity) is closed and proven end to
end: `glasshouse pairing` reports a class per configured profile, and a
`[pairing.models."<id>"]` table in the user's configuration changes what the
binary prints with no router code touched. Group 2 (the prior and its evidence)
is **0 of 11, none of them blocked on each other**: seven wait on a routing
prior existing at all (Phase 35B, 0 of 25), two more also on Phase 33A (0 of
15), one *is* Phase 33A's tenth line almost verbatim and arguably belongs in
that phase, and two have a partial answer already shipped. There is no scoring
function anywhere in the crate — `grep -rn 'fn score\|Score' src/` is empty —
so a prior would have nothing to be a term of.

**Line 576 was deliberately left open rather than faked.** Four preference
values are half an hour of configuration plumbing and would be a field parsed
and never consulted, because a preference over a prior that does not exist
consults nothing.

**Two Phase 0 boxes were unticked** when its evidence entry was finally
written — see the next-action list. `check-evidence-coverage.py --strict` is now
in the gate, so a box ticked without a ledger entry fails it.


**Phase 9G, 2C, 9B, 9C and 9D are COMPLETE.** 9E eleven of thirteen; 2D six of
nineteen; Phase 9 five of seven; 9F eleven of thirteen; 9A nineteen of
twenty-six; **392 checked boxes (30%).** Local suite **1300+ passing**.

**Phase 9H, 9I and 21B all landed in one round.** Sticky gateway routing and
free-pool routing were both untouched before it; Phase 21B is complete at 11 of
11. Phase 21 has four lines left and three of them wait on Phase 39.

## Next action

The three pieces the previous round specified are **done**: the disposable
policy has its caller (Phase 9I is 13 of 14), migration 7 landed with `seq`
proven durable, and the Linux gate's random failures are fixed — 8 failures in
17 full-suite runs before, 0 in 20 after.

**The TUI spin is fixed for the deterministic case and survives at roughly two
in sixty.** A terminal that goes away now ends the interface instead of pinning
a core. The wait moved out of crossterm into `tui::event::wait_for_terminal`,
which uses `libc::poll` on the descriptor crossterm itself reads from and
answers `HangUp` on `POLLHUP` before it ever looks at `POLLIN` — a hung-up pty
reports both at once (`0x11`, measured), and reading the `POLLIN` half of that
is precisely the spin.

**Do not read `tests/terminal_loss.rs` passing as "the spin is gone" — practice
§60.** That test runs the scenario once per gate run. A sixty-trial harness on
the same tree still caught the process alive at `Rs+ 100.0` twice, with
cumulative CPU equal to its whole lifetime; an idle Glasshouse with a live
terminal is `Ss+ 0:00.03 0.3%` over the same interval, measured twelve times,
so the residual is real and not the harness's own load. The harness that
reproduces it is kept at `.agent-runtime/diagnostics/` with what it measured.
**This is the next package on this defect**, and `terminal-loss`'s own report
named the likely window: a terminal that dies between `Wait::Ready` and
crossterm's `read`. That report called the window microseconds wide; two in
sixty says it is wider than that, or that there is a second path.

**The packet's account of the cause was wrong, and the correction is the
transferable part** — practice §58. Crossterm's `try_read` does not burn a tick
and return; `Ok(0)` falls through an inner loop that checks no timeout, so the
call never returns at all. The pre-check the packet proposed would therefore
have fixed nothing.

**A third defect came out of the fix itself and is now also fixed.**
`shutdown.rs`'s handler implemented "a second signal forces the process down"
by reading `SHUTDOWN_REQUESTED` — sound for as long as a signal was the only
thing that could set it, and false the moment `wait_for_terminal` began
answering a hangup by requesting shutdown. Closing a terminal delivers `SIGHUP`
and `POLLHUP` at the same instant, so one hangup looked like two interrupts and
the process was forced down through `force_exit` with no destructors, exit 130
instead of 0. Measured before the fix at **eight of ten** on macOS with a
controlling terminal, and by the worker at **ten of ten** in a Linux container;
after, **nine of ten clean zeros** with the one residual spin above. The handler
now counts signals in `SIGNALS_SEEN`, which only it may touch.

Windows is **deliberately unhandled and says so in the doc comment**: a console
going away raises `CTRL_CLOSE_EVENT` on a handle, not endless zero-byte reads
on a descriptor. `Wait::Unavailable` keeps the old behaviour there byte for
byte. Compiling that path locally with the cfg flipped (§18) caught a real
`-D warnings` break that would have failed the Windows job on a green tree.

**`session/attach.rs` now notices its own terminal dying, and the fix is inert
today.** `pump_input`'s `Ok(0)` confirms with the same `POLLHUP` check before
requesting shutdown. The worker built the acceptance test the packet asked for,
found it passed with the fix reverted, instrumented `supervise` to find out
why, and reported honestly: `ctrlc`'s `termination` feature already delivers
`SIGHUP` to the same flag, and `supervise` polls it every 20ms, so the signal
path already saves that process. The fix is a second, independent way to set
the flag that does not depend on a signal arriving at all. **It is kept, it
cannot mis-fire, and it has no test** — which is the right report to have
written rather than a test that appeared to prove more.

**Ratatui's drop no longer panics on the way out.** `Screen`'s terminal is
`ManuallyDrop` and goes through `drop_terminal_tolerantly`, which drops it
under `catch_unwind`; Ratatui shows the cursor on drop and `eprintln!`s behind
an `.expect` when that write fails, which it does once the terminal is gone.
Exit code after a hangup reaching `Screen::drop`: **101 before, 0 after.**

**This also removes an orchestration hazard, not only a product defect.**
Practice §38 says the only way to drive the binary is a cmux pane, so every
binary probe a worker performed created a candidate, and closing that pane
afterwards left it spinning — four accumulated in a single day. Closing the
pane now ends the process.

**Two related defects were found in passing and are not fixed.** Both are
outside that partition and belong to a follow-up package:

- `session/attach.rs:255` — `pump_input` breaks on `Ok(0)` and its thread ends;
  `supervise` then waits forever on a harness nobody can see or type at. It
  does not spin, so it was not part of the 501%, but it is the same missing
  question, and the new `request_shutdown()` does not reach it because `attach`
  runs while the TUI does not. Suggested shape: `Ok(0)` calls
  `shutdown::request_shutdown()`, which `supervise` already watches at line 185.
- Ratatui's `Terminal::drop` `eprintln!`s when it cannot show the cursor, and
  that is itself a panic on a hung-up pty — so some paths exit 101 rather
  than 0. The process does go away; a clean exit is still worth having.

**A Windows host now exists.** `GLASSHOUSE_WINDOWS_HOST` +
`scripts/ci-local.sh --windows-vm` has a real target for the first time, so the
interrupt box below can finally be tested rather than compiled. Expect several
jobs to fail at once on the first run; reconcile them in one sweep.

**`scripts/ci-local.sh --windows-vm` is GREEN**, all three steps, as of
2026-08-27 — the first time in this project's history. The paragraph below is
kept because the number in it is the one that matters going forward.

**281 of 1401 tests had never executed on Windows.** `cargo test` stops after
the first failing **binary**, and the gate stopped at `events_lifecycle` — so
behind it sat two more red binaries, including **five `response_profiles`
failures from a phase that landed the same morning**. All nine failures are now
fixed. **`C:\ci\run-glasshouse-ci.cmd`'s `:test` line still does not pass
`--no-fail-fast`**, which is one word, and until it does the next red binary
hides everything behind it again.

**Superseded — kept for the reasoning:**
On the current tree it reports `PASS test (windows) / build`,
`FAIL test (windows) / test`, `PASS msrv (windows) 1.88`. The library suite is
**1084 of 1084** (it was 1069 with one failure hiding three whole suites), and
`events_lifecycle` then fails three — `a_stalled_event_consumer_does_not_stall_a_live_harness`,
`a_quiet_harness_that_exits_cleanly_is_never_reported_as_having_finished` and
`one_worker_crashing_leaves_unrelated_sessions_running`. `cargo test` stops
there, so `pty_smoke`'s one failure is still behind them.

**Do not chase that red by weakening a test.** It is the first accurate picture
of Windows this project has ever had, and every one of those failures traces to
the two product defects below. The gate's final line now distinguishes the
cases: `Windows ran for real on the ARM64 VM. Those lines ARE evidence about
Windows.`

**1. Two Windows product defects, both found by the VM's first honest run.**

**`OutputEnded` can never fire on Windows.** `runtime.rs`'s `pump()` uses
`Ok(0) | Err(_) => break` as its only stop condition, and `pty/mod.rs:499-514`
had **already written down** that Windows does not produce EOF there:

> it must not treat "no more bytes" as its stop condition, because on Windows
> that may never come while the pty is still held open. Treat "the process was
> observed to have exited" as the authoritative stop condition on every
> platform.

A prediction recorded in the source and then not honoured. Consequence:
`output_ended()` never becomes true, so `LifecycleEvent::OutputEnded` never
reaches the event log, the shell's feed, or memory extraction's narrative.

**On Windows you can enter a session and never get out.** `Ctrl-]` is not
recognised — `pty_smoke::the_shell_enters_and_leaves_session_mode_in_a_real_terminal`
fails 3 in 3, deterministic, not a flake. Isolated by a test in the same run:
`q` exits cleanly, so the shell is fine and the escape chord is not. That is
precisely the trap `is_session_escape`'s own doc comment says the single chord
exists to prevent. `shell/state.rs:3554-3567`.

Both are **Opus specialist**, both need the VM, and they belong together in one
packet with the two `#[cfg(unix)]` tests that should have covered them —
`an_embedded_session_answers_the_cursor_position_query_itself` and
`every_startup_question_a_harness_asks_is_answered` test the DSR-answering
mechanism **only on the platform where answering is optional**, and not on the
one where it is the difference between a session and a hang.

**2. `glasshouse-windows-ci` packages main, not your worktree.** It hardcodes
`ci_repo="/Users/eneas/projects/glasshouse"`, so run from any worktree it
tests **main** and reports the result as though it were about the tree you are
standing in — the same wrong-green the Linux leg copies-instead-of-mounts to
avoid. One line fixes it and **it is the user's file**, so it is a
recommendation, not a change:

    readonly ci_repo="${GLASSHOUSE_CI_REPO:-/Users/eneas/projects/glasshouse}"

`ci-local.sh` handles both worlds already: it exports the worktree path if the
helper honours the variable, and **skips with a reason rather than guessing** if
it does not. Once that line is taken, the helper should move into
`scripts/dev/` — a gate whose runner lives only in one person's `~/.local/bin`
is a gate nobody else can run — but not before, and not with `ci_key` and
`ci_lease_file` still absolute paths to a private key.

**3. Three Phase 9K boxes are arguably closed and were left unticked.** The
`response-profiles` worker built the additive `--append-system-prompt` path,
which belongs to group 3, because its own line 604 requires recording that a
native, **additive**, or fallback mechanism was applied — and with no additive
mechanism that branch is unreachable and 604 is only half real. It wrote the
argument for 613, 614 and 615 in `docs/product/evidence/phase-9k.md` and left
the call deliberately. Read those three arguments and decide; the code is
already there and proven.

Two smaller ones from the same package, both cheap and both written up:
**line 631** (a way to disable injection above a layer that has already spoken)
is about thirty lines, and **line 632** needs migration 8 adding
`sessions.response_profile` and `sessions.response_mechanism`, append-only, in
the shape migration 3 used for `launch_profile`.

**Line 605 is reachable on the launch path only.** `crate::shell`'s quick-open
calls `install_hooks` and resolves no launch profile either, so it gets the
harness untouched in every respect. The packet's partition omitted
`shell/mod.rs`; the worker correctly did not edit it and reported the ten-line
patch. Whoever gives the shell's quick-open a launch profile should give it a
response profile in the same change.

**4. A second 100% CPU spin, pre-existing, and it loses keystrokes.** Found by
`spin-residual` while attributing a resize regression; it is on `main` today and
it needs **no hangup at all**.

Crossterm's `mio` registration is edge-triggered (`EV_CLEAR`, confirmed in mio
1.2.2's kqueue selector) and its `try_read` returns the *first* readiness it
looks at, abandoning unread whatever arrived in the same batch. When a
`SIGWINCH` and terminal input land in one batch and the signal is looked at
first, **the terminal's readiness is discarded and those bytes stay invisible to
crossterm until new input creates a new edge.** Caught instrumented:

    stalls=1001 fd=0 FIONREAD=32 revents=0x1

Thirty-two bytes — the stranded command — unread on the descriptor, `POLLIN`
set, no `POLLHUP`, crossterm reporting nothing. `wait_for_terminal` correctly
answers `Wait::Ready`, `event::poll` correctly answers "nothing", and `next()`
spins: **380,987 of 381,501 waits** at 100% of a core with the user's keystrokes
never delivered.

Reproduction: `pty_smoke::resizing_the_shell_reaches_the_harness_terminal` under
a tree that polls crossterm less than once per tick, plus the `FIONREAD` probe.
A fix was written and **taken back out** — 2 failures in 8 against 1 in 12
without it, samples too small to separate. The honest fix is upstream, or is
Glasshouse not sharing one crossterm poll between input and `SIGWINCH`.
**Opus specialist, its own packet.**

**Also open, and it is the only way to make the hangup residual structurally
zero:** nothing inside `next()` can close the window, because once crossterm is
wedged the main thread cannot observe the shutdown flag, a signal, or even a
closed descriptor — a closed fd makes that same `read` return `EBADF` and fall
through the same arm. Ending the process from outside a wedged loop needs a
watchdog thread blocked in `poll(fd, events: 0, -1)` — `POLLHUP`, `POLLERR` and
`POLLNVAL` are reported whatever is subscribed — that requests shutdown on wake
and takes `shutdown::force_exit` if the loop has not ended within a tick or two.
`force_exit` is private and `shutdown.rs` is Red tier (§61).

**5. Phase 9J line 572 is probably in the wrong phase.** "Keep evidence for the
same nominal model distinct across different harnesses, gateways,
quantizations, model revisions, or protocol translations" is an
evidence-*storage* requirement and is nearly word-for-word Phase 33A's *"Keep
metrics distinct for materially different model versions, quantizations,
routes, or changing stealth-model identities"*. Whoever builds 33A closes both
or neither. Leaving it in 9J makes that phase read one line further from done
than it is. **A map edit, so it needs the user.**

**6. The residual `SIGABRT`, 1 in 37 runs.**
`pty_smoke::a_direct_provider_profile_reaches_a_real_child_and_only_that_child`
fails with the child killed by signal 6. It is **not** the drain race that was
just fixed. Four hypotheses are already ruled out with data — the `EIO` theory
(600 trials), a non-blocking master fd, `malloc` between `fork` and `exec`
(2400 spawns), and mislabelling — and `report-PTY-FLAKE.md` §6 ranks where to
look next, starting with `std::env::set_var` in a threaded test binary.

**7. Phase 9I line 528** is the last free-pool line: `Allowance` separates
request pools from token-priced allowances and only the request-pool half has a
production feed. It needs a source for "this credential is priced per token".
Deliberately not solved by parsing rate-limit headers on the forwarding path —
the gateway forwards headers without reading them, and a parser there would
make it a reader of the payload it exists to pass through. Possibly a Phase 32
job rather than a 9I one.

**Still blocked on Phase 39:** Phase 21's `809` (configurable cheap or local
model) and `817` (extraction after task completion) close together. The trigger
is built, proven, reachable and consults the routing policy on every completed
task — and dead-ends every time, because nothing can supply a model at a turn
boundary. `818` is blocked two phases deep on Phase 7 line 307 and Phase 8
line 324.

**Before sizing any packet, read §32 and §36 together**, and then §43: extract
every `YOURS` list from the round's packets and intersect them pairwise. Two
workers were given `shell/state.rs` last round; it did not bite, and that was
luck rather than design.

**Phase 4 gained its unfocused-control lines.** `m` and `c` in the session
overview act on a session the viewport is not showing, and `N` /
`glasshouse launch --headless` runs a harness that never takes the terminal.
The load-bearing change was the smallest: the overview used to highlight the
same index that drives focus, so "send text without focusing it" was not
expressible at all until the overview got its own cursor.

**The interrupt box is deliberately still open.** Every interrupt test in the
suite is `#[cfg(unix)]`, so Windows compiles them and runs none — a green
`test (windows-latest)` is the absence of evidence wearing the same colour.
ConPTY's `PSEUDOCONSOLE_WIN32_INPUT_MODE` path has never executed.

**Two Phase 9F mechanisms landed with both boxes open, and the reason is the
transferable part.** `resolve_checked` was about to be wired into
`launch_session`, where `session::select::select` has *already* resolved the
executable and errored if it were unusable — so the call would have passed
`Usable` unconditionally and the refusal could never fire. Line 466's verb is
*offering*; line 465's is *starting*. **When a mechanism's proposed call site
cannot produce the failure the line describes, the box is not close to done.**

**Phase 9D closed at fourteen of fourteen.** A provider connectivity test is a
real bounded request now rather than a precondition check, a model list can be
refreshed manually, and the catalogue is cached in the data directory with a
timestamp so starting Glasshouse issues no request at all. Proven against the
shipped binary in a real terminal: 417 models fetched live from OpenRouter, a
refused host reported as refused, and **an endpoint that accepts and never
answers bounded at the shipped ten seconds while the interface kept tracking
keystrokes.**

**One evidence promotion was withdrawn on review, and it is the finding worth
carrying.** See the correction entry in the evidence ledger and practice §23:
a control has to be run against the host it is being used to justify.

The local gateway now serves **all three wire protocols** — Anthropic Messages,
OpenAI Responses and OpenAI Chat — from one upstream holding one credential,
proven end to end against OpenRouter with two real harnesses over the same
gateway in one run.

**One thing needs the user.** A real conversation identifier of theirs is
committed in git history (one identifier, one commit, working tree already
scrubbed). The repository is **private**, and the value is not a credential —
it names a local SQLite file and grants no access. Whether that warrants a
history rewrite is the user's call; the orchestrator will not force-push
unattended.

Three workers now run concurrently, partitioned by the files they touch —
see `docs/process/orchestration-measurements.md`, which is a standing inherited
experiment and not a one-off note.

The user signed the Antigravity CLI in, which unblocked Phase 9.

The README now carries a progress bar generated from this map by
`scripts/progress.py`, checked in CI. **Run it after every map change or the
lint job fails.** `main` clean. Phase 8 is nine of ten, Phase 6 twelve of
thirteen.

Phase 9A's nine open lines are open for recorded reasons, each with the phase
that unblocks it: **350** (9K), **353** (9C/9D), **355**, **359**, **360**,
**363** (9F), **365** (9J/9K), **369** (34-37).

**A behaviour change worth stating plainly.** Every Codex session Glasshouse
starts now carries `--approve-for-me`, and every Claude Code session
`--permission-mode auto`, because the default launch profile selects the
harness's own automatic-review mode. Verified against the real Codex — the
session came up and showed Codex's own trust prompt — and the end-to-end PTY
test asserts the exact argv.

## Verified completed work

### This session — a connectivity test that makes a request, and a promotion that had to come back off

Phase 9D's last three lines, closing the phase at fourteen of fourteen. The
2D batch had shipped an honest placeholder — a precondition check whose own
screen text said "Glasshouse has no HTTP client" — and `ureq` arriving with the
gateway made that sentence false. It is a real request now.

**The hazard the packet existed to prevent did not happen, and was proved not
to.** Three network calls were added to a settings screen; a blocking call on
the drawing thread would have frozen the terminal, which is the class of bug
Phase 9E shipped once already. `spawn_provider_probe` moves every request to
its own thread, and the proof is not an argument: against a Python listener
that accepts a connection and then never writes a byte, **three `Down` presses
each moved the cursor while that socket was open**, and the probe came back at
`no answer within 10004ms` — the shipped `RESPONSE_TIMEOUT`, not a test value.

**Three timeouts, not one.** Connect (5 s) and response (10 s) bound the phases
a stall is likely in; a 20-second global ceiling bounds the one nobody thinks
of — a server that answers its head promptly and then dribbles the body forever
satisfies the other two indefinitely.

**The cache is in the data directory and cannot fetch.** `ModelCache::load`
returns `Option` and **has no error type at all**: absent, truncated, wrong
version or filed under another provider all mean "no cache hit, carry on". The
module has no HTTP client, which is a stronger guarantee than remembering not
to call one. Verified by restart: `fetched_at` and the file's mtime both
unchanged at `1787731823` after a fresh process start.

**A provider name is untrusted input reaching a file path.** `file_stem`
slugifies to `[a-z0-9-]` and appends 16 hex characters of a SHA-256 of the
original, so `my provider` and `my/provider` land in different files and
neither can contain a separator or be `.` or `..`.

**One of six evidence promotions was withdrawn.** The batch promoted six
`model_list_endpoint` declarations from live probes; the orchestrator re-ran
all six in under a minute. Five reproduced exactly. z.ai had answered `401`
rather than `200` and was promoted on a control — *"a host that served nothing
there would have answered 404"* — **cited from a probe against a different
service**. Against z.ai every path under `/api/paas/v4/` answers `401`,
invented ones included, and a nonexistent API version answers `200`. The `401`
discriminates nothing, so the claim is back to `Unverified`. The base URL is
untouched; only "a model list is served at `<base>/models`" is withdrawn, and
establishing it needs one authenticated request.

The user-visible consequence is a better answer, not a worse one: the z.ai row
now reads `no model-discovery endpoint established for this provider` where it
had said `none cached — press m to fetch` — an invitation to press a key that
would have fetched a `401`.

**Two defects the team lead found by running the binary**, both the shape this
project's history predicts: a result line that read "reached … unreachable" in
one sentence, and a row advertising a refresh key for a provider that cannot
refresh. Both fixed with a test and a mutation each.

**Thirteen mutations by the lead, all killed; three more by the orchestrator.**
The orchestrator's second one is worth noting — it made the caller *join* the
probe thread rather than running the probe inline as the lead's did, so the
responsiveness guarantee is now proved two independent ways.

### This session — the macOS Keychain, and a hang that would have frozen the TUI

Three Phase 9E lines. Credentials now resolve from the operating system's own
secure store where one is available and from the environment where it is not,
with the fallback **labelled** — `glasshouse doctor` prints "credentials resolve
from: the macOS Keychain, then the process environment".

**The defect that justifies the run-the-binary rule on its own.** `doctor`,
pointed at a provider whose credential was in the Keychain, hung indefinitely —
no output, no visible dialog. `SecKeychainFindGenericPassword` decrypts the
item, decryption consults its access control list, and for an item this binary
did not create the call blocks waiting for an authorization dialog a piped
process never shows. The same read is on the path that starts a session, where
it would have frozen the TUI. One `SecKeychainSetUserInteractionAllowed(0)`
makes it fail cleanly and fall back instead.

**A durability caveat, measured rather than assumed.** The ACL binds to the
binary's code identity, so for an unsigned build — which Glasshouse is today —
a rebuild breaks the link. Store, rebuild, read: does not read. For a signed
release the designated requirement should be stable across versions, and that
is explicitly *not* claimed. When configuration records a credential the store
will not return, `doctor` says so and says what to do.

**The orchestrator supplied the production caller.** The packet forbade
`main.rs`, so the batch flagged rather than reached — `launch_session` now
builds `PreferNativeSecretStore::detect()`. Without it the preference would
have been true of the store, of `doctor` and of settings, but not of
`glasshouse run`.

**Windows and Linux stay unchecked**, as the packet required. Neither is
provable from this machine, and `LOCALLY VERIFIED` with the platform gap
recorded is the honest state.

### This session — settings that manage providers and profiles, and a test that was passing for the wrong reason

Four Phase 2D lines: Providers and Launch Profiles sections, with add, edit,
disable, duplicate, remove. Phase 9D's connectivity-test line stays **open** —
the branch had no HTTP client and the packet forbade adding one while the
gateway batch was introducing `ureq`, so the affordance is an honest
precondition check and says so on screen.

**The orchestrator's own mutation found a weak test.** Acceptance test 7 plants
a real credential, drives nine settings screens and asserts the value never
appears. It survived a mutation that renders the value instead of
`set`/`not set` — because at 100 columns the providers row is **truncated**, so
a leaked 46-character value was clipped off-screen. The test was passing for a
reason unrelated to the code. Every snapshot is now captured at a realistic
*and* a wide size, and the mutation is caught in both directions.

**A test that asserts the absence of a string in rendered output is only as
strong as the viewport it renders into.** Truncation makes absence trivially
true. That is new to this project's practice and now written down.

**Three defects came from running the binary**, which is why the packet demands
it: a stale banner left the profile wizard silently un-drivable, `cmux` was
accepted as a launch-profile harness because validation used every integration
rather than only harnesses, and a long refusal message rendered off-screen
because the input panel's height was a fixed constant.

### This session — a gateway that holds the key, so the harness never has to

Ten Phase 9G lines. A Claude Code session launched under a gateway-backed
profile now gets `ANTHROPIC_AUTH_TOKEN` = **the gateway's own per-instance
token**, never the provider key. The gateway checks that token and attaches the
real credential itself, resolved through `SecretStore` and never leaving the
process. A request with the wrong bearer is refused **before any upstream
connection is opened** — and the test asserts that on the fixture's *connection
count*, not on the status code.

Still no async runtime: blocking threads, one per connection, with `ureq` for
the outbound hop because its body is an incremental `Read`. +26 lock packages,
the unavoidable price of TLS.

**The survived mutation was the most useful result.** Removing
`set_nonblocking(false)` from an accepted socket broke nothing — every test
wrote its request before the gateway accepted, so the bytes were already
buffered. A real harness connects first and writes after. A new test pauses past
one accept poll before writing, and the mutation then fails.

**Two real defects, found by building rather than reasoning.** The test fixture
had the very platform bug the production code documents, and it looked exactly
like a flaky network test. And Nagle's algorithm was stalling every streamed
event, because the response head was written field by field with `TCP_NODELAY`
off — a latency defect in precisely the property the streaming line promises.

**`redact` is not enough, and a test written to prove the seam caught it.** It
removes credential-shaped runs and says nothing about the text around them; a
captured line had the credential redacted and a planted prompt body verbatim.
Transport details are now one of eight `&'static str` phrases written in that
file, so a leak is not something to be careful about — it is something the
function cannot express.

**A caching trap that could have poisoned every mutation verdict.** A
subcontractor pointed `CARGO_TARGET_DIR` at the repo's shared `target/` and
cargo served a cached test binary built from mutated source. It caught this
itself; the lead then reproduced it deliberately, found that restoring a file
with `mv` puts back the original mtime, made its runner `touch` every source,
deleted `target/` and re-derived every number from a clean build. Practice §16.

### This session — an identifier read from an index, without opening a single conversation

Phase 9 lines 2 and 3. `NativeSessionSource` is now an enum over two shapes:
Codex's walk-and-filter, unchanged byte for byte, and a new `SharedIndex`
variant that reads **exactly one named file** and never calls the directory
walk. That matters because Antigravity's records are
`conversations/<uuid>.db` — the user's own private conversations — and an
earlier packet had asked for the walker to be pointed straight at them.

**The identity guard is two rules and both are load-bearing.** The index has no
per-entry timestamps, so: the index file's mtime must fall inside the session's
window, *and* this project's entry must have changed during it. Rule 1 alone
has a real hole — the mtime moves when any project's entry changes — and rule 2
closes it, because a stale entry is by definition unchanged.

**Antigravity honours no environment variable for its state root.** The lead
searched the 1.1.20 binary for every plausible name and found none, so
`home_env` became an `Option` rather than gaining a fifth invented declaration.

**A design rule of mine was too broad and is now corrected.** "No log line, no
diagnostic" for a conversation identifier collided with two pre-existing log
lines, one carrying a deliberate comment that the identifier is what makes a
failed resume diagnosable. The lead reported it instead of choosing. The lines
stay: the identifier is not a credential, and the property the rule should have
stated is *never log the index's contents, and never log another project's
identifier.*

**Two things found that were not this batch's job.** A real conversation
identifier of the user's was already committed in git history — spotted by a
subcontractor that refused to reuse the literal in a fixture. And an existing
Codex resume test could pass vacuously if its harness never started; the
orchestrator hardened it, since the batch correctly declined to touch a test
outside its scope.

### This session — the gateway process, built by a team lead with its own subcontractors

Seven of Phase 9G's nineteen lines: the local gateway *process* — loopback-only
listener, ephemeral port, per-instance token, and the lifetime of all three.
No ingress; that is the next slice.

**Line 2 is structural rather than promised.** The module imports none of
`crate::session`, `crate::shell`, `crate::tui`, `crate::harness`, enforced by a
source scan with a paired vacuity test. A module that cannot see the session
model cannot own a session.

**The packet was wrong and the worker measured its way out.** It said "a
connection that arrives is closed immediately" — impossible for a listener
nothing accepts, because the kernel completes the handshake into the backlog by
itself, so `connect` succeeds. The honest behaviour was measured and asserted
instead: the gateway never sends a byte, checked by reading *after* the drop,
which catches a gateway that greeted its client without needing a sleep.

**A latent hazard in existing code, found and correctly left alone.**
`shutdown`'s `FORCED_EXIT_CLEANUP` is a single slot: registering there would
have displaced the harness-kill callback an attached session installs, and
dropping the gateway would have unregistered it — orphaning a real harness on a
second Ctrl-C. Harmless today only because there is exactly one caller.
**The next slice that adds a second caller must fix that API.**

**The team-lead experiment paid, on evidence.** Three subcontractors, none with
write access to the same file; the lead kept the listener, the token, the
predicate, the shutdown decision and every mutation. **Two of ten mutations
survive the lead's own tests entirely and die only to a subcontractor's test** —
delegation bought coverage the lead demonstrably did not have. A subcontractor
also caught a 45-in-100 flake in the lead's own `Debug` test: it scanned
prefixes of a *generated* hex token, and `[redacted]` contains four of the
sixteen hex digits. The orchestrator ran the suite 40 more times: 0 failures.

**One process lesson worth carrying:** a subcontractor snapshotted the lead's
worktree *mid-mutation* and captured a deliberately broken tree. Snapshot before
mutations begin, or have subcontractors work from a git ref.

### This session — a gateway a harness can actually reach, and a header that cannot forge another

Five lines across two phases, and the one that matters most is not a template:
**OpenRouter serves Anthropic Messages at `https://openrouter.ai/api`** — the
root, no `/v1`, because Claude Code appends `/v1/messages` itself. Established
twice over: an unauthenticated POST to `/v1/messages` answers 401 while a
nonexistent path under the same prefix answers 404, and the user's own working
launcher drives the real Claude Code against exactly that root. So
"Claude / OpenRouter" (9A line 353) is now a profile that resolves, and Phase
9F finally has a real backend to be proven against.

**NVIDIA and LiteLLM templates**, both read from the vendors' own docs. NVIDIA
is `openai-chat` only, so a test asserts the honest consequence — it cannot
back Codex. LiteLLM's base URL is written as read (`http://0.0.0.0:4000`), and
its `credential_env` is deliberately empty because the docs reuse the generic
`OPENAI_API_KEY`, which Glasshouse must not read for a local proxy.

**Headers are overridable, and CR/LF is refused rather than escaped.** A
newline inside a header value would forge a second header into every request,
so `unsafe_header_value_char` rejects control characters outright. Both
delivery mechanisms were verified off the wire beforehand:
`ANTHROPIC_CUSTOM_HEADERS` as newline-joined `Name: value` lines, and Codex's
`-c model_providers.<id>.http_headers` inline table.

**Line 355 closed end to end at last.** It had stayed open because no shipped
profile could populate `env`; a direct-provider profile now can. A pty_smoke
test resolves one, spawns a real child, and asserts the base URL and credential
arrive **in the child**, the parent's environment does not carry them, and
`PATH` is untouched.

**Thirteen mutations, thirteen kills**, plus two re-run independently by the
orchestrator — disabling the CR/LF guard killed its test, and adding `/v1` back
to the OpenRouter root killed two independent tests at different layers.

**Two forbidden-file findings, both correct and both flagged rather than hidden.**
Adding a field to `Provider` forces every exhaustive struct literal to change,
including one inside `secret/mod.rs`'s tests — unavoidable. And the batch's own
design change broke an unrelated pre-existing test whose `.take(5)` window was
sized for a one-protocol world; replaced with a `take_while` that is correct for
any number.

**A known, bounded inconsistency is recorded rather than smoothed over:** header
validation runs at the config boundary while credential-variable validation runs
at resolve time. It is bounded because the only production constructors of a
`Provider` are `to_provider`, which validates, and `templates()`, which a test
pins to carry no headers. If a third is ever added, header validation must move
to resolve time too.

### This session — the gateway keys become usable, and one defect caught on the way

Phase 9F is the join Phases 9A, 9C, 9D and 9E were building towards: a launch
profile can now name a configured provider, and a real harness starts against
it. Eleven of its thirteen lines closed.

**Every mechanism was probed, not recalled.** The installed harnesses were
pointed at a local HTTP capture server and what they actually sent was read
off the wire. That settled four things no amount of reasoning would have:

- `ANTHROPIC_BASE_URL` is the **root** — Claude Code appends `/v1/messages`
  itself, so a helpful `/v1` would have produced `/v1/v1/messages`. A provider's
  declared base URL goes through verbatim, and a mutation that appends a path
  kills a test.
- `ANTHROPIC_AUTH_TOKEN` **wins over the user's claude.ai login for that child
  and leaves it untouched on disk** — the harness said so itself. No
  `x-api-key`, and the user's own credential was never sent.
- **Codex needs no generated file at all.** Six `-c` overrides do the whole
  job, every one accepted under `--strict-config`, which rejects keys it does
  not know. "Avoid overwriting `~/.codex/config.toml`" is satisfied by there
  being nothing to overwrite.
- **`wire_api = "chat"` is gone in Codex 0.149.1.** A provider serving only
  `openai-chat` cannot back Codex, so Glasshouse refuses that pairing instead
  of composing a configuration Codex would reject after the process started.
  Every built-in template is chat-only today, so no template can back Codex —
  correct rather than a gap.

**And Codex refuses a missing credential itself** ("Missing environment
variable: `…`") rather than falling back to the user's paid account, which
corroborates the "clear launch error" line from the harness's own behaviour.

**A defect caught before it shipped.** Phase 9A gives every Claude Code session
`--permission-mode auto`. Composed with 9F, **every gateway-backed session
would have come up with its tools blocked** — auto mode's classifier is a model
call a third-party gateway cannot serve as Anthropic would. The user's own
working gateway launcher avoids auto mode for exactly this reason. `resolve` is
now backend-aware: a defaulted profile on a non-Native backend adds no approval
argument and records why, an explicit request is refused rather than silently
dropped, and `Bypass` is unchanged. Keyed on the **backend**, so 9G inherits it.
Recorded as a strong reading corroborated by a working implementation — not as
a controlled experiment.

**The secret boundary is structural.** An adapter is handed variable *names*
and returns a *placement*, never a value, so it has nothing to leak.
`profile::resolve` is the only place in Glasshouse where a `Secret` exists —
exactly one production `.expose()` call in the crate, verified by grep. The
leak test plants a known value and asserts its absence from the overlay's
`Debug`, every mechanism note, every argument, and the `Display` and `Debug` of
all fourteen `Refusal` variants, then proves it *is* in the child environment
by comparison rather than by printing it.

**Sixteen mutations, sixteen kills**, plus two re-run independently by the
orchestrator against the integrated tree — the `Debug`-prints-values mutation
and the silently-skip-a-missing-credential mutation both failed their named
test. Restoration was per-file from a byte-compared backup, never a path-wide
`git checkout`.

**The worker corrected its packet three times and was right every time**: the
name check could not live in the adapter (`direct_provider_launch` returns
`Option` and has no error channel, so a refusal there could only be spelled
`None`); `secret/mod.rs` needed more than a doc change (a `Secret` cannot be
minted outside its module, so no external test can implement `SecretStore`);
and acceptance test 8's premise was wrong (the other adapters are refused one
step earlier, at the protocol intersection). That is four sessions running.

**What is not proven, stated plainly.** Neither path has run against a real
backend *through Glasshouse*. For Codex that is currently impossible. For
Claude Code it is now possible and was not before: **OpenRouter serves
Anthropic Messages at `https://openrouter.ai/api`** — an unauthenticated POST
to `/v1/messages` answers 401 while a nonexistent path under the same prefix
answers 404. No template declares it yet; that is Phase 9D's, and it is the
thing that would close this end to end.

### This session — wrappers and shims, and a name that reaches a command line

Phase 9B closed whole. `glasshouse run` and `glasshouse launch` share **one**
dispatch arm through an or-pattern, so line 390's "same behaviour from the TUI,
`glasshouse run`, or a shim" is a compile error to violate rather than a
review note. `glasshouse shim` writes one small file into a directory the user
names, containing nothing but an `exec` back into `glasshouse run`.

**The real binary:** a 125-byte, mode-0755 shim whose entire contents are
`#!/bin/sh` and one `exec` line — no secret, no URL, no routing logic — and a
message saying the exact path and that deleting it is all it takes.

**A profile name is untrusted input reaching a command line.** The worker
flagged that it had quoted but not escaped the names it interpolates, and
judged a general shell-escaper out of scope. That judgement was right and the
answer was not escaping: this codebase already **refuses** this class of input,
in `platform::exec`'s rejection of `cmd.exe` metacharacters. `check_name` now
refuses anything outside `[A-Za-z0-9._-]` before a byte is written, and names
the offending character. Verified against the binary:
`--profile 'evil"; id; echo "'` is refused.

**Six mutations, six kills.** One verdict had to be re-read: the first pass
showed the lib target's result line, which had filtered the test out, while the
kill was in the **bin** target. Read the named test's own line, in the target
that actually runs it.

### This session — launch profiles, and a vertical slice that reaches production

Phase 9A's abstraction landed with its production caller in the same batch,
deliberately: a mechanism nothing calls does not get its box, and this project
has already paid that price twice with `SessionRuntime` and Phase 1 line 90.

- **A profile is data; an overlay is its resolution.** `resolve(profile,
  adapter, acknowledged)` is the only place a declaration becomes arguments,
  and it **refuses rather than invents** — six refusal variants, every one
  naming the harness and what was asked for.
- **A default that falls back is not a request that is refused.** An explicit
  automatic-review request on a harness that has none is refused; a profile
  that merely took the default gets no approval argument at all, never a
  bypass.
- **A bypass needs an acknowledgement**, per harness, **user layer only** — a
  repository must not pre-acknowledge a blanket bypass for whoever clones it.
- **Only `Native` resolves today.** `DirectProvider` and `GlasshouseGateway`
  are representable and refused with a diagnostic naming the phase that
  supplies them.

**Verified against the real harness, not a fake one.** `glasshouse launch
codex` was run from the built binary in a real terminal: Codex started with the
injected `--approve-for-me` and displayed **its own workspace trust prompt** in
the viewport — a native prompt staying interactive, which is the product
invariant. It was declined; nothing was trusted. `glasshouse sessions` then
showed `PROFILE = native`, `--profile bogus` was refused while leaving the
session count unchanged, and the mechanism diagnostic read back from a real
log.

**Nine mutations, nine kills.** Two of the nine tests were added by the
orchestrator, because two lines had no guard at all: line 362 (a source scan
proving `profile/mod.rs` never touches the filesystem, so it cannot modify the
user's global harness configuration) and line 371 (a configured gateway
profile never displacing the implied Native one).

**The worker was right against the packet, and honest about a gap.** Its own
test comment records that no shipped profile can populate `env` yet, so the
overlay had to be built directly to prove the mechanism — which is exactly why
line 355 stays unchecked while line 356 (arguments) is closed.

### This session — the approval declaration had to carry argv, not prose

Phase 9A must *select* a harness's approval mode, so the first thing checked
was what the adapters actually declare. Three of seven could not be used as
launch arguments at all:

- **Claude Code declared `auto-mode`** — a *subcommand* ("Inspect or reset auto
  mode classifier configuration"). Appending it to a launch would have run the
  subcommand instead of starting a session. The flag that selects the mode for
  a session is **`--permission-mode auto`**, one of six choices.
- **Codex and Cursor declared their sandbox as usage strings** with
  placeholders (`-s/--sandbox <read-only|workspace-write|danger-full-access>`)
  that no process can receive.

A mode is now `ApprovalMode { args, description }` and the sandbox a
`SandboxSelector { flag, values }`; `HarnessAdapter::approval_args` answers
`None` — never a substitute — for a mode a harness lacks.

**Verified against the real binaries, both directions.** `claude
--permission-mode auto` is accepted while `--permission-mode bogus` is rejected
with the allowed list; `codex --approve-for-me` is accepted **through the cmux
PATH shim**, which incidentally settles a recorded worry — the wrapper does not
swallow a flag Glasshouse adds.

**Five mutations, five kills.** Reverting Claude Code's argv to the subcommand
kills two separate tests; turning an argv back into a usage string, making
`approval_args` fall back to the bypass, and giving a description a backtick
each kill their own.

**Running the binary caught two defects the types could not.** Descriptions are
rendered inside backticks, and Claude Code's and Cursor's own descriptions
contained backticks, so both rows printed doubled. Both are plain prose now, a
guard test prevents a recurrence, and the row shows the description **and** the
argv — a diagnostic that hides the half reaching the process is the weaker one,
and this row previously named a subcommand.

This is the **third** declaration derived from an artifact that did not serve
the purpose it was cited for, after Antigravity's executable name and Codex's
snake_case hook events. The rule it earns: *before a declaration is used, check
that its evidence supports the use, not merely the claim.*

### This session — the permission cycle, watched from both ends

Phase 8 line 8 closed. `glasshouse launch codex -- --sandbox read-only
--ask-for-approval on-request` started a session Codex reported as "Read Only",
which incidentally proves the `--` pass-through reaches the harness in
production. Asked to create a file, Codex raised its own approval prompt and the
record moved to **`lifecycle = 'waiting_for_user'`**; on approving, the file was
created and the record moved to **`idle`**.

`running -> waiting_for_user -> idle`, every transition written by a hook Codex
fired, none of it inferred from the screen — which
`nothing_derives_session_state_from_terminal_output` makes structurally
impossible anyway.

### This session — Codex lifecycle hooks, watched running end to end

Three Phase 8 lines closed: integrate hooks, translate events, detect turn
completion.

**The chain was watched, not argued.** `glasshouse launch codex` was run
against the real Codex 0.149.1 with `project_hooks = true`. Glasshouse wrote
`<project>/.codex/hooks.json` — five events, `timeout: 3`, every path pinned —
Codex asked to trust the directory, then asked to review the hooks, and after
one real turn the session record read **`lifecycle = 'idle'`**.

That settles it rather than suggesting it: the only production code that writes
`Idle` is the `Stop` arm of `lifecycle_for`. Generate, install, trust, fire,
report, translate, record.

**Quitting cleanly then captured the native identifier from a live session** —
`01a03983-b696-7832-ac49-296a4deccda1`, verified to be the exact rollout Codex
wrote (`originator: codex-tui`, no `parent_thread_id`, matching `cwd`), with the
session reading `resumable`. That closes the last open gap on Phase 8 line 2 as
a side effect.

**Most of the translation needed no code at all.** Codex spells
`UserPromptSubmit`, `PermissionRequest` and `Stop` exactly as Claude Code does,
so `lifecycle_for` already handled them. Only `SessionStart` was added — Codex
fires it and Claude Code does not. `SessionEnd` is deliberately left unmapped:
the operating system reporting the process is the authority for a session
ending, and a hook only races it.

**Two things the harness told us that no amount of reading would have.** Codex
clamps hook timeouts, announcing `clamping SessionEnd hook timeout to 3s`, so
the declared timeout is 3 and a real installation warns about nothing. And hook
trust is a prompt distinct from workspace trust, which is why the project-local
design needs no user-level write at all.

**Four mutations, four kills** — removing the consent gate, raising the timeout
to one Codex would clamp, mapping `SessionEnd`, and making the handler read and
log its payload. The payload scan was additionally hardened to assert the slice
it scans is the real function, because a scan over the wrong span passes for the
wrong reason.

### This session — every adapter declares its approval modes

Phase 6's new line, closed. `ApprovalModes` carries `automatic_review`,
`bypass` and `sandbox` as `Declared<&'static str>`, all seven adapters fill it
in from their own binaries, and `glasshouse doctor` prints it.

The distinction is the point: **three harnesses classify, four only bypass.**
Claude Code's auto mode, Codex's `--approve-for-me` and Cursor's
`--auto-review` are automatic review; OpenCode's `--auto`, Hermes's `--yolo`
and Antigravity's `--dangerously-skip-permissions` are not, and are recorded as
bypasses only.

**A mutation caught a weak test, which is what mutations are for.** The first
version asserted only that an `automatic_review` evidence string avoided the
words "yolo", "dangerously" and "bypass". A mutation recording OpenCode's
`--auto` as automatic review — evidence reading "…(dangerous!)" — walked
straight through it, because "dangerous!" is not "dangerously". The fuzzy check
was replaced with an exact harness-by-harness table, and the same mutation now
fails. That weak test was specified by the orchestrator's own packet, not
invented by the worker.

**And running the binary caught an overstatement before it shipped.** The first
rendering printed "no automatic review" for anything `Unverified` — but
`Declared` cannot say "verified absent" for a mode name, so `Unverified` means
nobody established one. Pi makes the difference concrete: installed, not on
`PATH` here, `--help` unreadable. It now reads "automatic review unverified",
matching the convention the neighbouring `capabilities:` line already used.

### This session — resuming a Codex session, which cost no production code

Phase 8 line 3 closed without a line of new production code, and that is the
Phase 6 adapter contract paying for itself. `resume_session` selects the
harness the *record* names and asks its adapter; `Codex::resume` already
returned `["resume", <id>]`. The only thing missing was an identifier, and
line 2 supplied it.

**Codex resumes with a subcommand, Claude Code with a flag.** That difference
is exactly what the contract exists to absorb, and it is now asserted rather
than assumed: the test fails if a Codex invocation is ever handed
`--resume`, with an assertion message that names the failure as one harness's
vocabulary leaking into another's.

The test is deliberately **not** `#[cfg]`-gated. Windows CI found a real defect
on this same rollout-fixture path for line 2, so there is a concrete reason to
keep proving it on all three platforms rather than only where it was written.

Three mutations, three kills: Codex given Claude Code's flag, Codex returning
no resume arguments, and Codex resuming a different conversation.

**Verified against the real Codex 0.149.1, at no model cost** — a known
identifier replays the conversation, an unknown one answers `ERROR: No saved
session found with ID <id>`. Two traps recorded with it: a pseudo-terminal with
no window size makes Codex draw nothing and look hung, and its update prompt
defaults to an option that runs `curl … | sh`.

### This session — the Codex session identifier, and the rule `cwd` alone cannot express

`session::native_id::discover` finds the rollout a Glasshouse-started Codex
session wrote, and `capture` records it. Four conditions, all required:
`originator == "codex-tui"`, no `parent_thread_id`, `payload.cwd` canonically
equal to the project root, and `payload.timestamp` inside the window between
Glasshouse starting the session and observing it end.

**Two or more survivors means nothing is recorded.** Not "take the newest" —
the failure mode of guessing is resuming a stranger's conversation, and
`session::select` and the resume identifier resolver already refuse ambiguity
for the same reason.

**Only the first line of a rollout is ever read**, capped at 1 MiB. Everything
after it is the user's own conversation, and
`nothing_is_read_past_the_first_line` is what keeps that a boundary rather than
a habit.

**Discovery runs once, at session end, from both producers** — `launch_session`
and the shell's `poll_exits` loop. That is when the identifier is needed (a
stopped session is `Resumable` only if it has one) and when the window is
two-sided and therefore tightest. Codex writes no rollout until a turn has
happened, verified again this session under an isolated `CODEX_HOME`, so there
is nothing to find earlier.

`session::store::set_native_session_id` finally has a production caller; it had
been unused since Phase 2.

**Eight mutations, all eight killed** — including deleting each of the two call
sites in turn, which is what makes the wiring proved rather than asserted. The
first attempt at the mutation harness was itself defective in two ways worth
recording:

- it restored each mutation with `git checkout -- crates/glasshouse/src/`,
  which — because workers are told never to commit — reverted the worker's
  entire contribution to five tracked files rather than the one mutated line.
  Recovery meant asking the still-live worker session to rewrite them. **Never
  use a path-wide git restore in a worktree whose value is uncommitted.**
- its verdict logic grepped for `0 failed` across all four test binaries, which
  always matches the filtered-out lib line, so it reported "survived" for every
  mutation including ones that never compiled. **A mutation harness must read
  the named test's own result line**, and must distinguish `error: test failed`
  (the kill) from `could not compile` (no result at all).

**An adapter may no longer depend on the session model.** The first
implementation had `harness/codex.rs` importing `crate::session::native_id`;
no adapter on `main` imported `crate::session` at all. The two record types
moved to `harness/mod.rs` where the rest of the adapter vocabulary lives, the
RFC3339 parser became private to `codex.rs`, and
`no_adapter_depends_on_the_session_model` now scans all seven adapters, with a
paired test proving the scan fires on a fabricated `use` and stays quiet on a
doc comment.

**One worker judgement was better than the packet.** The packet asked for a
bidirectional consistency test between `session_id_source` and
`SessionIds::Discoverable`. Cursor, Hermes, Pi and OpenCode all correctly
declare `Discoverable` about their own harnesses without Glasshouse having
built a reader for each, so the converse is not a defect and the test is
one-directional by design.

**A worker's report can be written before it stops working.** The report file
appeared while the worker was still running its own mutation checks, and two
successive `git status` snapshots each showed a different call site missing.
Gate review on the pane going idle, not on the report appearing.

### Previous session — Codex, and a question it asks that Claude Code does not

Codex's startup handshake is `ESC[>5u`, `ESC[6n`, `ESC[?u`, `ESC[c`,
`ESC[0 q`. The `ESC[?u` is the kitty keyboard-protocol probe — a fourth
question, and Glasshouse **deliberately stays silent on it**.

Answering would be the obvious move and the wrong one. The reply means
"supported"; the harness would enable the protocol and expect key events
encoded that way, and `tui::event` sends ordinary bytes. The session would come
up looking perfect and then mis-read every keystroke.

Silence is not a hang here, because of the idiom Codex uses: it sends `ESC[?u`
and `ESC[c` together, and a device-attributes reply arriving with no keyboard
reply before it *is* the negative answer. So the device-attributes reply added
this session is exactly what lets Codex conclude "no kitty protocol" without
waiting. Two tests pin it, and the constant is named
`DELIBERATELY_UNANSWERED` so the next person to find an unanswered query has to
read why before answering it.

The real-harness viewport probe is now shared between Claude Code and Codex,
so both are held to the same check: the harness's own version string must be
absent before a session exists and present afterwards.

**Codex writes no session file until a turn happens** — starting it and killing
it left the rollout count unchanged. So its identifier can only be discovered
after the first turn, by matching a rollout header's `payload.cwd` against the
project and taking its `payload.id`. That is the next piece of Phase 8.

### Previous session — the hooks, observed firing for real

A Glasshouse session was opened against the real `claude` in a pseudo-terminal,
one prompt was submitted, and the session record moved from `starting` to
**`idle`**.

That value settles it rather than suggesting it. The only production code that
*writes* `Idle` is the `Stop`/`StopFailure` arm of
`session::lifecycle::lifecycle_for`; nothing else in Glasshouse can produce it.
So the record could only have reached that state by Claude Code running the
hook Glasshouse generated and installed, which invoked `glasshouse hook`, which
translated the event and wrote it down. Generate, install, fire, report,
translate, record — the whole chain, end to end, against the real harness.

**And one line closed by having nothing rather than something.** "Keep
terminal-text parsing only as a fallback" is satisfied because Glasshouse has
no such fallback at all: state comes from the operating system or from the
harness, never from reading the screen.
`nothing_derives_session_state_from_terminal_output` keeps it that way — the
runtime is the one component that sees terminal output, and it may not move a
session's state. Giving it a method that infers one fails the test.

### Previous session — answering the terminal's questions

A real Claude Code startup was captured in a pseudo-terminal and every escape
sequence it writes before drawing was examined. Three are *questions*:
`ESC[6n` (cursor position), `ESC[c` (primary device attributes) and `ESC[>0q`
(XTVERSION). Everything else — bracketed paste, focus reporting, synchronised
output, keyboard-protocol pushes — is an instruction.

**Glasshouse answered one of the three.** Phase 5's design note had already
written the rule down — "an embedded session must always answer, or the harness
hangs" — and only the cursor-position half was ever built.

The consequence was worse than a hang. Claude Code counts the failures and,
after two, disables its fullscreen renderer *globally*, writing that decision
into the user's own configuration where it outlives Glasshouse entirely. This
user's `settings.json` says `"tui": "fullscreen"`, so Glasshouse had overridden
an explicit preference of theirs, on their machine, permanently.

`TerminalQueryScanner` now recognises all three across chunk boundaries and
answers each: the emulated screen's cursor position; `ESC[?1;2c` for device
attributes, which is what the viewport actually is rather than a richer
terminal whose sequences it could not draw; and Glasshouse's own name for
XTVERSION, so an application that knows the name can decide for itself and one
that does not falls back to conservative defaults.

**Verified against the real binary, in an isolated Claude configuration so the
user's own was not touched.** Before, two sessions were enough to trigger the
auto-disable. After, three consecutive sessions left it absent, the failure
notice was gone, and with `"tui": "fullscreen"` set the fullscreen interface
rendered in the viewport with no notice at all. The isolated configuration was
deleted afterwards.

### Previous session — Claude Code lifecycle hooks

Glasshouse installs per-session hooks so a session's state comes from the
harness saying what happened, not from reading its terminal and guessing.

- The adapter builds the settings document, because its shape is the harness's
  own business. Glasshouse writes it into a directory it owns inside the
  project's state and passes `--settings` — which loads *additional* settings,
  so the user's own hooks keep running and their `~/.claude` is never touched.
- The hooks invoke Glasshouse itself (`glasshouse hook --session … --event …`)
  rather than a shell one-liner, because a one-liner would need different
  quoting on every platform and a harness's configuration is not the place to
  hide shell portability.
- `session/lifecycle.rs` is the only place that knows both vocabularies. An
  unfamiliar event changes nothing, and a late hook cannot revive a finished
  session — hook processes outlive their harness.

**A hook must always exit 0, and that is not a preference.** Claude Code treats
a non-zero exit as a veto: a `UserPromptSubmit` hook that exits non-zero blocks
the prompt outright, with the user's own words echoed back and nothing sent.
That was observed directly — and it is also what made the whole hook mechanism
verifiable *without spending a turn*, since a deliberately failing hook proves
firing while cancelling the API call.

**Two facts read from the real binary, not assumed:**

- `SessionStart` **does not fire** in Claude Code 2.1.245. A document declaring
  one was installed and its hook never ran, while `UserPromptSubmit` from the
  same document did. It is deliberately not among the reported events, and a
  test pins that.
- The hook schema was read out of a real settings document rather than
  recalled: entries hold `{type, command, timeout}`, and only tool events carry
  a `matcher`.

**A defect that only appeared by running it.** The first version of the hook
command carried no paths, so it discovered its own project from wherever the
harness happened to run it. It exited 0, looked healthy, and silently updated
nothing. Every path is pinned now, and dropping them fails two tests.

### Previous session — `glasshouse resume`

- `glasshouse resume <session>` reopens a recorded session in the harness that
  created it — not whichever harness is configured now, because resuming a
  Codex conversation in Claude Code would be nonsense.
- **The identifier resolver accepts any leading part of an identifier**, and
  that is a requirement rather than a nicety: `glasshouse sessions` prints only
  the first twelve characters, so the short form is the *only* identifier a
  user can copy off the screen. Running the shipped binary is what made that
  obvious. Ambiguity is refused and names every candidate.
- Matching uses `substr`, not `LIKE`. Under `LIKE` a bare `%` typed by the user
  would match every session in the project, and "resume whichever came first"
  is precisely the wrong answer.
- The order in `resume_session` is the safety property: the store decides
  whether the session may be resumed *at all* — right project, not still
  running, something to resume to — before a harness is selected and long
  before a process exists.

**A mutation that passed, and what it exposed.** Bypassing `open_for_resume`
entirely left `resuming_an_unknown_session_is_refused` green, because the
identifier resolver turns an unknown identifier away before the guard is ever
reached. That test proved nothing about the guard.
`resuming_a_session_with_no_conversation_is_refused` was written to reach it —
a Codex session, which has no identifier to resume to — and the same mutation
now fails. This is the fourth time a passing mutation has been information
about the tests rather than the code.

**One unreproduced failure, recorded rather than dismissed.** The resume smoke
test failed once, on the run that first compiled it, while clippy, rustdoc and
an MSRV check were building concurrently. It has not failed since in 23 further
runs (15 targeted, 8 full-suite). That matches the macOS `openpty` allocation
race this project already diagnosed and retried around, rather than anything in
the resume path, but it is written down because an unexplained failure that is
merely rare is not the same as one that is understood.

### Previous session — assigned native session identifiers (Phase 7)

**The whole chain is verified against the real binary**, with the user's
approval for the one step that needed a turn:

- `claude --session-id not-a-uuid` → "Error: Invalid session ID. Must be a
  valid UUID." The format requirement is enforced, not merely documented.
- `claude --session-id <minted> -p "..."` → Claude Code wrote its transcript to
  `~/.claude/projects/<slugged-cwd>/<minted>.jsonl`. The assigned identifier
  *is* the conversation's identity.
- `claude --resume <minted>` → reopened that conversation with its earlier turn
  replayed. `claude --resume <unknown-uuid>` → "No conversation found with
  session ID: …". Both observed in a real pseudo-terminal, neither costing a
  model turn.

- `HarnessAdapter::assign_session_id` is how a harness says it will take an
  identifier rather than invent one. Assigning beats discovering: the
  identifier exists before the process does, so a harness that dies during
  startup still leaves a named session, and nothing has to be parsed or
  watched for afterwards.
- `SessionStore::new_native_session_id` mints a valid RFC 4122 version-4 UUID
  from SQLite's randomness — the same source the store already uses. It is
  deliberately **not** derived from the Glasshouse session identifier: the two
  identifier spaces are independent by design, and a session's own name has to
  stay meaningful after the harness's history is gone.
- Both production start paths mint it, record it on the `NewSession`, and pass
  it to the harness. `a_claude_code_session_is_launched_and_recorded_under_one_identifier`
  runs the shipped binary and compares the identifier the harness *received*
  with the one Glasshouse *recorded*. **Mutation-checked in both directions** —
  either half alone is useless, and either half alone now fails.
- Claude Code's own binary enforces the format: `--session-id not-a-uuid`
  answers "Error: Invalid session ID. Must be a valid UUID." A minted
  identifier is accepted and the harness runs normally.

**A test's expectation changed for a good reason.** A cleanly stopped session
used to read `closed`, because nothing ever gave a session a native identifier.
It now reads `resumable`, which is the point of the work. The test asserts the
new truth and records why the old one was right at the time.

**Two smoke tests had to stop using a plain shell as Claude Code.** Glasshouse
now hands that harness `--session-id <uuid>`, and `/bin/sh` answers by printing
its usage. One test was re-registered under Codex — which names its own
sessions and so is started bare — because it is about resize reaching the
child, not about arguments. Worth knowing: **anything configured as
`claude-code` now receives that flag**, so a user's wrapper script has to pass
its arguments through.

### Previous session — the harness adapter interface

- `harness::HarnessAdapter` is the contract: `id`, `executable_candidates`,
  `start`, `resume`, `describe`, `message`, `interrupt`. The map's six verbs
  all land on it — observing is `describe().hooks` plus
  `describe().session_ids`.
- `IntegrationId::executable_candidates` **delegates to the adapter** for every
  harness. One place a harness's executable name lives, which is the phase's
  fixed requirement made structural rather than aspirational. The catalogue
  keeps names only for cmux, Ollama and llama.cpp, which are not harnesses and
  have no session to start.
- `HarnessSelection::start_args` is the single seam both session producers go
  through (`glasshouse launch` and the shell's `n`): the adapter's arguments,
  then the user's, so an explicit request always has the last word. No harness
  needs a start argument today, so the ordering rule is proven against a test
  adapter that does.
- `glasshouse doctor` prints every adapter's declarations. That is what keeps
  `describe` from being a data structure nothing reads, and it is generic over
  the trait — it cannot tell one harness from another.
- Two source-scanning tests hold the architecture: the generic PTY runtime and
  the session model may not name `HarnessAdapter`, `crate::harness` or
  `IntegrationId` in production code. Comments are stripped first, because
  `session/store` *documents* that it holds an identifier's string form — the
  boundary working, not breaking.

**Running the shipped binary found what the suite could not, again.** Two
rendering defects surfaced only from reading real `glasshouse doctor` output:
declarations rendering as nested backticks, and a session-id source phrase
that did not fit the sentence it was interpolated into. Both were invisible to
every test that passed.

**Five mutations were run and each failed its target**: giving Codex Claude
Code's resume flag; removing Antigravity's `agy`; restoring a hard-coded
executable name to the catalogue; making the doctor report's adapter loop
print nothing; and adding an `IntegrationId`-returning method to
`SessionRuntime`.

**Growing the catalogue moved the setup wizard toward a limit, so the list now
scrolls.** Ten integrations plus two section headers still fit an 80x24 screen,
but the margin is thin, and Ratatui silently draws fewer rows when a list
outgrows its area — an integration past the bottom edge would be one the user
can neither see nor toggle, with every test still green. The list is rendered
with its selection now, so it follows the cursor. Two tests hold it: all rows
present at 80x24, and every row still reachable at 80x12, where the list
genuinely truncates. Reverting to stateless rendering fails the second and not
the first.

**One scare that was not a defect.** The first version of that test asserted
every integration had a row, and failed on cmux — which the wizard
deliberately never offers unless it is actually detected. The layout was fine;
the test's premise was wrong. Worth remembering that a failing new test is a
claim about the code *or* about the test.

**A test was rewritten because it pinned a wrong fact.**
`antigravity_only_searches_the_literal_name` asserted the guess that a real
install disproved. It is now
`no_integration_is_searched_for_under_a_guessed_abbreviation`, which keeps the
hazard the original actually guarded — `ag` is the-silver-searcher on many
machines — and drops the guess.

### Earlier sessions — live sessions behind the interface

- `session::runtime::SessionRuntime` holds several live harnesses, each with
  its own reader thread draining its pseudo-terminal into its own bounded
  `Scrollback`. Focus is only a statement about which session the keyboard
  reaches; `focus()` touches no process.
- `shell::run` is its production consumer: `n` starts a session, session mode
  forwards keystrokes, resize reaches the focused child, ticks poll exits and
  refresh the viewport.
- Exits come from asking the process, never from output going quiet. A harness
  thinking in silence must not be mistaken for one that has finished.

**The defect end-to-end testing found, that unit testing could not.** The
session-mode escape chord was implemented as `Ctrl` + `']'` — which is what the
synthetic `KeyEvent` in its unit tests looked like. Crossterm's Unix parser
decodes the control range `0x1C..=0x1F` arithmetically, so a real terminal's
`Ctrl-]` arrives as `Ctrl` + `'5'` and never matched. A user entering session
mode had **no way back**: precisely the failure the single-chord escape exists
to prevent. Both spellings are now accepted and separately tested.

**A test written and then deleted, twice, for different reasons** — both worth
remembering:

- Asserting that switching sessions changes only the view requires reading the
  frame currently on screen. A full-screen Ratatui application repaints
  differentially, so a captured pseudo-terminal stream cannot be sliced back
  into frames by content, and the assertion silently read every viewport ever
  drawn. Phase 5 needs a real terminal emulator anyway; that is when this
  becomes testable.
- Asserting exit detection is independent of output needs a process that exits
  while its output stream stays open. A direct probe showed macOS reports
  end-of-file on the pseudo-terminal master as soon as the foreground child
  exits, even with a background child still holding the slave, so the
  discriminating case cannot be built there.

### Earlier sessions — the TUI shell

- `glasshouse` with no arguments opens the shell; piped or redirected runs keep
  the plain summary rather than drawing a full-screen interface into a file.
- Split like the first-run wizard: `shell::state` answers keys without drawing,
  `shell::view` draws without deciding anything. That is what makes the
  interesting behaviour testable without a terminal.
- The session bar renders the records `session::store` keeps, so Phase 3 reads
  what Phase 2 wrote — the two halves of this session meet in production, not
  only in tests.
- The overview draws *over* the shell rather than replacing it, so it reads as
  somewhere you leave rather than somewhere you go. Escape leaves the overlay
  while one is open and leaves Glasshouse only when none is.
- Selection follows a session's identifier, not its index. Sessions sort by
  last activity, so a refresh reorders them, and holding an index would move
  the user to a different session behind their back.
- The status bar carries the key bindings plus a note when a key could not do
  anything — pressing Tab in a one-session project explains itself instead of
  looking like a dead keyboard.

**Mutation testing rejected a piece of this code, which is the point of it.**
The status bar originally measured the remaining width and truncated a note to
fit. Removing that measurement changed nothing on screen, because Ratatui
already clips the row. The measurement is gone; the property that matters —
bindings are needed permanently, a note only once — is now carried by writing
the bindings first and letting the clip fall where it should, and swapping the
order fails the test.

**It also exposed two vacuous assertions, both the same mistake.** The
real-terminal check for the project root survived having the root blanked out,
because the project's name and its root's last component are the same string
and a bare `contains` matched the title bar. The same flaw let the
narrow-terminal test pass while truncating from the wrong end. Both now read a
single specific row or field. The lesson generalises: **asserting against a
whole screen is nearly always weaker than it looks.**

The text-first constraint is enforced mechanically rather than by assertion:
Ratatui's decorative widgets all draw with Unicode block elements, so the test
fails on any character in U+2580..U+259F, and adding a sparkline-looking line to
the viewport fails it.

### Earlier sessions — the session store

- `session::store` is Glasshouse's own record of the sessions in a project,
  deliberately not a view over any harness's session files. `native_session_id`
  is a nullable *reference*, so a record is complete before a harness has
  produced an identifier and stays valid after the harness's history is gone.
- **Project isolation is structural, not a query filter.** Migration 2 adds
  `BEFORE INSERT` and `BEFORE UPDATE OF project_id` triggers that abort any row
  whose `project_id` is not the identifier bound in `project_metadata`. No
  present or future query has to remember to filter. The comparison uses
  `IS NOT` rather than `<>` so that a missing binding aborts instead of
  evaluating to NULL and passing — mutation-proven, not merely argued.
- `SessionRecord::disposition` derives active/resumable/closed/failed from
  lifecycle plus the presence of a native identifier, rather than storing a
  second column that could disagree with the first. A stopped session with no
  native identifier reads as closed, because offering a resume with nothing to
  resume to would produce a blank session wearing an old session's name.
- `glasshouse launch` now records what it starts, moving the session through
  `Starting` -> `Running` -> `Stopped`/`Failed`, and `glasshouse sessions`
  reads it back. Creating the record is fatal if it fails; every later state
  change is best effort, because once a harness is running, Glasshouse's
  bookkeeping is not worth failing the user's session over.
- The schema has nowhere to put a provider credential, and
  `the_project_database_schema_has_nowhere_to_put_a_credential` pins the exact
  `(table, column)` list so any future addition fails until someone reviews it.
  An allowlist, not a name pattern: `project_metadata.key` would false-positive
  on any name match, and a credential column could just as easily be `value`.

Two defects were caught by running the thing rather than reading it:

- Every `Display` impl used `Formatter::write_str`, which **silently ignores
  width and alignment**, so the session listing's columns were ragged. Fixed
  with `Formatter::pad` and pinned by a test.
- `too_new_schema_is_rejected_and_not_recreated` set *every* migration row to
  99, which worked with one migration and violated the primary key with two.
  The fixture now appends a row, which is also what a newer build would
  actually leave behind.

One documented claim turned out to be **wrong and was corrected**: the unique
index's `WHERE native_session_id IS NOT NULL` clause was justified as
preventing collisions between sessions with no identifier yet. It does not —
SQLite already treats NULLs as distinct in a unique index. The mutation that
should have failed passed, which is how it was caught. The clause is kept for
index size and intent, and the comment now says so; the real hazard it guards
against is a future `NOT NULL DEFAULT ''` refactor, which is now its own
mutation check.

- `glasshouse launch [harness] [-- args]` is the first production consumer of
  `HarnessLaunch`. Until now the Phase 1 promise rested on a mechanism no
  shipped code exercised.
- `session::select` resolves exactly one harness and one executable, preferring
  a project-level configured path over a user-level one and an explicit path
  over PATH discovery. It refuses ambiguity rather than guessing, and a
  configured path that will not resolve is an error, never a silent fallback to
  a different binary.
- `session::attach` is a transparent bridge, not a renderer. That is what makes
  ConPTY's startup handshake work with no terminal emulation in Glasshouse: the
  cursor-position query reaches the user's real terminal, which answers it as
  it would for any program. Nothing in Glasshouse may answer it as well, or the
  harness receives the reply twice, as input.
- `shutdown::RawModeGuard` takes raw mode without the alternate screen, which
  is what routes Ctrl-C to the harness instead of to Glasshouse.
- The reported parallel PTY flake was diagnosed and is **not a Glasshouse
  defect**. Under stress (320 binary runs, ~6,400 test executions, 27 failing
  runs) every failure had one cause: `openpty` refusing to allocate at spawn
  time. The test named in the earlier report failed zero times. Probes pinned
  it to a macOS `openpty(3)` race under concurrent allocation — 64 live
  pseudo-terminals against a cap of 511 reproduced it, while the same churn
  from one process at ~8,000/s produced none — and it leaves `errno` at `-6`,
  which is not a valid errno. `pty::open_pty` now retries the allocation only,
  five times, side-effect free by construction.

- Discovery no longer gives up when an executable is absent. Both the cmux and
  Ollama capability lines are an OR, and only the left half had been built, so
  Glasshouse running *inside* cmux reported cmux as not found. Presence
  evidence — cmux's control environment, Ollama's configured endpoint — is now
  consulted in the not-found path only, reporting the integration as configured
  with no executable, so `is_usable()` stays false and nothing tries to launch
  it. Only variable *names* are ever recorded: a live `doctor` run with a
  credential in `OLLAMA_HOST` shows zero occurrences of it.
- A `.cmd` harness in a UNC project is refused before any process exists.
  `cmd.exe` would not have failed there — it substitutes the Windows directory
  and runs — so the session would have looked alive while operating outside the
  project entirely.

### What CI caught the moment it was allowed to run

Pushing for CI turned up **two production defects** that every local gate, two
independent reviews, and a green 24-test PTY suite had all missed:

1. **`cmd.exe` cannot open a verbatim `\\?\` path** (`4aa31ad`). Resolving an
   executable canonicalizes it, canonicalizing on Windows yields the verbatim
   form, and that went straight into `cmd.exe /D /C <script>`, which answered
   "The system cannot find the path specified" and exit 1. npm installs
   `claude`, `codex`, and friends as `.cmd` shims, so **no harness could have
   started on Windows at all.**
2. **A project-level executable override silently disabled the harness**
   (`e937dda`). `IntegrationConfig::enabled` was a plain `bool` with
   `#[serde(default)]`, so a project file overriding only a path parsed as
   `enabled = false` and beat a user-level `true`. The decision is now
   `Option<bool>`, making the tri-state per field rather than per entry.

Two process lessons worth keeping:

- **A green Windows tick is not proof the suite ran.** When the lib target
  fails, cargo never reaches `tests/pty_smoke.rs`, so the `.cmd` and
  verbatim-path claims silently did not execute while an earlier ledger
  revision implied they had. Confirm execution, not just the conclusion.
- **Make a platform-only failure explain itself on the first red.** Two CI
  round trips were spent guessing before the test was changed to print
  program, argv, requested cwd, canonical root, marker presence, exit status,
  and both streams. That one change identified the bug immediately.

### Review findings, and one the reviewer got half right

A read-only Ox reviewer worked the batch as a ten-item checklist and returned
ACCEPT WITH FINDINGS. Both findings were real and both are fixed:

- `SessionRecord::disposition` led with `lifecycle if lifecycle.is_live()`. A
  **guarded arm does not count towards exhaustiveness**, so the match needed a
  wildcard, and a new `SessionLifecycle` variant would have silently become
  `Active` — the opposite of what its "unreachable" comment claimed. Both it
  and `is_live` now enumerate every variant with no `_`, verified by adding a
  variant and watching three compile errors appear.
- `format_age`'s explicit `seconds < 0` branch returned the same string as the
  arm below it.

The reviewer's *reasoning* on the second was wrong: it said `saturating_sub`
clamps to zero, making the branch dead. It does not — `i64::saturating_sub`
saturates at `i64::MIN`, so the value really can be negative and the branch was
reachable, merely redundant. Right conclusion, wrong mechanism. Checking it
rather than accepting it also turned up an edge the report missed: a row
holding `i64::MIN` prints an absurd age, now pinned by a test that asserts the
honest contract (finite, never negative) instead of a prettier one that would
have required a magic clamp.

## Unresolved loose ends

- **`DELIBERATELY_UNTEMPLATED` is empty, and stays.** The 9D worker asked
  rather than deciding, which was right. The decision: **keep the mechanism.**
  An absence has to stay assertable, and the next credential someone holds for
  a service with no readable endpoint belongs there rather than in a guessed
  template. The worker added a control case so the now-vacuous loop still
  proves something; that is what makes an empty list honest rather than
  decorative.

- **z.ai's model list needs one authenticated request to settle.** Its
  unauthenticated `401` establishes nothing (see the ledger correction), so
  `model_list_endpoint` is `Unverified`. The user holds a key; the condition
  attached to it is free models only, and a `GET /models` costs no tokens. Do
  it the next time that key is being used anyway rather than spending a round
  trip on it alone.

- **A catalogue count is a snapshot, not a fact about a service.** UnoRouter
  answered `374` entries at 09:00 on 2026-08-26 and `369` an hour later. Every
  citation names a date for this reason, and nothing downstream may treat a
  count as stable.

- **The PTY test harness duplicates a character at every wrap boundary, and
  that is not the product.** `pty_smoke.rs` reads the raw pseudo-terminal
  stream and removes escape sequences (`strip_terminal_sequences`); it does not
  run a terminal emulator. When a line reaches the window width, ConPTY defers
  the wrap and **re-emits the last character** at the start of the next line,
  expecting a real terminal to overwrite it. Stripping the escapes and
  concatenating therefore duplicates one character per wrap. Observed on
  `windows-latest` against a runner `PATH` of several thousand characters:

      ...C:\hostedtoo | olcache\windows...  ->  "hostedtoo" + "olcache"
      ...bin;C:\Pro   | ogram Files\dotnet  ->  "Pro" + "ogram"

  **Glasshouse's own viewport does not have this problem** — it runs the stream
  through `vt100`, which honours those escapes. Only this test harness is
  naive. The rule it earns: **never assert on a long value reconstructed from
  `pty_smoke`'s output.** Assert on something short enough not to wrap, or run
  the stream through `vt100` first. Two Windows CI round-trips were spent
  before this was diagnosed, the first on a wrong hypothesis (plain wrapping)
  that whitespace normalisation "fixed" locally and did not fix on Windows.

- **The user's own gateway keys are available for Glasshouse, on one
  condition.** `~/projects/openrouter-clis` holds working credentials for
  seven gateways (OpenRouter, UnoRouter, AnyRouter, Z.ai, OpenCode Zen, Kilo
  and Nous — all seven with endpoints verified live on 2026-08-26). The user
  offered them for Glasshouse's own testing **provided only free models are
  used**. Full inventory — names, endpoints and env-var *names*, never a value
  — is in `docs/product/design-decisions.md`. Four map lines were added for what
  this exposed: naming the three missing services, key *pools* rather than
  duplicate provider instances, per-credential quota tracking, and the
  free-models-only rule for automated runs. Nothing is implemented yet; it
  lands with Phases 9C-9E and 9I.

- **`glasshouse hook` blocks forever if its stdin never reaches EOF.**
  `report_hook` drains stdin with `std::io::copy(stdin, sink)` — deliberately,
  so a harness is never left writing into a closed pipe — but that read is
  unbounded. Found by accident: running `cargo test` from a shell whose stdin
  was an open pipe hung `a_hook_that_cannot_report_still_exits_zero` and
  `an_installed_hook_moves_the_session_state` indefinitely, both parked in
  `wait4` on the child. Harnesses close the hook's stdin and both Claude Code
  and Codex additionally impose their own timeouts, so this is not known to
  bite in production — but it is an unbounded blocking read in a process the
  harness waits for. **Run the suite with `< /dev/null`**, and consider
  bounding the drain.

- **`codex` on `PATH` is a cmux wrapper script, not Codex.** In a cmux terminal
  — which is where this project is developed and run — `which codex` resolves
  to `…/cmux-cli-shims/<uuid>/codex`, a bash script that execs
  `cmux-codex-wrapper`. That wrapper injects `--enable hooks`,
  **`--dangerously-bypass-hook-trust`** and `-c hooks.X=…` into every
  entrypoint that starts a session (interactive, `exec`/`e`, `resume`, `fork`)
  so cmux's own session hooks run unprompted; other subcommands, including
  `--help`, pass through untouched.

  Observed directly: a session started with no such flag printed
  "`--dangerously-bypass-hook-trust` is enabled. Enabled hooks may run without
  review for this invocation."

  Consequences worth holding on to. `session::select` resolves that shim, so
  Glasshouse's Codex sessions already inherit cmux's hooks and its trust
  bypass. Glasshouse's own project-local hooks would be a *second* source
  alongside cmux's `-c hooks.X=` injections, and whether they compose is an
  assumption rather than a finding. Declarations read from `codex --help`
  remain sound, because the wrapper passes `--help` through — but they were
  read through a shim, which is worth knowing now rather than discovering later.

  **This is the Antigravity lesson in a new shape**: there the executable's
  *name* was wrong; here the name is right and the *identity* is not. Glasshouse
  should not silently prefer the real binary — the shim is what the user's
  environment provides, and stepping around it would break cmux's integration
  and the "operate the user's real installed harness" invariant. Making the
  resolved path visible, which `glasshouse doctor` already does, is the right
  response.

- **Codex's hook trust rides on its workspace-trust prompt.** Entering an
  untrusted directory, Codex asks whether to trust it and says in its own words
  that "Trusting … allows … hooks". So a Glasshouse session, being a real
  harness in a visible viewport, can simply let the user answer it — no
  user-level write needed. Whether that prompt alone enables a project-local
  `.codex/hooks.json`, or a per-file `[hooks.state…]` hash is also required, is
  **not yet established**.

- **No Codex hook has been observed firing yet.** With the workspace trusted
  and `<project>/.codex/hooks.json` present under both PascalCase and
  snake_case, a start-and-kill session fired nothing — consistent with
  `SessionStart` not firing in Claude Code either. Settling whether the file is
  read at all needs one real turn against a `user_prompt_submit` hook. Full
  evidence and the ordered open questions are in
  `.agent-runtime/notes-codex-hooks.md`.

- **The Codex adapter was citing the wrong artifact for its event names, and
  is fixed.** It declared ten `snake_case` events taken from
  `[hooks.state…]` trust keys — real keys, but the spelling Codex uses to
  *record trust*, not the spelling it reads from a hooks document. Codex
  0.149.1's own **hook review screen** enumerates **eleven PascalCase events
  with descriptions**, and that is now what the adapter declares:
  `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`,
  `PostCompact`, `SessionStart`, `SessionEnd`, `UserPromptSubmit`,
  `SubagentStart`, `SubagentStop`, `Stop`. `SessionEnd` had been missing
  entirely. This is the second time a declaration derived from the wrong
  artifact was wrong; the first was Antigravity's executable name.

- **Codex hook trust is its own prompt, separate from workspace trust.** On
  first seeing a project's `.codex/hooks.json` it says "Hooks need review — N
  hooks are new or changed. Hooks can run outside the sandbox after you trust
  them", offering `Review hooks` / `Trust all and continue` / `Continue without
  trusting (hooks won't run)`. So the project-local design works with **no
  user-level write at all**: the session is a real harness in a visible
  viewport and the user answers there. The `[hooks.state…]` hash entries are
  what that answer records.

- **Codex hooks are observed firing, and their payloads are captured.** With a
  project-local `.codex/hooks.json` trusted and one real turn taken,
  `SessionStart`, `UserPromptSubmit` and `Stop` all fired. Codex also printed
  `⚠ clamping SessionEnd hook timeout to 3s in <project>/.codex/hooks.json`,
  which names the file and proves it was read — and warns that **Codex clamps
  hook timeouts**, so a declared timeout may be silently shortened.
  Note `SessionStart` *does* fire for Codex, unlike Claude Code.

  Every payload carries `session_id`, `transcript_path`, `cwd`,
  `hook_event_name`, `model` and `permission_mode`; `UserPromptSubmit` adds
  `turn_id` and `prompt`; `Stop` adds `turn_id`, `stop_hook_active` and
  `last_assistant_message`. Full schema in
  `.agent-runtime/notes-codex-hooks.md`.

- **A hook is a better identifier source than a rollout scan, and Phase 8 line
  2 stays anyway.** `session_id` is in every payload, handed over directly with
  none of the originator/parent/cwd/time-window filtering that discovery needs —
  `transcript_path` even names the exact rollout. But hooks require
  installation *and* the user trusting them, while discovery needs nothing and
  works for a session that predates the hooks. Prefer the hook's `session_id`
  when one has reported; fall back to discovery otherwise.

- **The hook payloads carry conversation content.** `prompt` is the user's own
  words and `last_assistant_message` is the model's reply. A Glasshouse hook
  handler needs `session_id` and `hook_event_name` and must read neither of the
  others into a log, a diagnostic, a `Debug`, or the database. Make it a test,
  the way `nothing_is_read_past_the_first_line` already does for rollouts.

- **Never steer a user toward "Trust all and continue".** Doing so during this
  probe trusted five unrelated `warp@claude-code-warp` plugin hooks that
  happened to be pending review, writing them into the user's `config.toml`.
  Restored byte-identical from a backup. It is a blanket action over whatever
  else is pending; "Review hooks" is the honest path.

- **Windows CI caught a real production defect on the first push, again.**
  `read_first_line` required a trailing newline, so a rollout whose only line
  was its header — which is what a harness writes before it has anything to
  append — was discarded and the session reported no identifier. Linux and
  macOS passed; only Windows exercised a fixture written without the newline.
  Fixed, with `a_header_with_no_trailing_newline_is_still_read` writing the
  bytes directly rather than through the helper that appends one. **Every one
  of the eight original unit tests went through `write_rollout`, which appends
  `\n` — a shared fixture helper is a shared blind spot.**
- **No *live* Codex turn has had its identifier captured end to end.** The
  header format is proven against 555 real rollouts and the wiring against the
  shipped binary with a fake harness; what is unproven is only the join between
  the two on a real turn, which costs model usage. Worth doing once,
  deliberately, when a turn is being spent anyway.
- **A Codex session that takes no turn gets no identifier, forever.** That is
  correct — there is nothing to resume to — but it means a Codex session the
  user opened and closed without prompting reads as `closed`, not `resumable`,
  and the reason is invisible in `glasshouse sessions`.
- **Two Glasshouse Codex sessions started in the same project within the same
  window will both refuse to record an identifier**, because each sees the
  other's rollout as a second candidate. Fail-closed and honest, but a real
  usability edge if anyone runs parallel Codex sessions in one project. The fix
  is a narrower discriminator, not a ranking rule.

- The `fullscreenAutoDisabled` record this defect left in the user's
  `~/.claude.json` is **cleared**. `/tui fullscreen` was run in a real Claude
  Code session at the user's explicit request — they could not run it
  themselves, being on Remote Control, where `/tui` is unavailable — and Claude
  Code confirmed "Using flicker-free rendering". The fix and the repair are
  both verified; nothing edited the configuration file directly.
- **The terminal handshake is verified on macOS only.** The queries and replies
  are platform-independent and their tests run everywhere, but no real harness
  has been driven through the viewport on Windows.

- **Permission detection is the one hook line still open.**
  `PermissionRequest` is installed, translated, and proven to move the record
  when its command runs, but Claude Code firing *that* event has not been
  watched: the verifying turn needed no permission, and this machine runs
  Claude Code in auto mode, where a prompt that would ask is approved without
  asking. Closing it needs an isolated configuration with approvals required
  and a prompt that wants to run something.
- **Compaction is blocked by the harness.** Claude Code 2.1.245 exposes no
  compaction hook — the events a real installation accepts are the ten
  recorded in the adapter, none about compaction. Codex *does* expose
  `pre_compact`/`post_compact`, so Phase 8's equivalent is reachable and this
  one is not. Revisit when a release exposes one.
- **Hook firing is verified on macOS only.** The document and the reporting
  command are platform-independent and tested everywhere; Claude Code's own
  hook execution on Windows is not.
- **A new project directory makes Claude Code ask the user to trust the
  workspace.** An embedded session will show that prompt in the viewport,
  which is correct — native prompts stay interactive — but it means a session's
  first screen may be a question rather than a prompt box.

- **Anything configured as `claude-code` now receives `--session-id`.** Before
  this session Glasshouse passed no arguments at all, so any executable
  worked. A user pointing that integration at a wrapper script now needs the
  wrapper to pass its arguments through. This is correct — the flag belongs to
  the harness the user named — but it is a real change in blast radius.
- **A stopped session reads as resumable on the strength of an assigned
  identifier**, not on proof that a conversation exists. If a harness starts
  and dies before creating one, the harness refuses the identifier — Claude
  Code answers "No conversation found with session ID: …" and exits, which was
  observed directly. That is a clear failure rather than lost state or a blank
  session wearing an old name, and `Failed` sessions are never resumable; but
  it is optimism, and the resume command should surface the harness's own
  refusal rather than dressing it up.

- **Phase 6's communication-style line stays unchecked.** Six of seven
  adapters declare `Unverified` because their installed binaries document no
  such mechanism — Codex 0.149.0 in particular exposes no "personality",
  though the capability map names one as its example. `StyleChange::InPlace`
  therefore has no instance: Claude Code's output style is declared
  `NewSession` because the mechanism Glasshouse can drive, a settings document
  read once at startup, is fixed for the life of the process. Closing this
  needs one verified in-place mechanism, or a second harness with any verified
  native mechanism.
- **`resume`, `message` and `interrupt` have no production caller.** They are
  declared and unit-proven. Resuming belongs to Phase 7/8; messaging and
  interrupting to Phase 13/14. Line 3 asks an adapter to *expose* the resume
  command, which it does — executing one is a later line, and is not claimed.
- **No adapter parses harness output yet**, so the isolation guard for line 12
  currently protects a property nothing is pushing against. Installing it
  before Phase 7 rather than after is the point.
- **DeepSeek Harness waits for Phase 9A.** It is installed and its launcher
  interface is verified, but it ships no interactive terminal profile, and its
  own profile concept is Phase 9A's launch profiles under another name.
- **Pi is installed but not on `PATH`** on this machine (npm's global prefix
  is `~/.hermes/node`, which is not in `PATH`), so `glasshouse doctor` reports
  it as not found with `candidates tried: pi`. That is correct behaviour, and
  a good live example of why a configurable explicit executable path exists.
- The rustdoc baseline recorded in earlier revisions of this file as "15
  pre-existing diagnostics" was **wrong**: measured against `HEAD` in a clean
  worktree it is **23**. This session added none — it briefly added two
  ambiguous doc links (`crate::session::select` is both a function and a
  module) and both were fixed to `mod@` form before commit.

- **The shell's key bindings are plain single keys**, because no native session
  owns the keyboard yet. When one does (Phase 5) they must move behind a prefix
  or a mode, or they will steal keystrokes the harness needs.
  `ShellState::handle_key` is deliberately the only place that has to change.
- The shell reads sessions once at startup and on an explicit redraw event.
  Nothing yet raises that event, so a session started elsewhere while the shell
  is open does not appear until it is reopened. `AppEvent::Redraw` and
  `ShellState::refresh` are the seam, and `refresh` already reconciles by
  identifier rather than index.
- The viewport is reserved and empty. Phase 5 fills it.
- **Open question on Windows: does a bare carriage return satisfy a real
  harness?** `encode` sends `\r` for Enter, which is what a terminal sends. The
  Windows *fake* harness reads with `set /p`, which wants CRLF, so the shell's
  end-to-end round-trip test is Unix-only. Making `encode` emit CRLF would be
  wrong — every Unix harness would get a spurious extra newline per keystroke —
  and the harnesses Glasshouse actually targets read raw input and accept CR.
  But that is reasoning, not evidence; confirm it against a real harness on a
  real Windows install. The forwarding path itself is covered on Windows by
  `keystrokes_reach_the_focused_session` at the runtime layer, and the shell's
  mode machinery by `the_shell_enters_and_leaves_session_mode_in_a_real_terminal`.
- `session::runtime` (`SessionRuntime`) exists and is proven against real
  processes on all three platforms, but **has no production caller yet**, so
  seven Phase 4 boxes stay unchecked. `docs/product/design-decisions.md`
  records the decision that unblocks it: the shell's single-key bindings cannot
  coexist with forwarding every keystroke to a harness, so control mode and
  session mode split, with `Ctrl-]` as a single-chord escape.
- `SessionRuntime::is_running()` reports the status cached by the last
  `poll_exits`, not a fresh answer from the operating system. That is honest —
  it is documented as observation-based — but it caught a test out: a mutation
  killing every session on `close` stayed green because the survivor had not
  been polled since. Any test asserting liveness must poll first.
- Exit detection cannot currently be proven independent of output *on macOS*.
  The discriminating case needs a process that exits while its output stream
  stays open, and a direct probe showed macOS reports end-of-file on the
  pseudo-terminal master as soon as the foreground child exits, even with a
  background child still holding the slave. The capability's real risk — a
  silent-but-running harness mistaken for a finished one — is covered.

- **Nothing calls `open_for_resume` in production.** The cross-project resume
  guard is implemented, structurally enforced, and mutation-proven, but there
  is no `glasshouse resume`, so Phase 1 line 90 is `PARTIALLY VERIFIED` and its
  box stays unchecked. The adapter it was waiting for now exists — every one of
  the seven exposes a verified resume invocation — but a `glasshouse resume`
  built today could still only report "not resumable", because no adapter
  *captures* a native identifier yet. That is Phase 7/8, and the earlier
  instruction stands: do not close line 90 with a command that can only say no.
- No harness adapter captures a native session identifier yet, so in production
  `sessions.native_session_id` is always `NULL` and no session ever reaches the
  `Resumable` disposition. The mechanism is complete; what feeds it is Phase
  7/8.
- Only `Embedded` presentation occurs in production, because `glasshouse
  launch` is the only session producer. `Headless` and `External` arrive with
  Phase 4 and Phase 17.
- `glasshouse sessions` has no filtering, no sorting options, and no way to
  remove a record. Phase 11 owns the real overview; this is the minimum that
  makes the stored metadata observable.

- The forced-exit orphan is **fixed**: an attached session registers a cleanup
  that `shutdown`'s force path runs before `process::exit`. It is best effort
  by construction (`try_lock`, never `lock`) because a cleanup that waits could
  hang the one escape hatch whose purpose is to always work. If the lock is
  held at that instant the harness is still orphaned — no worse than before,
  and the alternative is a Glasshouse that will not die.
- `session::attach` owns the process's terminal for its whole life: its stdin
  pump cannot be cancelled, so the process exits out from under it. The
  multi-session TUI will need a different input path.
- Native Windows UNC project roots remain refused; `cmd.exe` cannot reliably
  hold a UNC working directory.
- Antigravity detection is **resolved**: a real Antigravity CLI 1.1.20 was
  installed and `glasshouse doctor` reports it. The executable name was wrong —
  the published package links its binary onto `PATH` as `agy`, not
  `antigravity` — so nothing would ever have detected it. Both names are
  searched now. cmux control-environment and Ollama configured-endpoint
  detection remain implemented and checked.
- The UNC refusal's *premise* — that `cmd.exe` substitutes the Windows
  directory rather than failing — is documented Windows behaviour, not
  something a live run confirmed. No real UNC share was exercised; the refusal
  itself is platform-independent and runs in CI everywhere.
- `IntegrationId::minimum_version()` returns `None` for every integration, so
  unsupported-version classification exists but is unreachable. Declaring a
  real minimum needs verified release data this environment does not have.
- The main session TUI, session metadata schema, harness adapters, durable
  memory table, and session persistence are not implemented.
- Strict rustdoc still fails on 15 pre-existing lib-doc diagnostics, 9 of them
  public docs linking to private items. The count in an earlier revision of
  this file said 12 and was simply wrong; this session added none, verified by
  measuring the baseline with the branch stashed.
- The cross-harness completion protocol remains design documentation. This
  session used its durable-file half — each worker wrote
  `.agent-runtime/report-<TASK-ID>.md` — with manual visible pane polling and
  no automatic wake, exactly as the protocol prescribes until its safety tests
  exist.
## Where to go next

**Every batch that was blocked on file ownership has landed.** Phase 9D is
closed, so the three lines that were waiting on an HTTP client are done and no
worker is in flight. 1,015 mandatory lines remain unchecked, and the map's own
structure says they partition: whole blocks sit in modules nothing else
touches.

Three batches are ready, partitioned by the files they touch (practice §9):

1. **Phase 4's last three lines — `send_text`, `interrupt`, headless
   presentation.** This is the oldest unchecked mandatory work in map order
   after the blocked lines, and it is **red risk**: PTY lifecycle, signals and
   job control are explicitly the Opus specialist's, never Sonnet's. It owns
   `session/runtime.rs`, `session/mod.rs` and all of `shell/`. Note the
   recorded trap: `SessionRuntime::is_running()` reports the status cached by
   the last `poll_exits`, so **any test asserting liveness must poll first** —
   a mutation killing every session on `close` once stayed green because the
   survivor had not been polled.

2. **Phase 9A's 359/360/363 and Phase 9F's 465/466, plus the deferred
   `gateway`-into-`Resolution` fold.** One coherent seam about generated
   configuration and pre-launch verification. Owns `profile/mod.rs`,
   `harness/`, and `config/mod.rs` (which the fold needs for two test
   literals). Both files are free for the first time.

3. **Phase 2D's Routing settings section (lines 181-186).** `RoutingConfig`
   already exists — Phase 2C's routing-model step built it — so this is the
   settings surface over a model that is already there. It owns
   `shell/state.rs` and `shell/view.rs`, **so it cannot run beside batch 1**.
   Run it after batch 1 lands, or give batch 1 only `session/` and accept a
   thinner slice. Line 187 (Memory section) stays blocked on Phase 20.

Still blocked, unchanged:

- **Phase 8 line 9 (Codex compaction)** — needs Phase 30's compaction counter.
- **Phase 7 lines 305/307** — permission detection needs an isolated
  configuration with approvals required; compaction is not exposed by Claude
  Code 2.1.245 at all.
- **Phase 9 lines 337/338 (Antigravity lifecycle events)** — the CLI exposes
  none.
- **Phase 9E lines 438/439** — Windows Credential Manager and a Linux Secret
  Service keyring are not provable from this machine.
- **Phase 6's communication-style line** — needs one verified in-place
  mechanism.
- Phase 1 line 107, Phase 3 line 231, Phase 2D line 187 — all Phase 20's memory
  table.

**And one thing that needs the user's environment rather than a worker:**
proving Phase 9F end to end against the real OpenRouter gateway. Everything
needed exists — Anthropic Messages at `https://openrouter.ai/api`, the root,
**free models only**, which is the condition attached to those keys. It is the
named evidence gap in 9F's ledger entry.

## Active worker tasks and results

**One worker, GH-P09D-CONNECTIVITY**, an **Opus team lead** with three
`agy-gh` leaf subcontractors, in its own worktree
(`claude/9d-connectivity`). Roughly 2 h 50 min; +5343/-176 across 12 files
after integration.

What it got right, and what the orchestrator still had to do:

- It kept every red-risk part itself, as the packet required: the timeout and
  responsiveness design, everything touching the credential, the cache's
  on-disk format, and **all thirteen mutations**.
- **It verified its leaf workers mechanically** rather than reading their
  summaries — 339 quoted `path:line` pairs checked against the source, 339
  exact, 0 mismatched.
- **A mutation changed its code, which is the point of mutations.** Mutation 1
  did not fail — it *hung*, because a probe with no read timeout never returns.
  A test that can only fail by hanging reports nothing, so the test was
  rewritten to run the probe on a thread and wait with `recv_timeout`.
- **It corrected its packet on five points and was right on all five**,
  including that the packet's "the verified constructor exists and is used only
  in tests" had no literal referent. That is five consecutive batches in which
  a worker was right against its brief.
- **The orchestrator withdrew one of its six evidence promotions.** Re-running
  all six probes took under a minute; five reproduced, z.ai did not. See the
  correction entry in the ledger and practice §23.
- The orchestrator re-ran every gate on the integrated tree, ran three
  independent mutations, drove the shipped binary against a live provider, a
  refused host and a real never-answering listener, checked the cache file's
  raw bytes for the planted credential, wrote the records and made the commit.
  The worker committed nothing and touched no project record.

## Commands run and outcome

All run by the orchestrator on the integrated tree, not taken from the
worker's report:

- `cargo fmt --all -- --check` — pass.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` —
  zero diagnostics.
- `cargo test --workspace --all-features < /dev/null` — **865 passing, 0
  failing** (780 lib + 8 bin + 11 provider_discovery + 62 PTY + 4 settings),
  against a **779** baseline measured on `main` at the start of the session.
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` — clean. The
  baseline is zero and stayed zero.
- `scripts/msrv-check.sh` — pass, resolving rustc from the 1.88 toolchain.
- `git diff --check` — pass.
- `python3 scripts/progress.py` — 252 / 1267 mandatory, 19%.
- Three orchestrator mutations, three kills, each verdict read from the named
  test's own result line: `ProbeRequest`'s `Debug` printing the credential; the
  caller *joining* the probe thread so the request blocks the drawing thread;
  and re-promoting z.ai, which killed at two independent layers.
- **Six live endpoint re-probes** reproducing the batch's evidence
  independently — five confirmed, one withdrawn — plus a five-request control
  run against z.ai that is what settled it.
- **The shipped binary, in a real terminal**: a live 417-model refresh from
  OpenRouter cached with a timestamp; a refused host; a real listener that
  accepts and never answers, bounded at `10004ms` with the cursor still moving
  through three `Down` presses; the cache file's raw bytes checked for the
  planted credential (zero occurrences); and a restart that re-fetched nothing,
  with `fetched_at` and mtime both unchanged.

## Next exact step

Hand this checkpoint to Opus:

> Start with `git status`, `git log -5`, this handoff, and
> `.agent-runtime/CONTINUATION.md` — whose Part 1 is generic standing rules,
> including re-arming the context and usage-window watches, which do not
> survive a session. **Verify the statusline file is fresh before trusting
> either watch.** Pushing to run CI is standing authorization.
>
> No worker is in flight. **"Where to go next" names three batches already
> partitioned by the files they touch** — start two of them concurrently and
> keep the third until the first lands, because batches 1 and 3 both own
> `shell/`.
>
> The habits that earned this session's results:
>
> - **Re-run a worker's decisive external observations yourself** (practice
>   §23). Six `curl`s took under a minute and caught an unfounded `Verified`
>   declaration that was otherwise about to ship.
> - **A control has to be run against the host it justifies.** A control
>   borrowed from another service is a statement about that service.
> - **Run the binary.** It is still the most productive check in this process,
>   and this session it confirmed both the timeout value and the
>   responsiveness guarantee in a way no test could have on its own.
> - **Read the named test's own result line**, in the target that runs it.
> - **Never `git checkout` in a worktree holding uncommitted work.** A
>   `PreToolUse` hook blocks it; if it fires, it is right.
