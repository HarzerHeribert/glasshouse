# Glasshouse orchestration measurements

> This describes how Glasshouse is built, not what Glasshouse does. Nothing
> here is a product requirement. Capability requirements live only in
> `docs/product/capability-map.md`.

Glasshouse's own product principle says telemetry must measure **outcomes and
evidence, not token/spend vanity metrics**. This file applies that principle to
the process that builds Glasshouse, because the same question the product asks
about routing work to models is the one this project answers every hour by
hand: *which tier, at what cost, produced what verified result?*

**This is a standing experiment, and it is inherited.** Every orchestrator adds
its batches to the ledger below and writes down what changed its mind. Do not
start a fresh measurement culture; continue this one. An entry with no verdict
is worse than no entry.

## What counts as an outcome here

Not lines of code, not tokens spent. A batch's outcome is:

- **boxes** — authoritative capability-map checkboxes closed with `COMPLETE`
  evidence. The only unit that means anything.
- **kills** — mutations run and killed. A box with vacuous tests is not a box.
- **corrections** — times the worker was right against its packet. This is a
  quality signal about the *tier*, and it has been consistently high.
- **rework** — gates that failed under the orchestrator's own re-run, or
  findings that had to be sent back.

Cost per box is the headline ratio. Wall-clock per box matters only where it
blocks the next batch.

## Method

- Worker cost, context and wall-clock are read off the harness's own status
  line in its cmux pane at the moment it reports.
- Orchestrator context is read from `CTX_PCT` in the statusline data file.
- Boxes are counted from the map before and after integration.
- Mutation verdicts come from the named test's own result line, in the target
  that runs it.
- **Leaf output is verified mechanically**, never eyeballed — diff its quotes
  against the source.

## The ledger

| Batch | Tier | Wall-clock | Worker cost | Output | Boxes | Kills | Corrections | Verdict |
|---|---|---|---|---|---|---|---|---|
| 9F direct provider | Opus specialist | ~39 min | ~$10.90 | +1757/-78, 6 files | 11 | 16/16 | 3 | PASS, CI green first push |
| 9D templates+headers | Sonnet | ~31 min | ~$11.00 | +775/-21, 9 files | 5 | 13/13 | 2 | PASS, CI green first push |
| 9G gateway skeleton | Opus **team lead**, 3 subcontractors | ~22 min | ~$15.30 | +663, 6 files (new module) | 7 | 10/10 | 4 | PASS |
| 2C onboarding | Sonnet | ~50 min | ~$9 | +1471/-24, 5 files | 6 | 3 (2 weak, rewritten) | 3 | PASS, CI green first push |
| 9 Antigravity id | Opus **team lead**, 1 subcontractor | ~25 min | ~$13 (last read $10.83 at 16 min) | +1258/-75, 7 files | 2 | killed, 2 re-run by orchestrator | 7 | PASS |
| 9G Anthropic ingress | Opus **team lead**, subcontractors | ~65 min | ~$22 | +4426/-193, 11 files | 10 | 24 run, 23 caught + 1 survivor that found a real gap | 6 | PASS, CI green first push |
| 2D settings sections | Sonnet | ~55 min | ~$12 | +2675/-77, 5 files | 4 | 1 orchestrator mutation found a **weak test** | 1 | PASS |
| 9E native secret store | Opus specialist | ~35 min | ~$14 | +2445/-46, 11 files | 3 | 1 orchestrator mutation, killed by 2 tests | 3 | PASS |
| Records audit | Gemini 3.7 Flash via `agy` | **blocked** | — | — | 0 (read-only) | — | — | BLOCKED on its permission model — see below |
| Records audit (redone) | orchestrator, one script | ~1 min | negligible | 1 script | 0 (read-only) | — | — | PASS — zero real drift found |
| Dev shims | orchestrator solo | ~35 min | — | +2 shims, 3 docs | **0** | n/a (both guards proven both directions) | 1 (mine: conflated dev shim with product shim) | PASS, CI green |
| MSRV correction | orchestrator solo | ~50 min | — | +1 script, 3 code sites, 4 docs | **0** | 1 gate mutation, killed | 0 | PASS, 6/6 CI jobs green |
| 9G ingress ×2 | Opus **team lead**, 2 subcontractors | ~50 min | ~$29 | +1887/-182, 6 files | 2 (phase COMPLETE) | 8 by lead + 1 by orchestrator, all killed | 1 (mine, load bearing) | PASS, CI green |
| 2C routing model | Opus **team lead**, subcontractors | ~55 min | — | +2450/-51, 4 files | 4 (phase COMPLETE) | 17 designed, 17 killed, 0 survived | 4 | PASS, CI green |
| Rustdoc links | Sonnet | ~25 min | — | +22/-22, 12 files | **0** (made a gate real) | 1 gate mutation, killed | 0 | PASS |
| 9B child env | **Codex `gpt-5.6-sol` xhigh** | ~17 min | subscription, 4% of weekly | +169/-95, 2 files | 1 (phase COMPLETE) | 3 by worker + 1 by orchestrator, all killed | 4 | PASS |
| 9D connectivity + model cache | Opus **team lead**, 3 leaf subs (`agy-gh`) | ~2 h 50 min | — | +5343/-176, 12 files | 3 (phase COMPLETE) | 13 by lead + 3 by orchestrator, all killed | 5 | PASS — **but one of six evidence promotions was withdrawn on review** |

### The batch that says review is not a formality

The 9D batch is the strongest single deliverable this process has produced —
thirteen mutations all killed, three leaf subcontractors all verified
mechanically, two real defects found by running the binary, and a report that
volunteered five things its own packet got wrong. It also contained **one
unfounded evidence promotion that would have shipped as a `Verified`
declaration in the product.**

Both facts are about the same batch, and the second does not diminish the
first. The lead promoted six `model_list_endpoint` declarations from live
probes. The orchestrator re-ran all six independently: five reproduced exactly.
The sixth, z.ai, had answered `401` rather than `200`, and the lead promoted it
on a stated control — *"a host that served nothing there would have answered
404"* — that it had **cited from a probe against a different service**. Run
against z.ai, the control fails: every path under that prefix answers `401`,
including invented ones, and a nonexistent API version answers `200`.

Three things worth carrying:

- **Re-running a worker's decisive external observations is cheap and it
  paid.** Six `curl`s, under a minute. The five that reproduced cost nothing to
  confirm; the one that did not was about to become a product claim.
- **The error was in the reasoning, not the diligence.** The lead ran a real
  probe, read a real body, wrote down what it saw, and explained itself well
  enough that the flaw was *visible in its own doc comment*. A less careful
  worker would have left nothing to catch.
- **It is the fifth declaration in this project derived from an artifact that
  did not support the use it was cited for** — after Antigravity's executable
  name, Codex's snake_case hook events, Claude Code's `auto-mode` subcommand,
  and Cursor's sandbox usage strings. The pattern is now unmistakable enough to
  be a standing review step rather than a lesson: **before accepting a
  declaration, check that its evidence was gathered against the thing it is
  being used to justify.**

**Answering open question 3 — the leaf tier's accuracy, on a second and third
task.** The lead ran three `agy-gh` leaf workers, each in its own worktree with
one explicit file. The inventory leaf returned **339 quoted `path:line` pairs,
and the lead verified every one mechanically: 339 exact, 0 mismatched, 0
missing-file.** The fixture leaf found and fixed a rustdoc trap the lead's own
spec had introduced. The test leaf caught that the snapshot it was given was
unformatted. So the leaf tier now has 171/171 and 339/339 on bounded quoting
tasks, from two different orchestrating sessions. **Treat that as established
for inventory work, and keep verifying mechanically — its value is that it is
checkable, not that it is trusted.**

The lead's own read on delegation is worth preserving: the tests packet was the
weakest of the three, because a subcontractor can only reach the public API, so
the load-bearing tests stayed with the lead regardless. **Delegate breadth;
keep the tests that need private access.**

### What the first data point already says

**N = 1 for Opus, so treat these as orders of magnitude, not constants.**

- **~$1 of worker spend per capability box**, at Opus tier, including its own
  mutation testing. That is cheap against the alternative of the orchestrator
  writing it.
- **~17 boxes per worker-hour** on a well-specified packet.
- **Orchestrator review and integration cost ~10% of context and ~12 minutes**
  against the worker's 39. The review is *not* the bottleneck; worker
  wall-clock is.
- **Sonnet runs at roughly a third of Opus's cost rate** for comparable
  packets. The 9F batch was routed to Opus because it crossed a secret
  boundary, not because Sonnet could not have written the code.

### Three batches in, and the tiers separate

| | 9F (Opus solo) | 9D (Sonnet solo) | 9G (Opus + 3 subs) |
|---|---|---|---|
| boxes | 11 | 5 | 7 |
| worker cost | ~$10.90 | ~$11.00 | ~$15.30 |
| **cost per box** | **~$0.99** | **~$2.20** | **~$2.19** |
| wall-clock | 39 min | 31 min | 22 min |
| **boxes per hour** | **17** | 10 | **19** |
| packet corrections | 3 | 2 | 4 |

Read carefully, because the headline is misleading. **Cost per box is dominated
by how many boxes a packet's lines happen to be worth, not by tier.** 9F's
eleven lines were a single coherent seam; 9D's five each needed their own
evidence. Comparing Sonnet to Opus on this table is not valid — the packets
were not comparable. What *is* comparable: **Sonnet produced 775 lines with 13
mutations and 2 correct packet corrections, and its CI went green first push.**
Nothing in the 9D batch needed Opus, and it was routed to Sonnet on exactly
that judgement.

**The team lead is the fastest thing measured so far** — 19 boxes/hour against
17 for a solo Opus, on red-risk work, with the most packet corrections of any
batch. It cost ~40% more per box than the solo Opus batch, and bought:

- **coverage the lead did not have.** Two of its ten mutations survive the
  lead's own tests entirely and die only to a subcontractor's test. That is the
  single most useful number in this file.
- a **45-in-100 flake found in the lead's own test** by a subcontractor. The
  lead had convinced itself the test was correct.
- a vacuity check run against **all four** forbidden import paths rather than
  the one asked for, plus a false-positive nobody had considered
  (`crate::shell` being a substring of `crate::shutdown`).

**The cost was real:** subcontractors each copied a 1.1 GB worktree, and one
snapshotted the lead's tree *mid-mutation*, capturing a deliberately broken
intermediate and having to redo everything. **Snapshot before mutations begin,
or have subcontractors work from a git ref rather than the working tree.**

Verdict so far: **use a team lead for red-risk work that decomposes.** Its
review cost is paid from its own context, its subcontractors find what it
cannot, and it is not slower.

### The parallelism ceiling, derived rather than guessed

Reviews are serial — the practice file's "never review two workers at once"
rule stands, and it is about attention, not throughput. Workers run
concurrently; reviews queue. So:

- **worker wall-clock ≫ review time** means concurrency pays until reviews
  start colliding — about **three editing workers**;
- the real ceiling is **orchestrator context**, at roughly 10% per batch, which
  puts a session's budget at **six to eight integrated batches** before handoff;
- a **team lead that reviews its own subcontractors** raises the first ceiling
  without touching the second, because its review cost is paid out of *its*
  context, not the orchestrator's. That is the whole reason to use one.

### A tier's cost includes the cost of driving it

The leaf tier is cheap per token and **not** free to operate. Measured
2026-08-25:

- Antigravity declares no automatic-review mode, and its "always allow" matches
  the **exact command prefix including the whole script body**. So a leaf doing
  real work re-prompts on every distinct command, and the "always allow" option
  buys nothing. It is unusable unattended without
  `--dangerously-skip-permissions`.
- **Claude Code's own auto-mode classifier refused to type that flag into a
  pane**, repeatably. The orchestrator did not route around it — that is the
  one thing such a denial exists to prevent — so the leaf worker was parked and
  the task was done another way.
- **The task itself took the orchestrator about a minute as a single script.**
  Which is the lesson worth keeping: a purely mechanical counting task is often
  cheaper to *do* than to *delegate*, and delegation earns its keep on breadth
  (many files, many quotes) rather than on arithmetic.

So the open question "how accurate is the leaf tier" is still open, and a new
one joins it: **what does it cost to drive each tier, in orchestrator attention
and in permission friction?** A tier that needs a human to approve a flag is
not a tier you can fan out to at 2 a.m.

### The most valuable thing a subcontractor found was not in its brief

The Antigravity batch's subcontractor was asked for three end-to-end tests. It
also **refused to reuse a literal conversation identifier** it found in an
existing fixture, and said why — which is how a real identifier of the user's,
already committed to git history, was discovered at all. Neither the lead nor
the orchestrator had noticed it across several batches touching that file.

Two batches running, two subcontractor finds outside their briefs (the other
was a 45-in-100 flake in the lead's own test). That is now the strongest
argument in this file for the team-lead pattern: **the value is not the extra
hands, it is the extra pair of eyes that has not already convinced itself.**

Set against it, an honest cost from the same batch: the lead's subpacket
initially invited the subcontractor to mutate the same `src/` files the lead was
mutating. It caught this and cancelled before anything started, but the failure
mode is real and now written into the practice file.

### The acknowledged bypass is human-only, and two independent designs agree

2026-08-26. The user explicitly authorized recording Antigravity's
blanket-bypass acknowledgement so the leaf tier could run unattended. **The
orchestrator could not do it, by three different routes** — typing the harness's
bypass flag into a pane, launching the harness with it, and writing the
acknowledgement key into the user config. Each was refused by Claude Code's
auto-mode classifier, and none was routed around.

That is a *result*, not an obstacle, because two safety designs that know
nothing about each other reached the same conclusion:

- **Phase 9A** permits a blanket bypass only "after the user has been shown its
  risk once and acknowledged it" — a human act, recorded per harness, user
  layer only.
- **The harness's classifier** independently refuses to let an agent enable a
  bypass on its own behalf.

So the acknowledgement genuinely requires a keyboard, which is what the
capability line intends. **Do not treat this as friction to engineer away.** The
correct sequence is one human step — `glasshouse setup`, tick the harness — after
which `glasshouse shim <harness> --profile <p>` produces a user-owned entry
point with the decision recorded behind it.

The measurable consequence for tier selection: **the leaf tier cannot be
bootstrapped autonomously.** An overnight run cannot add it; a human must arm it
once, in advance.

### The mistake this measurement exposed

For most of 2026-08-25 the orchestrator ran **one worker at a time**, believing
the work could not be partitioned. That was wrong, and the map itself proves
it: **1,266 unchecked lines across 99 phases**, with whole blocks in modules
nothing else touches. The conflicts were real only *within* the Phase 9 family,
because work was being taken in strict map order inside one family.

**Map order is a priority, not a mutex.** Partition batches by the *files they
touch*, then order those batches by the map. A packet's `FORBIDDEN FILES`
section is the scheduling primitive: it is what makes two workers safe to run
at once, and it should name the other live workers' files explicitly.

### Ten batches in: what actually caught the defects

Tally across the whole session, because it settles an argument this file opened:

| how a real defect was found | count |
|---|---|
| **running the shipped binary** | **6** |
| a mutation the orchestrator ran during review | 3 |
| a subcontractor working outside its brief | 2 |
| Windows CI | 2 (both test defects, not product) |
| a worker reading its own packet critically | 4 packet errors |
| **a CI job on its first run** | **1 — a false MSRV, wrong since ratatui 0.30** |

**Running the binary is the single most productive check in this process**, by a
clear margin, and nothing else is close. It found the Keychain hang that would
have frozen the TUI, the Nagle stall on every streamed event, a stale banner
that made a wizard silently un-drivable, a refusal message rendered off-screen,
`cmux` accepted as a launch harness, and two doubled-backtick renderings in an
earlier session. Every one of those compiled, passed clippy, and passed a full
suite.

**Mutation review is second, and its value is asymmetric.** Two of the three
mutations that mattered *survived*: one exposed a real gap in the product, the
other exposed a test passing for the wrong reason. A mutation that dies confirms
what you already believed; a mutation that lives teaches you something. Budget
review time for the survivors.

## Questions the next orchestrator should answer

Add your data; do not re-derive from scratch.

1. **Does the Sonnet tier close boxes at Opus's rate on amber work?** If cost
   per box is comparable, red-risk routing is the only reason to spend Opus.
2. **Does a team lead with subcontractors beat a lone Opus worker** on the same
   packet size — in wall-clock, in cost, and in whether its mutations still get
   done properly? Delegated test-writing is the obvious win; delegated
   *judgement* is the obvious risk.
3. **How accurate is the leaf tier on a second task?** It scored 171/171 on a
   map inventory. One score is not a capability.
4. **What is the real failure rate of concurrent worktrees?** Count merge
   conflicts and reverts, not intuitions about them.
5. **Where does an orchestrator's context actually go?** If reading diffs
   dominates, a verifier tier between worker and orchestrator pays for itself.
6. **~~How many of this project's gates are decoration?~~ ANSWERED
   2026-08-26: two of six.** Every gate was mutated deliberately and the result
   recorded. Total cost, about ten minutes.

   | gate | mutation | verdict |
   |---|---|---|
   | `cargo fmt --all -- --check` | added badly-formatted fn | **bites** — exit 1, exit 0 restored |
   | `cargo clippy … -D warnings` | none needed — failed for real on `collapsible_if` when the MSRV rose | **bites** |
   | `cargo test --workspace` | proven continuously by this project's own mutation discipline | **bites** |
   | `python3 scripts/progress.py --check` | changed a count in the README block | **bites** — exit 1, diff printed, exit 0 restored |
   | `RUSTDOCFLAGS='-D warnings' cargo doc` | none needed | **DECORATION** — 22 warnings, never once green |
   | `rustup run 1.85.0 cargo check --locked` | raised `rust-version` to 1.99 | **DECORATION** — could not fail, for two independent reasons |

   The two that were decoration were both *inherited and trusted*, and neither
   had ever been questioned. The four that bite were never in doubt. **The
   correlation worth noticing: a gate nobody has ever seen fail is the one to
   suspect.** `fmt`, `clippy` and `test` fail routinely, so they were obviously
   alive; the MSRV and rustdoc gates were "always green", which read as health
   and was actually silence.

   Do this to any gate you inherit, and to every gate you write.

## Zero-box work is not zero-value work — record it anyway

Two batches today closed **no capability boxes** and were among the most
valuable of the project. This ledger measures boxes per hour, so it structurally
undervalues them, and a future orchestrator reading only the table would
conclude they were waste.

- **Dev shims** removed a per-invocation tax that every session had been paying:
  `cargo run --manifest-path …` instead of `glasshouse`, and a round trip to the
  user every time a leaf worker needed launching. The cost was being paid
  forever and counted nowhere.
- **The MSRV correction** found that the gate the whole project trusted was
  incapable of failing. Every "MSRV clean" claim in the evidence ledger before
  `aef4285` was unfounded — not wrong about the code, but unfounded as
  evidence, which for this project is the same thing.

**The pattern: infrastructure work shows up as a flat line on a boxes-per-hour
chart and as a slope change everywhere else.** When you spend a session on
something that closes no boxes, write down what recurring cost it removed, so
the next orchestrator can tell the difference between that and drift.

## The Codex tier — first data, and what it costs to run

Added 2026-08-26 at the repository owner's request, alongside the existing
Claude Code and Antigravity tiers. Model identifiers on a ChatGPT subscription
are `gpt-5.6-sol` (frontier), `gpt-5.6-terra` (mid) and `gpt-5.6-luna` (fast) —
the bare names are rejected. Run at `xhigh` to match the Claude Code workers.

**First batch: one map line, seventeen minutes, and it found a real defect.**
The line looked like a formality — "preserve the user's existing shell
environment" — and the packet explicitly allowed "already correct, here is the
regression test" as an outcome. It was not already correct: `portable-pty`
0.9.0 merges Windows registry values over the environment it was handed,
replacing `PATH`, and **a pre-existing test had responded by compiling its own
assertion out on Windows.** The worker fixed the cause and re-enabled it.

**Do not read boxes-per-hour across differently-sized batches.** One line in
seventeen minutes is ~3.5 boxes/hour against a team lead's 19, and that
comparison is meaningless: the lead's batch had ten boxes of related work to
amortise its setup across, and this one spent most of its time auditing spawn
paths to answer a yes/no question. What the number does say is that a
single-line packet carries most of a multi-line packet's fixed cost, so **do not
send single lines unless the line is the point.**

### Three operational facts, all learned the hard way

1. **Codex needs no bypass shim.** `-s workspace-write -a never` is a real
   automatic-review mode, unlike Antigravity's blanket bypass. This is the same
   distinction Glasshouse's own adapters record, and it makes Codex the cheaper
   tier to run safely.
2. **Its sandbox denies loopback bind and Keychain**, so ~27 gateway tests and
   3 macOS secret tests fail on infrastructure alone. **The orchestrator must
   run the full suite for every Codex batch.** Say so in the packet, or a
   conscientious worker will burn time trying to make them pass — and a less
   conscientious one will "fix" them.
3. **Its sandbox denies writes outside the worktree.** Put the report path
   *inside* the worktree. The first batch had its report write refused, wrote to
   `/tmp`, and said so clearly — good behaviour recovering from a bad packet.

### What it did with a bad packet

Four packet corrections, and the important one was substantive: the packet
asserted the PTY builder "inherits the parent environment by default", which is
true of `std::process::Command` and **not** true of `portable-pty` on Windows.
A worker that had taken the packet's word would have written a passing test and
closed the box over a live defect.

That is now four consecutive batches where the worker corrected the
orchestrator's brief, across three different harnesses. **Packets are wrong
often enough that "tell me what this packet got wrong" belongs in every one** —
it is the cheapest review step available and it has never once come back empty.

---

## Batch 13–14 — two concurrent workers, both integrated 2026-08-26 midday

| | GH-P04-UNFOCUSED | GH-P09F-PREFLIGHT |
|---|---|---|
| tier | Claude Code, Sonnet implementer | Claude Code, Sonnet implementer |
| boxes closed | **2 of 3** | **0 of 2** |
| diffstat | +2056 / −45, 6 files | 2 files |
| tests added | +29 (867 → 896) | +18 |
| mutations | 11 run, 9 killed | see report |
| packet corrections | 1 (`cli.rs`/`main.rs` missing from EXPECTED FILES) | 2, both substantive |
| defects found by running the binary | **2** | 0 |
| verdict | **excellent** | **excellent, and the zero is the point** |

**Both workers corrected the orchestrator's packet. That is now six consecutive
batches.** "Tell me what this packet got wrong" has never come back empty and
remains the cheapest review step in this process.

### The measurement that matters this round: a worker earning zero boxes was the
### more valuable of the two

P09F-PREFLIGHT closed no boxes and its report is the best artifact of the day.
It refused to claim a production caller it did not have, wrote §0 as "the one
thing to read before anything else: main.rs is not wired", and proposed exact
wiring instead of quietly shipping the mechanism as done. **The orchestrator's
review then found that the proposed wiring could never fire** — `select()`
already resolves the executable — which is a finding only available because the
worker surfaced the gap instead of burying it.

A boxes-per-hour metric would score this worker at zero. It should not be read
that way. **Add a column for "gaps surfaced rather than papered over", because
the tier comparison is otherwise biased toward workers who close boxes on weak
evidence.**

### Orchestrator-side cost

Two integrations, one CI round-trip wasted on a self-inflicted red (`README`
progress block, now practice §24), one genuine finding requiring a rewritten
follow-up packet. Reviewing two finished workers concurrently was comfortable;
the earlier ceiling of three editing workers still looks right.

### Open question answered

*"Does verifying a worker's gates myself pay for itself?"* — This round, no
defect was found by re-running gates: both workers' numbers were exactly right.
But re-running is cheap (one backgrounded script) and the one time it mattered
it caught a worker whose tests did not compile. **Keep doing it, and stop
counting it as a cost.** The expensive check is not the gate re-run; it is
reading what the mechanism actually connects to, which is where both of this
round's real findings came from.

## Batch 15–17 — three concurrent workers, three harnesses, started 2026-08-26 14:15

First round in which the three tiers run **simultaneously and in different
harnesses**, rather than being compared across rounds. Partitioned by files,
each packet naming the other two workers' directories under FORBIDDEN FILES.

| | 2d-routing | p06-adapter | p02b-status |
|---|---|---|---|
| harness | Codex | Codex | agy-gh |
| model | `gpt-5.6-sol` | `gpt-5.6-terra` | Gemini 3.7 Flash |
| boxes in packet | **7** | 1 | 1 |
| files owned | `src/shell/**`, `src/config/mod.rs` | `src/harness/**` | `src/integrations/**` |
| worktree | `glasshouse-2d-routing` | `glasshouse-p06-adapter` | `glasshouse-p02b-status` |

Outcome columns to be filled on integration, including the
"gaps surfaced rather than papered over" column the previous round asked for.

### Why these three and not the obvious three

The first partition drawn was 2D settings + Phase 4 interrupt + Phase 6, and it
was wrong: `grep -rl interrupt` puts the interrupt work in `src/shell/` as well
as `src/pty/`, which is the settings worker's territory. **A partition is a
claim about files, and it is checkable before anyone starts** — one `grep`
per candidate batch is cheaper than discovering the overlap in a merge.

Phase 4's interrupt box stays open deliberately. It is the one open box whose
evidence can only come from `test (windows-latest)`, and **workers never push**,
so it cannot be delegated without the orchestrator in the loop for every
iteration. It belongs to a round where that round-trip is the main activity.

### A counting error worth recording, because it moves a headline number

The handoff reported "254 checked boxes (20%)". The map holds **1470** boxes:
254 checked, **1216 open — 17%**. The first attempt to count them returned zero
of each, because `☑` and `☐` are U+2611 and U+2610 and the pattern was mangled
before it reached `grep`. A count that returns zero for *both* states is not a
finding about the document; it is a broken instrument, and it should have been
disbelieved on sight rather than worked around. Percentages quoted in the
handoff are worth re-deriving rather than inheriting.

### Outcomes for 15–17, filled on integration (2026-08-26, by the incoming orchestrator)

| | p06-adapter | p02b-status |
|---|---|---|
| harness / model | Codex `gpt-5.6-terra` | `agy-gh` (Gemini 3.7 Flash) |
| boxes in packet | 1 | 1 |
| **boxes closed** | **0** | **1** |
| mutation proofs | 1 | 8 |
| gates re-run by orchestrator | all 5 green, numbers exact | all 5 green, numbers exact |
| decisive mutation re-run | yes, killed | yes, killed |
| packet corrected by worker | yes | yes |
| gaps surfaced rather than papered over | yes | yes |

`2d-routing` was still running when this was written.

**p06-adapter closed nothing, and that is the right outcome.** Its box needed
one verified in-place communication-style mechanism, or a second harness with
a verified mechanism of any kind — a bar this ledger recorded on 2026-08-25.
The worker re-read all seven harnesses' native artifacts against newer
binaries (claude 2.1.246, codex 0.149.1, agy 1.1.21, opencode 1.18.22,
cursor-agent 2026.08.11, hermes 0.15.1, pi absent) and found neither. It then
said so, unprompted, in a report whose own headline was `PASS`.

The orchestrator's first reading was that the box *was* closable — "declare
which mechanisms it supports" is satisfied by an honest "none observed", and
Phase 9's `☑ Treat unsupported lifecycle information as unknown` is the same
shape and is checked. **The ledger is what settled it**, because a previous
session had written down a specific falsifiable closing bar rather than a
verdict. That is the argument for recording criteria and not just conclusions:
it survived a change of orchestrator and stopped a wrong tick.

What the batch did buy: every adapter's declaration is now a named constant
citing the artifact it was read from, and one test pins the whole seven-adapter
table. Before it, flipping Claude Code's launch-only `--settings` mechanism to
`StyleChange::InPlace` passed every gate in the repository. **A box that does
not move can still be worth a batch**, and a tier that declines to overclaim is
worth more than one that closes boxes optimistically.

### The denominator, a third time — and the repository already had the answer

This document's own "counting error worth recording" section replaced the
handoff's *"254 checked (20%)"* with *"254 checked, 1216 open — 17%"*, and the
outgoing session repeated the 17% in its handoff as a correction.

`scripts/progress.py` — which writes the README block that the `lint` job
checks on every push — reports **255 / 1267 mandatory (20%)**.

Three numbers, three denominators, and the disagreement is not arithmetic:
**1470 counts every `☐` in the file**, including the `Maybe / Experimental`
phases, the `Explicit Non-Goals for V1` list and the `Product Rules` list —
lines that are deliberately not work to be done, and one of which literally
reads *"Do not build a graph database before demonstrated need."* Counting a
non-goal as unfinished work makes the project look permanently 17% done.

So the original 20% was right, by the only denominator the repository
enforces. The correction was confidently reasoned, arrived with a good story
about a mangled `grep`, and was still wrong, because it answered a question the
repo already had an instrument for.

**The rule this earns:** before re-deriving a headline number by hand, check
whether a script in the repository already computes it — and if one does, its
denominator is the definition, not a candidate. `grep -c '☐'` is a different
question from "how much of the mandatory work is left", and only the second one
belongs in a handoff.

## Batch 18–19 — the team-lead tier, started 2026-08-26 ~14:35

First round using **team leads that subcontract** (practice §10). Two Opus
leads at effort `high`, each given a package far larger than a single worker
packet, each free to spawn its own Sonnet subcontractors in visible cmux panes.
The point is to find out whether pushing the review cycle down a level actually
buys concurrency, since a lead's review cost is paid out of the lead's context
rather than the orchestrator's.

| | lead-memory | lead-events |
|---|---|---|
| harness / model | Claude Code / Opus 5, effort `high` | Claude Code / Opus 5, effort `high` |
| phases | 20, 22, 23, 26 | 12, 13, 45 |
| **open boxes in package** | **40** | **24** |
| files owned | `src/memory/**`, `database.rs`, `cli.rs` | `src/events/**`, `src/session/**` |
| worktree | `glasshouse-lead-memory` | `glasshouse-lead-events` |
| surface | `surface:88` | `surface:89` |

Both packets require a **delegation ledger** in the report: who was spawned, on
what model, what was handed out, what came back, what the lead had to fix, and
what it deliberately kept. Without that the arrangement cannot be measured, only
assumed.

Budget shape this round, which is itself a variable: Claude is on Max 5x and has
headroom, so Sonnet subcontractors are cheap; Codex and Gemini share one €20
subscription, so the leaf tier is rationed to inventories and scans.

**A collision removed before anyone started.** Both packages needed a `mod` line
in `lib.rs` — the one file two file-partitions could not both own. Declaring
both modules empty in a scaffolding commit first cost one commit and removed the
only overlap. Practice §9 says a partition is a claim about files that is
checkable before anyone starts; this is the cheaper corollary — when the overlap
is one line, the orchestrator can just take that line off the table.

### Open questions for this round

- Does a lead's delegation actually raise throughput per orchestrator review, or
  does it mostly move the same serial review one level down?
- Does an Opus lead delegate the things its packet nominated, or keep more than
  it planned to? (The packets nominate specific work as "good to hand out"
  precisely so the gap between plan and behaviour is visible.)
- Is 40 boxes too large a package to stay coherent, compared with 24?

### Outcomes for 18–19 — the team-lead tier works, and the review cost moves rather than shrinks

| | lead-memory | lead-events |
|---|---|---|
| package | 40 open boxes (Phases 20/22/23/26) | 24 open boxes (Phases 12/13/45) |
| **boxes closed on integration** | **31** | **18** |
| subcontractors | 2 Sonnet (`mem-search`, `mem-snapshot`) | 2 Sonnet (`ev-api`, `ev-recovery`) |
| mutations | 30, all run by the lead — 29 killed, 1 explained survivor | 25, all run by the lead |
| lead wall-clock | ~31 min | ~35 min |
| gates on arrival | 4 of 5 green; test gate red by design (see below) | 5 of 5 green |
| packet corrections | 4 | 3 |
| orchestrator work to land it | CLI surface + 4 reconciled tests, one a real find | none beyond gates |

**49 boxes from two leads in one round.** The previous three-worker round
closed one. That is the headline, and the rest of this entry is why it is not
the whole story.

### The three questions this round was opened with

**Does a lead's delegation raise throughput, or move the same serial review one
level down?** Both leads answered independently and agreed: it moves, and it
still pays. `lead-events` measured its subs at 34% of new lines and estimated
30–40% of wall-clock saved against 10–15% given back in packet-writing and
review. `lead-memory` was blunter: *"I still had to read both diffs line by
line, re-run every gate, and write every mutation — the review cost did not
shrink, it moved."* The gain is real but it is parallelism, not less work.

**Does an Opus lead delegate what its packet nominated?** No — and the
deviation was correct both times. Each packet nominated the enums and their
round-trip tests as good to hand out; `lead-memory` kept them, because they
were the frozen API both subs built against and handing them out *would have
serialized the batch*. Both leads independently concluded that **the API must
be frozen before anything is delegated**, which is the real precondition the
packets should have stated instead of a list of nominees.

**Is 40 boxes too large a package compared with 24?** No. The 40-box package
closed 31 and enumerated the other 9 with reasons. Size was not the limit;
**file ownership was.** Four of `lead-memory`'s ten open boxes were blocked
on six lines in `main.rs`, and its own report says so in its second sentence.

### What was not anticipated

**The `lib.rs` stub trick propagated down a level on its own.** Both leads
pre-declared `pub mod x;` with stub files before starting their subs, so no sub
ever had to edit a shared file — the same move made for them one level up,
reinvented by both without being told. A technique that survives being passed
down is worth writing into the packet template.

**A subcontractor caught its own lead's mistakes.** `mem-snapshot` reported that
`lead-memory`'s own `mod.rs` and `store.rs` failed the clippy and rustdoc gates
— a `dead_code` accessor, three `should_implement_trait`, a `collapsible_if`,
three private intra-doc links — which the lead had not yet run. Delegation
caught errors **upward**. The lead then caught the sub's report claiming it had
touched no forbidden file when `cargo fmt --all` had reformatted one, which is
the next finding.

**`cargo fmt --all` edits files you do not own.** It is in every packet's gate
list and it ignores file partitions entirely. A worker told to touch nothing
outside its list will violate that instruction by running its own gates. Only
the diff catches it, never the worker's sentence about it. Packets should say
so.

**Stagger by expected size, not by clock.** `lead-events` staggered its two subs
by ~15 minutes per practice §1 and they finished within a minute of each other
anyway, because the second task was much smaller. The advice as written
optimises the wrong variable.

**A red gate can be the correct deliverable.** `lead-memory` handed over a
failing test gate on purpose: migration 4 legitimately broke four tests in a
file another worker owned, and the packet forbade touching it. It reported exit
101 in its own gate table, in bold, with the exact replacement values. That is
better than a green gate obtained by editing a forbidden file, and the ledger
should stop treating "all gates green" as the only acceptable hand-over.

### The finding that only re-running produced

Three of those four tests were pinned constants. The fourth was a **hole in a
rollback**: both migration tests simulated an older database by deleting *some*
`schema_migrations` rows, and the runner resumes from `MAX(version)`, so once
migration 4 existed the deletion left the max untouched and nothing re-applied.
The lead predicted "change 3 to 4" and stopped at the version assertion — the
second failure is invisible until the first is fixed. Practice §23 says re-run
a worker's decisive observations; this round it says re-run the ones it
predicted would be boring.

## Batch 20–21 — the team-lead tier again, with yesterday's findings written into the packets

Second team-lead round, started 2026-08-26. The variable under test this time
is **whether the previous round's lessons transfer through a packet**, rather
than whether the tier works at all — that was answered by 49 boxes.

| | lead-extract | lead-record |
|---|---|---|
| harness / model | Claude Code / Opus 5, effort `high` | Claude Code / Opus 5, effort `high` |
| phases | 21, 21A | 18, 19, and Phase 12's four stranded lines |
| **open boxes** | **25** | **28** |
| files owned | `src/memory/**` | `src/events/**`, `src/checkpoint/**`, `src/session/**`, `src/shell/**`, `main.rs`, `database.rs`, `cli.rs` |
| surface | `surface:94` | `surface:95` |

Both packets carry an eight-point block of measured findings from batch 18–19 —
freeze the API before delegating, pre-declare module stubs, stagger by expected
size, `cargo fmt --all` crosses file partitions, a red gate can be the correct
hand-over, report surviving mutations, no production caller means no box, and
re-run the observations you predicted would be boring. Whether a lead follows
advice it was given rather than rediscovering it is the thing to look for in
the reports.

### The partition moved, deliberately

`lead-record` gets an unusually wide partition — `main.rs`, `shell/**` and
`database.rs` included. **Every one of yesterday's stranded boxes was stranded
by a file boundary, not by difficulty.** Four Phase 12 lines and four Phase 26
lines sat finished-but-unreachable because the lead that built them could not
add a caller. Widening one lead's partition and narrowing the other's is
cheaper than a third round of "behaviour built, surface missing".

The cost is that only one lead can hold `database.rs`, so `lead-extract` was
told to report DDL rather than write it — the same constraint that produced a
correct red gate last round.

### Open questions for this round

- Do the transferred lessons actually change behaviour, or does each lead
  rediscover them anyway? (The reports will say: a lead that was told to freeze
  its API first and did so will not report it as a discovery.)
- Does a wide partition close the stranded boxes, or merely move the boundary?
- Phase 19's checkpoints are the first capability whose value is cross-harness.
  A checkpoint that only ever bootstraps the same harness proves little.

## Batch 20–21 outcome — two Opus team leads, and the answers to the round's questions

| | `lead-record` | `lead-extract` |
|---|---|---|
| package | 28 boxes (Phases 19/26/45) | 25 boxes (Phases 21/21A) |
| partition | wide — `main.rs`, `shell/**`, `database.rs` | narrow — `memory/**` only |
| subcontractors | 2 (Sonnet 5, medium) | 3 (Sonnet 5, medium) |
| mutations | 13 run, 3 first-run survivors, all then killed | 23 run, 22 killed, 1 predicted survivor |
| boxes closed | **25 of 28** | **17 of 25** |
| closable *as delivered* | 25 | **0** |

**Answer to "does a wide partition close the stranded boxes, or merely move the
boundary?" — it closes them, decisively, and the control group proves it.**
The two leads ran the same round with the same model, effort and process, and
differed mainly in partition width. The wide one closed 25 of 28 from its own
worktree. The narrow one closed **zero** — not for want of quality (23
mutations, 22 killed, +81 tests, the strongest evidence in the batch) but
because *nothing in the shipped binary produced a memory*, and every line that
would give the extractor a caller lived in `main.rs`, which it was forbidden.
Two `main.rs` patches, 2 lines and ~40 lines, took it from 0 to 17.

This is now observed three times — `lead-memory` (4 of 10 boxes blocked on six
lines of `main.rs`), `lead-record`'s two caller-only boxes, and this. It is a
rule, not a pattern: **a producer phase must have its trigger's file in the
partition, or the batch ends in a patch instead of a tick.** The packet
template gets that line.

**Answer to "do transferred lessons change behaviour, or does each lead
rediscover them?" — they change behaviour, and the evidence is what the reports
*stopped* saying.** Neither lead reported freezing its API first as a
discovery; both did it and moved on. `lead-extract` reported its surviving
mutation with reasoning and a decision to keep the code, which is exactly what
the transferred block asked for, rather than presenting 23/23 killed. The
lessons that were *not* transferred are the ones that got rediscovered: the
`MAX(version)` rollback trap surfaced for the third time in a third file, and
the shared-mutable-state hazard for the sixth.

**Answer to "is 40 boxes too large?" — the question was wrong.** Neither lead's
count was limited by box count. `lead-record`'s 28 and `lead-extract`'s 25 both
finished inside their context. What bounded the result was **file partition
width**, not package size. Size the package by the files a capability's
production caller lives in, and stop sizing it by how many lines the map lists.

### The measurement that pays for the whole team-lead arrangement

Across the two batches, **five mutations were killed only by a subcontractor's
tests** — three in `lead-extract` (M20, M21, M23), two reported by the 9G lead
earlier. A lead's own tests are written by whoever wrote the code and inherit
its blind spots; a subcontractor writing tests against a frozen API does not
share them. This is the third independent observation and the strongest
argument in this ledger for paying the lead tier's review cost.

Second only to it: **`lead-record` had three mutations survive on their first
run**, each exposing a claim nothing tested — including one where the box's own
subject (an `observe()` call) could be deleted and only a debug log noticed.
Three boxes would have been ticked on nothing. Mutation testing is not a
formality in this process; it is where the batch's real defects are found.

### One zero-box finding worth more than most closed boxes

`lead-extract` established, and reproduced in both directions, that
`scripts/ci-local.sh`'s three Linux jobs **could not run from a git worktree at
all** — one docker volume shared by every checkout, and `.git` is a directory
in the main checkout but a *file* in a worktree, so `tar` refused the copy and
all three jobs reported FAIL having compiled nothing. Every editing worker on
this project is in a worktree by definition, so a third of the gate had been
silently unavailable to all of them, failing in the shape of a broken build.
Diagnosed, not applied — the file was not in its partition and other workers
were live. Fixed by the orchestrator during `lead-record`'s integration, which
had hit the same shared volume from the other direction.

### Open questions for the next round

- The wide-partition rule says put the caller's file in the partition. With one
  `main.rs` and several leads, does that serialize the leads — and if it does,
  is a thin "wiring" batch after each round cheaper than the serialization?
- Both leads independently reported the same surviving mutation (a
  `project_id` filter made redundant by structural per-project database files).
  Is one test asserting the *structure* the right answer, and does it retire
  the survivor in both places?
- `lead-extract` spent its evidence on a capability with no consumer, and it
  was still the right work. How should a package that ends in a seam rather
  than a surface be scored, given the box count says zero?

## Batch 22–23 — designed to answer §32's own open question

Two Opus leads at effort `high`, started together on `d62bc7a`.

| | `lead-mem6` | `lead-route` |
|---|---|---|
| package | 18 boxes — Phase 21 remainder (7) + Phase 21B (11) | 28 boxes — Phase 9H (14) + 9I (14) |
| owns the contested file | **`main.rs`**, `cli.rs`, `database.rs` | `lib.rs`, `shell/**`, `gateway/**` |
| caller lives in | `main.rs::report_hook` — its own partition | `gateway::start_if_required` — its own partition |
| phases' prior state | 6 of 25 closed last round | **both untouched, 0 of 28** |

### The design, and what it is testing

§32's open question was: *with one `main.rs` and several leads, does the
put-the-caller-in-the-partition rule serialize the leads?* This round says no,
and the reason is worth writing down before the reports come back so it can be
scored honestly.

**`main.rs` was not the only caller site — it was the only one anybody looked
for.** `lead-route`'s Phase 9H needs a provider/model assignment made when a
session starts, and `main.rs::launch_session` is where the gateway is started —
so the obvious reading is that both leads need `main.rs`. But
`start_if_required` *is* `gateway/mod.rs`, which one lead owns outright: the
assignment can be made where the gateway binds the profile, and the call site
never changes.

So the rule refines. **Do not ask "which file calls this?" — ask "which
function must change?"** A caller that is a one-line invocation of a function
in your own module is not a partition conflict; only a change to the invocation
itself is. If that distinction holds, two leads can both satisfy §32 against a
single `main.rs`, and the serialization §32 feared is mostly an artefact of
reading call sites at file granularity.

**The falsifier is stated in advance:** if `lead-route` comes back with a
`main.rs` patch it could not avoid, the refinement is wrong and the answer to
§32's question is "yes, it serializes" — in which case the thin-wiring-batch
alternative should be tried next round rather than argued about.

### The other thing this round tests

`lead-route`'s package is **28 boxes of policy**, which is the ideal shape for
building 28 mechanisms nothing calls — a cooldown nothing consults, a health
score nothing reads. Its packet names that trap explicitly and asks it to name
the code path that will *ask* each policy before writing it. Phase 9I's
consumer is the `ExtractionModel` seam whose caller `lead-mem6` is building in
the same round, which makes the two packages a genuine integration test of each
other rather than two independent piles.

### Questions to answer from the reports

- Did the caller-granularity refinement hold, or did `lead-route` need
  `main.rs` after all?
- Phase 9I line 539 — *Glasshouse's own evaluation and test runs use zero-cost
  models, never a metered resource without explicit opt-in* — is an acceptance
  condition with real money behind it. Was it built as a control, or as a
  default someone can change?
- Does a package of pure policy lines produce a lower closable rate than a
  package with obvious mechanisms, holding lead tier and effort constant?
- Does the standing Linux pty flake (§34) get diagnosed by whoever owns
  `pty_smoke.rs`, now that the attribution procedure is written down?

### Batch 22–23, first half: `lead-route` closed 22 of 28, and the falsifier did not fire

| | result |
|---|---|
| boxes | **22 of 28 closable** (9H 13/14, 9I 9/14) |
| wall clock | 57 minutes |
| mutations | 25 run, **25 killed**, 3 only after a survivor forced a fix |
| tests | +90 in the lib target, +18 `routing_policy.rs`, +6 `settings_persistence.rs` |
| size | +2200 / −178 tracked, +4421 in 7 new files |
| gate | all ten `ci-local.sh` jobs green, Linux included, no pty flake |
| subcontractors | 2 Sonnet at medium, both in visible panes with watches |
| **`main.rs` patch needed** | **none** |

**The §32 refinement held.** The falsifier stated in advance was: *if
`lead-route` comes back with a `main.rs` patch it could not avoid, the
refinement is wrong and the rule serializes leads.* It did not.
`main.rs::launch_session` is byte-for-byte unchanged, and `gateway_upstream`'s
signature was deliberately left alone to keep it that way, because the
assignment could be made inside `apply_gateway` — a function in the lead's own
module that `launch_session` already calls. **Two leads satisfied §32 against a
single `main.rs` in the same round.** Ask which *function* must change, not
which file calls it.

**But the packet was wrong about 9I, and the lead caught it.** The packet
asserted 9I's consumer was the `ExtractionModel` seam `lead-mem6` was building.
That seam is a caller for *extraction*, not for *model selection* — nothing
asks a router which resource a disposable job should use — so four of 9I's
boxes (530/531/532/540) were unclosable from that partition before the batch
began. The tell cost one command: `grep` for a call site of
`ExtractionModel::complete` outside a test finds one, in `main.rs`, behind
`--reply-from`. Recorded as practice **§36**: name the function that will *ask*
the policy, and check it is being built *for that purpose*.

So the partition question now has two parts, and the second is the harder one:
1. Is the caller's file in the partition? (§32 — answered, and satisfiable)
2. Does a caller that *exercises this policy* exist at all? (§36 — the packet
   got this wrong, and it cost four boxes)

**Answer to "does a package of pure policy lines produce a lower closable
rate?" — yes, and the mechanism is now visible.** 22 of 28 is a high rate, and
every one of the six misses is a missing consumer rather than a missing
mechanism. Policy packages fail at the consumer end, mechanism packages fail at
the caller end, and they need different questions asked before the packet is
written.

**The finding worth more than the boxes.** M18 deleted the production launch
path's only call to `routing().bind` and the entire suite passed, because all
ten conformance tests bound the assignment in their own helper. A caller every
test bypasses is not a caller — practice §35. Two of the three fixed survivors
were this same shape. **Mutating the callee is not enough; mutate the call.**

**Live-run findings, from a real terminal against a real router.** A `402` was
being classified as a healthy exchange when it is really this account's key
being unable to pay — another key would serve. No fixture would have produced
it. And the honest limit: **no live `200` was obtained at all**, because
OpenRouter answers `402` for `:free` models on an account that never purchased
credits. The account's state, not the model's price, decides whether a free
request is servable — practice §38.

### Batch 22–23, second half: `lead-mem6` closed 14 of 18, and Phase 21B closed entire

| | result |
|---|---|
| boxes | **14 of 18** closed (the lead argued 15; the orchestrator held one — see below) |
| wall clock | ~1h20m |
| mutations | 8 run, 7 killed, **1 survivor that was weak in the same way its test was** (§41) |
| tests | +109 local (1145 → 1254), of which **+24 from two subcontractors** |
| gate | all ten green on the final run; one intermediate Linux FAIL, attributed |
| subcontractors | 2 Sonnet at medium |
| **Phase 21B** | **11 of 11 — the phase is complete** |

**Both halves of batch 22–23 together: 36 of 46 boxes, 335 → 388 mandatory.**

**The round's design question is answered in full.** §32 asked whether putting
the caller's file in a lead's partition serializes leads against a single
`main.rs`. It does not: `lead-route` needed no `main.rs` change at all, and
`lead-mem6` owned it and used it. Two leads, one `main.rs`, one round, and the
falsifier stated in advance did not fire.

**But the two halves failed in opposite directions, and that is the finding.**
- `lead-route` (policy package): 22 of 28, and **every miss was a missing
  consumer** — four free-pool boxes share one absent caller.
- `lead-mem6` (wiring package): 14 of 18, and **its misses are a missing
  callee** — the trigger is built and complete and there is no model to call.

A package fails at whichever end of the chain the partition did not reach.
Naming the caller (§32) and naming the function that *asks* the policy (§36)
are the same discipline applied to the two ends, and a package needs both
questions asked before it is written.

**The orchestrator overruled one closable verdict, and the lead set it up to be
overruled.** `lead-mem6` closed *"allow memory extraction to run after task
completion"*, cited practice §33 as precedent, stated the counter-argument, and
noted that if the strict reading won then §33's own earlier decision needed
re-examining by the same standard. That is the ideal shape for a deferred
judgement. The answer sharpened §33 into a criterion that survives both cases:
**does the capability complete and produce its result in the shipped binary?**
Manual extraction does — memories are stored. The task-completion trigger does
not — it fires every time and dead-ends. Verified by running the binary, twice,
once by each of us.

**Cost of the orchestrator's own mistake, measured.** One relay mid-batch
(§39) was wrong on the merits and ended the lead's turn; it sat idle with three
shells open until a watch caught it. Recovery cost one nudge and about ten
minutes. The lead's refusal was the more valuable artefact — it produced the
`seq`-renumbering hazard that nobody had noticed and a fully specified
migration 7.

### What the next batch should be, in order

1. **The disposable wiring** — one line in `main.rs` swapping `NoExtractionModel`
   for a `DisposableRouting`-backed model. Closes `lead-route`'s 530/531/532/540.
   The seam is built: `report_hook_with` takes `impl Fn() -> Box<dyn ExtractionModel>`.
2. **Migration 7** — own `database.rs` + `events/mod.rs` + `events/log.rs`
   together; rebuild `lifecycle_events` with `gateway_backend_changed`; prove
   `seq` survives with a test storing a memory's event range across the rebuild.
   Closes nothing by itself and unblocks `lead-route`'s 515 durably.
3. **The Linux pty flake** (§34/§40) — a bounded wait on the observation, the
   same treatment `integrations/version.rs`'s ETXTBSY race got. It is a standing
   debt on the only gate this project has.

## Batch 24 — three parallel single workers, and the flake that was a real defect

Three workers started together on `81e9c16`, fully disjoint file sets, verified
by intersecting their `YOURS` lists **after** the round (which is when the
`shell/state.rs` double-assignment was found — see §43; it did not bite).

| worker | model | boxes | outcome |
|---|---|---|---|
| `wire-disposable` | Sonnet, medium | 4 | **4 of 4**, Phase 9I now 13 of 14 |
| `migration-7` | Sonnet, medium | 0 | migration landed, `seq` proven durable |
| `pty-flake` | Opus, high | 0 | **the gate's random failures fixed** |

**Two of the three closed no boxes and both were worth more than the one that
did.** This is the round that tests whether a package has to be scored in
boxes, and the answer is no: `migration-7` made Phase 9H's line 515 durable and
proved the hazard nobody had noticed, and `pty-flake` repaired the only gate
this project has. A box count would have scored this round 4 and missed both.

### The flake, with rates

**8 failures in 17 full-suite Linux runs before; 0 in 20 after.** Twenty runs
rather than five because a residual 10% rate survives five clean runs 59% of the
time and twenty only 12% — against a 47% baseline, 20/20 is decisive and 5/5
would have proved nothing. This is what practice §34's "a flake needs a rate,
not an anecdote" is for, and it is the first time the project has had one.

**The orchestrator's hypothesis in the packet was wrong**, killed with 600
trials (§44). The real cause was measured in situ: a pty child's exit is
observable ~1.1–2.2ms before its output is. **Glasshouse never lost a crashed
harness's output; it reported it as absent when asked inside that window** — a
smaller defect than budgeted for, and one the gate could not distinguish from
the larger one.

A rarer, *different* failure survives at 1 in 37 and is documented with four
hypotheses ruled out by data and a ranked list of where to look next.

### What this says about worker tier

`pty-flake` was the only Opus worker and it was the right call: the job was
open-ended debugging with a wrong premise to overturn, 600-trial and 2400-spawn
control experiments, and a judgement about whether the defect was in the product
or the test. The two Sonnet workers did precisely specified work precisely, and
`migration-7`'s handling of three cross-partition conflicts — patch locally,
verify, **revert to exact committed bytes**, report the patches — is the
standard for every tier.

**Cost note.** `pty-flake` ran ~37 full workspace suites in Docker. That is real
wall-clock and real CPU on the user's laptop, and it is why §40 says run the gate
alone. Budget a flake hunt as a container-hours job, not a code-reading job.

### Open questions for the next round

- The residual `SIGABRT` at 1 in 37 keeps the gate occasionally red. Is a second
  Opus batch on it worth it, or is the ranked list enough to hand to whoever
  next sees it fail?
- Phase 9I's last line (528) needs a feed for token-priced allowances, and the
  gateway deliberately does not parse the headers that would supply it. Is that
  a Phase 32 job rather than a 9I one?
- Two rounds running, the highest-value findings have come from work that closed
  no box. Should a round deliberately budget one worker for debt rather than
  boxes?

## Batch 25 — one Opus worker on a defect the whole test suite had missed

**One worker, one defect, no boxes closed.** `terminal-loss`, Opus at high
effort, on the 100% CPU spin the user found at 501% across five processes. The
round before it asked whether a round should deliberately budget a worker for
debt rather than boxes; this is that worker, and it is the second consecutive
round where the highest-value output closed nothing.

| | |
|---|---|
| tier | Opus, high effort, isolated worktree |
| partition | `tui/event.rs`, `tui/mod.rs`, new `tests/terminal_loss.rs` |
| delivered | +195 lines of production code, a 461-line acceptance test |
| boxes closed | **0** |
| gate | 12/12 local, first attempt on the final tree, no Linux pty flake |
| mutations | 3, all rebuilt, all `FAILED` with the CPU message; 1 re-run on Linux |
| orchestrator re-verification | M1 and M2 reproduced independently (§23) |

### The packet's cause was wrong in a way that would have produced a wrong fix

Recorded as practice §58. The packet's account matched every observable and
still prescribed a remedy that could not work — the pre-check it suggested runs
between calls to `event::poll`, and the hangup lands *inside* one. The worker
read crossterm's source, found `Ok(0)` falls through an inner loop with no
timeout, and moved the wait out of the library instead. **This is the second
consecutive batch in which the orchestrator's stated hypothesis was overturned
by the worker it was handed to**, and both times the correction was the batch's
main product.

It also rewrote the incident report: the orphans were not un-signalled. A
`SIGHUP` had arrived and been recorded; the loop never returned to the line that
reads the flag.

### An acceptance test that passed on the unfixed tree

Recorded as §59. Without a settle before the hangup, the test passed on Linux
with and without the fix — it was exercising a startup-error path, not the
defect. Found because mutations were run on **both** platforms; a macOS-only
mutation pass would have shipped a vacuous Linux test.

### §18 earned its keep, in the one place this machine cannot test

Compiling the non-Unix path locally with the cfg flipped caught a real break:
with no `#[cfg(unix)]` constructor, two `Wait` variants are dead code, and
`dead_code` under `-D warnings` is an **error**. The Windows job would have
failed on a tree whose twelve local jobs were green. That is the whole return on
§18 in a single batch.

Windows is otherwise **deliberately unhandled and says so in the function's doc
comment**: a console going away raises `CTRL_CLOSE_EVENT` on a handle rather
than endless zero-byte reads on a descriptor, and no one here can run a native
Windows terminal to check. `Wait::Unavailable` keeps the old behaviour there
byte for byte. A stated gap was preferred to a guessed branch, and the packet
asked for exactly that.

### What this says about worker tier

Opus was the right call and would have been wrong to economise on. The job
required reading a dependency's source to contradict its own packet, a measured
`POLLHUP` probe rather than an assumed one, an `EINTR` branch found only by a
second failing round, and the judgement to leave Windows alone. A Sonnet given
"add a `libc::poll` pre-check as described" would have produced a clean,
mutation-proofed, useless patch — and the mutations would have passed, because
they would have been mutations of the wrong line.

### Two defects found in passing, neither patched, both outside the partition

1. **An attached session never learns its terminal died.**
   `session/attach.rs:255` — `pump_input` breaks on `Ok(0)` and its thread ends;
   `supervise` then waits forever on a harness nobody can see or type at. It
   does not spin, so it was not in the 501%, but it is the same missing
   question. `request_shutdown()` on hangup does not reach it — `attach` runs
   while the TUI does not. Suggested shape: `Ok(0)` calls
   `shutdown::request_shutdown()`, which `supervise` already watches at line 185.
2. **Ratatui panics on the way out of a dead terminal.** `Terminal::drop`
   `eprintln!`s when it cannot show the cursor, and that is itself a panic on a
   hung-up pty, so some paths exit 101 rather than 0. Harmless — the loop has
   already returned — but a clean exit is worth having.

Both belong to a follow-up package. Neither was folded in: the partition was the
partition (§32).

### Open questions for the next round

- The residual `SIGABRT` at 1 in 37 is still open, still ranked, still unowned.
- Two rounds asked whether to budget a worker for debt. Two rounds answered yes
  by accident. Should the round template reserve one slot for it outright?
- The Windows host exists now. The first `--windows-vm` run is expected to fail
  in several places at once; is that one reconciliation package, or one worker
  per failing job?

## Batch 26 — three parallel workers, and the round where a worker's best output was a refusal

Three workers, disjoint partitions, validated by `validate_round.py` before
dispatch (the first round where that gate ran on real packets rather than on
itself). Two closed work; the third closed none and was the most valuable.

| worker | tier | outcome |
|---|---|---|
| `pairing` | Opus, high | **9 of 20** Phase 9J boxes; 11 assessed as blocked, with the phase each waits on |
| `hangup-followup` | Sonnet | two defects found by the previous batch (reported separately) |
| `phase0-evidence` | Sonnet, verifier | 8 boxes examined, **2 unticked**, the evidence backlog closed |

### A verifier that unticks two boxes is doing the job, not failing it

`phase0-evidence` wrote no production code and produced the round's two hardest
findings. Phase 0's box 2 is **unsatisfiable by any tree that satisfies its own
phase** — `clap` is what boxes 5 and 6 are built on, `tracing` is what box 7
*is*, `directories` is what box 4 needs, and none of the three is in the six
categories box 2 permits. That is a specification defect, and a worker with no
authority to edit the map found it by enumerating a dependency tree nobody had
enumerated before.

Its box 8 finding was reproduced **first-hand and independently** of the worker
that was fixing the same defect in a neighbouring worktree, before that worker
reported. Two independent observations of one defect is worth more than one
observation repeated, and the packet is what produced it: it named the sibling
worker's report as something to *check for*, not something to wait on or trust.

**The transferable rule: give a verifier the authority to recommend an untick
and it will use it.** Both recommendations came with the reasoning and neither
touched the map. That is the right division — the worker establishes, the
orchestrator edits, and the user decides anything that is a product question.

### 0 of 11, refused rather than faked

`pairing`'s group 2 assessment is the deliverable the packet asked for and the
answer was total. `grep -rn 'fn score\|Score' src/` is empty: the binary has two
routing callers and neither ranks anything, so a "positive initial routing
prior" has nothing to be a term of. Eleven boxes, each mapped to the phase it
waits on, two of them noted as partly built so the next worker does not start
from zero.

Line 576 (four native-pairing preference values) is explicitly called out as
half an hour of plumbing that would have looked like a tenth closed box and
been a field parsed and never consulted. **A worker declining to close a box it
could trivially fake is the behaviour this process is trying to buy**, and it
is worth more than the box.

### Cost of the mid-flight packet error

None this round. `validate_round.py` refused nothing after the packets were
reshaped to its `**YOURS**` / `**FORBIDDEN**` block format — worth knowing that
the tool requires bold headings, not `##` headings, which cost three minutes and
one confused re-read.

**One orchestrator error, caught before dispatch.** `git -C <repo> worktree add
-b <branch> <relative-path>` creates the worktree **inside** the repository,
because `-C` resolves the relative path from the repo rather than the shell. All
three landed in `crates/`'s parent as untracked directories. Removed and
recreated with absolute paths before any worker started, so nothing was lost —
but a `git status` that suddenly shows three untracked worktrees is a
five-minute detour at best and a swept-in commit at worst.

### Verification the orchestrator ran itself (§23)

- `pairing` M1 (an unattributed model answering `VendorNative` instead of
  `Unknown`) reproduced independently: **4 unit tests and 2 integration tests
  failed**, which is stronger than the report claimed — the production path
  catches it, so §35 is satisfied rather than asserted. Restored byte-identical.
- The §33 end-to-end claim re-run on the built binary: a `[pairing.models."<id>"]`
  table added to a throwaway config moved the class from `unknown` to
  `protocol-native` and the report named the layer the correction came from.
- `phase0-evidence`'s dependency finding re-run with `cargo tree --depth 1`:
  22 direct dependencies, 11 outside the six categories, and **no async runtime
  at any depth**.
- The mutation table cited ten test names that do not appear in
  `tests/pairing.rs`. They are unit tests inside `src/harness/pairing.rs` — all
  ten exist. Worth checking every time: a citation that does not resolve is the
  cheapest possible tell, and this one resolved.

### Open questions for the next round

- Phase 9J line 572 duplicates Phase 33A's tenth line almost verbatim. Does a
  requirement ever belong in two phases, or should the map move it?
- Two rounds now have produced their best output from a worker that closed no
  box. The round template should probably reserve a verifier slot outright
  rather than discovering the need each time.
- The residual `SIGABRT` at 1 in 37 is still unowned. It did not appear in
  either of this round's gate runs.

### Batch 26, addendum: `hangup-followup`, and a defect the previous batch's fix created

`hangup-followup` (Sonnet) fixed one of its two defects with a mutation-proofed
test, could not prove the other, said so, and found a third that neither packet
anticipated. All three outcomes are the right ones.

**Defect 1 (an attached session never learns its terminal died) has no test, on
purpose.** The worker built the suggested acceptance test first, found it
**passed with the fix fully reverted**, instrumented `supervise` to trace when
the shutdown flag went true, and discovered that `ctrlc`'s `termination`
feature already delivers `SIGHUP` to the same flag `supervise` polls every
20ms. It reported: *"no, my first test would not have caught the original
defect, because the signal path was never the thing missing."* It kept the fix
as a second independent way to set the flag, and wrote no mutation table
because mutating an inert fix would be §41's vacuous mutation exactly.

**A Sonnet declining to produce a test that appears to prove more than it
does** is the same behaviour `pairing` showed declining line 576, from a
different tier and a different direction. Two of three workers this round chose
an honest gap over a plausible artefact. That is what the packets are for.

**Defect 3 was created by the batch-25 fix and found by the batch-26 worker.**
`shutdown.rs` implemented "a second signal forces the process down" by reading
`SHUTDOWN_REQUESTED` — correct for as long as a signal was the only thing that
could set it. `wait_for_terminal` began setting it on hangup, and a hangup
delivers `SIGHUP` and `POLLHUP` at the same instant, so one event observed
twice read as two impatient interrupts and `force_exit` skipped every
destructor. **Eight of ten on macOS, ten of ten in a Linux container.**

The transferable part: **a fix that sets a shared flag inherits every meaning
anything else attaches to that flag.** `request_shutdown()` looked like a pure
addition and silently redefined the signal handler's second-signal test. Before
setting process-global state, read every consumer of it — there were two, and
the second was in a file the worker did not own.

Fixed by the orchestrator rather than dispatched: five lines, Red-tier
signal/lifecycle work that `worker-capabilities.md` puts at specialist level,
and it was the previous integration's own regression. The policy is now a named
function (`interpret_signal`, §36) so it can be tested at all; the old line was
re-applied as the mutation and fails the new test with the message that
describes the defect.

**And then the orchestrator's own claim did not survive its own measurement.**
"The spin is gone", written into batch 25's commit message on the strength of a
passing acceptance test and three killed mutations, is wrong. A sixty-trial
harness on the shipped tree caught the process alive at `Rs+ 100.0` twice.
`terminal_loss.rs` is not testing the wrong thing — `portable-pty` does
`setsid` and `TIOCSCTTY`, so it exercises the failing case — it runs it once
against a defect that fires about one time in thirty. Recorded as practice
**§60**: for a race, a one-shot pass is consistent with a residual rate of
anything up to roughly 1 in 3, and mutation-proofing does not close that gap
because mutations test the test, not the tail.

**Cost of finding it: about fifteen minutes and two builds.** The first harness
was itself wrong in the §59 way — `start_new_session=True` gives a child no
controlling terminal, so no `SIGHUP` is delivered and it reported 10/10 clean
exits both with and without the fix under test. It measured nothing, twice,
before the harness was corrected. Kept at `.agent-runtime/diagnostics/` with
that failure written down, because the next worker on this defect will
otherwise build the same wrong harness.

## Batch 27 — 20 of 20, and a mutation that survived

Two workers, both Opus at high effort, disjoint partitions.
`response-profiles` closed Phase 9K's first two groups entire and assessed the
other seventeen. `spin-residual` was still running when this was written.

| | `response-profiles` |
|---|---|
| boxes | **20 of 20 owned**; 10 of the remaining 17 blocked, 7 argued |
| delivered | 3,046 new lines across four files, 836 insertions in seven |
| mutations | **19 run, 19 killed** — one only after it survived and forced a redesign |
| gate | 12/12, twice, run alone; no `SIGABRT` flake in either |
| orchestrator re-verification | the decisive external probe and the safety-property mutation, both reproduced |

### The mutation that survived is the most valuable thing in the batch

M1b — one axis silently setting another — **survived its first run**.
`ResolvedProfile` stored the five resolved values a second time beside their
sources, and the report printed the stored copy. So a build in which one axis
forced another **printed the honest value and shipped the mutated one to the
harness**, and nothing in the system could tell the difference.

The worker did what §41 says to do when a mutation survives — asked what the
test and the mutation both assumed — found the answer was "the report reads the
profile", which it did not, and removed the second copy. M1b then killed.

**A surviving mutation is not a failed mutation.** This one found a defect that
no passing test could have surfaced, because every test agreed with the report
and the report agreed with itself. The rule worth carrying: *where a value has
two homes, a defect gets to live in the gap* — and a mutation is how you find
out there were two.

### The worker caught its own design being wrong, by running the binary

Its first `response_stack` filled the role layer from `Role::default_preset()`
unconditionally, so all five axes were always answered at layer three and
**layers four, five and six could never win** — the precedence chain present
and its bottom half structurally dead. Running the binary with no configuration
is what showed it; no unit test would have, because each layer's own test
passed. §33 earning its keep at the design stage rather than at review.

### External observation with a control, and it changed the design

`claude --settings A --settings B` honours only `B`. A second `--settings` does
not merge and does not error — it discards the first. Re-run by the
orchestrator (§23): a malformed document passed **first** produces no
complaint; the same document passed **last** produces
`outputStyle: Expected string, but received number`.

Had the design appended its own `--settings`, every lifecycle hook in the
session would have been silently switched off. **This is the fifth time this
project has been saved by probing a claim rather than reasoning from a
plausible one**, and the first where the probe changed the shape of the code
rather than the confidence of a declaration.

The worker also declined to build on the packet's framing. The existing
`COMMUNICATION_STYLE` declaration cited a status-line payload — enough to
support "a session has an output style", not "writing key X selects it". It
read the key out of the shipped bundle and probed it, and said what it would
have concluded had the probe failed.

### A packet error, caught by the worker rather than by the round gate

`YOURS` omitted `crates/glasshouse/src/shell/mod.rs`, which calls
`install_hooks`, so line 605 ("keep every spawned worker's response profile
explicit") is reachable only on the launch path and not through the shell's
quick-open. The worker **did not edit it**, kept `install_hooks` as a documented
shim over the new composer, and reported the ten-line patch it would have made.

`validate_round.py` cannot catch this: it checks that partitions are disjoint,
not that a partition is *complete* for the work described. Two rounds now have
had a caller left outside the partition — §32's own subject. Worth asking
whether the round gate can grep a packet's box lines for verbs like "every
session" and warn when the obvious caller is not in `YOURS`.

### Open question the round created

The worker built the additive `--append-system-prompt` path, which belongs to
group 3, because line 604 (its own) requires recording that a native, additive,
or fallback mechanism was applied — and with no additive mechanism that branch
is unreachable and 604 is half-real. It **did not claim** 613/614/615 and left
the call to the orchestrator, with the argument written per line. Left unticked
here. **Should a worker be allowed to close a neighbouring group's box when its
own box is otherwise unprovable?** The conservative answer taken today costs
three boxes that are arguably done.

### Batch 27, addendum: `spin-residual` — the round that produced a rate instead of a pass

The packet asked for a rate and got one, with a confidence bound and an explicit
refusal to claim zero.

| tree | trials | survivors |
|---|---|---|
| shipped | 200 | 7 (3.5%) |
| **fixed** | **400** | **0** |
| shipped, orchestrator's own matched run | 100 | **6** |
| fixed, orchestrator's own matched run | 100 | **0** |

`0 in 400` bounds a residual below about **0.75%** at 95% confidence — a ≥4.7×
reduction, **not** elimination, and the report says so in those words. That
sentence is the whole return on §60. The previous round wrote "the spin is gone"
on a one-shot pass and was wrong.

### The record was corrected twice, both times by measurement

**The compiled backend is crossterm's `mio` source, not `tty`.** §58's account
of the defect was read off `tty.rs`, which in crossterm 0.29 *does* `break` on a
zero-byte read and would never hang. Right mechanism, wrong file — and it
matters, because the `use-dev-tty` feature selects the other one.

**The window is the duration of the call, not a gap between calls.** Profiling
an *idle* process put 268 of 6210 and 233 of 6162 main-thread samples — about
**4% of every tick** — inside `crossterm::event::poll`. A hangup arriving at a
uniformly random instant lands there about one time in twenty-five; observed 7
in 200. After the fix: 0 of 6185.

That is the arithmetic the previous batch's "microseconds" claim failed, and the
packet's instruction to distrust it is what produced the profile. **Handing a
worker the orchestrator's own arithmetic against the orchestrator's own prior
claim is a cheap, repeatable move** — it cost two sentences and bought a
located cause.

### Two mutations that did not kill, and both were reported rather than hidden

- **M1 (the fix reverted) survived**, and the worker measured *why*: a mutation
  restoring a ~3% failure rate cannot reliably fail a 15-trial test. Run sixteen
  times, it failed twice. It said so, and named what carries the proof
  instead — the 400-trial rate, and the fact that a gate run executes the suite
  four times (two platforms, two Rust versions), 60 hangups per gate.
- **M5 survives on purpose**: the seeding branch's `false` has no observable
  consequence, only a cost. §41's question — what do the test and the mutation
  both assume — answered honestly as "there is no behaviour here to test".

A mutation table with two survivors and the reasoning for each is worth more
than one with none and no reasoning.

### A test-harness defect that would have hung the gate

`Shell::kill` called `portable_pty::Child::kill`, which on Unix sends **`SIGHUP`,
not `SIGKILL`**. Both of `terminal_loss.rs`'s failure paths go through it and
both can hang there forever — a Glasshouse wedged in crossterm never reaches the
shutdown the signal asks for, and one that *is* winding down blocks writing its
last frame to a pty nobody is draining, because the draining thread is the one
in `wait`. Observed for eleven minutes in state `E`. **A gate that hangs reports
nothing where a failed assertion reports a defect.**

### The freshness freeze worked, and is now §61

The packet forbade `shutdown.rs` and said *which* change had made it fresh and
why it mattered. The investigation led straight back there; the worker stopped,
explained why nothing inside `next()` can close the window — once crossterm is
wedged the main thread cannot observe the flag, a signal, or even a closed
descriptor — and proposed the watchdog. A bare prohibition would have produced a
quiet workaround instead.

## Batch 28 — the VM's first honest run, and a lead that refused a trade

Two Opus workers in parallel: `windows-session` (specialist) and
`lead-session-model` (team lead, ran it itself). Both PASS.

| | `windows-session` | `lead-session-model` |
|---|---|---|
| boxes | 0 — a defect package | **14 of 14**, 0 blocked |
| mutations | 4 run, 4 killed, **on the VM** | 18 run, 18 killed |
| gate | 13/13 plus Windows | 13/13, twice |
| delivered | 36 lines of test fix, 134 in `ci-local.sh` | 2,019 lines across 13 files + 2 new |

### "1069 passed, 1 failed" was a truncated run reported as a near-perfect one

`cargo test` stops after the first failing test **binary**. One failing library
test meant the three integration suites after it had **never executed on
Windows at all**. Repairing that one test let the run reach them and found four
more failures on an unmodified tree.

**The transferable rule: a suite that stops at the first failing binary reports
a floor, not a result.** Any "N passed, 1 failed" from a multi-binary suite is
worth re-reading as "N passed, 1 failed, and an unknown number never ran."

### A test defect that looked exactly like a product defect

The orchestrator handed over five observations, all consistent with "the child
runs and its output is lost" *and* with "the child never runs". Those prescribe
different fixes, so the worker made the cause **predict something the symptom
did not already say**: it gave a harness a side channel outside the pty. With
nobody answering ConPTY's `ESC[6n`, the marker file did not exist after three
seconds. **The child had not started.** ConPTY does not start it until the DSR
query is answered, and Glasshouse *is* the terminal for an embedded session.

All five orchestrator findings survived, explained by one cause, and the fix was
in the **test** — which had modelled an owner of a `SessionRuntime` that cannot
exist, since every real one answers terminal queries on every pass.

### What a green Windows suite has been worth, measured

Two `session::api` tests were shown, by the same side-channel technique, to hold
their assertions **while the child never started and produced not one byte**.
42 further tests are `#[cfg(unix)]`. Interrupt delivery to a real Windows child,
resize reaching one, and session resume are proven by nothing.

Sharpest of all: `an_embedded_session_answers_the_cursor_position_query_itself`
is `#[cfg(unix)]` — the DSR mechanism is tested only where answering is
**optional** and not where it is the difference between a session and a hang.

### The lead refused to close a new box by breaking a shipped one

`session/store.rs` may not name `crate::harness` — Phase 6 line 294, a
**checked** box guarded by a source scan. The packet's central instruction was
to store `PairingClass` there. Its first implementation did, that guard failed,
and it redesigned rather than weakening the guard: the store has its own
vocabulary and `session/mod.rs` holds three total conversions.

**And the constraint turned out to be right for a reason neither of us had.** A
stored vocabulary and a live one have different lifetimes: a row written last
month must stay readable when `PairingClass` gains a seventh variant, and two
types with an exhaustive function between them make that a compile error at the
one place someone has to decide what it means on disk. A shared enum would have
made it a silent constraint violation on a background write.

### The packet's line numbers were stale, and that is the orchestrator's defect

It cited nine box lines by number; the map had moved twice — partly from this
same session's own tick edits — and the offsets were not uniform. The worker
used `scripts/discover.py --phase 10` and lost nothing.

**Cite a box by number *and* by text.** A packet that names a line only by
number is one a worker cannot check, and this orchestrator has now shipped that
defect twice in one day.

### Subcontracting declined, with the reasoning

Risk routing puts project/session isolation, migrations and durable state in
Red, which is substantially the whole package; the one Amber slice (the CLI
surface) prints exactly the columns the migration decides, so splitting it would
have been two workers on one design. ~50 minutes wall clock, one Opus context,
2,019 lines. Worth having as a data point against the team-lead batches: at this
size, one specialist beat a lead-plus-subcontractor on overhead.

### §40 violated by a worker, and reported

It found one of its own backgrounded `cargo test` runs still going while it
started the gate, killed it, and **declined to quote any timing from that
window**. Nothing failed during the overlap. Reporting a contaminated
measurement as contaminated is the behaviour the rule is for.

## Batch 29 — four concurrent workers, and the first round dispatched by a successor

Four workers in parallel, started together on 2026-08-27 from `a24fee1`, by an
incoming orchestrator that inherited the round already written. **This is the
first round in this project's history that was prepared by one orchestrator and
dispatched by another**, which is practice §55 working as intended — the
predecessor stopped at its context ceiling rather than start a round it could
not integrate.

| worker | tier | kind | partition |
|---|---|---|---|
| `typing-throttle` | Opus specialist | defect | `tui/**`, `tests/terminal_loss.rs` |
| `windows-truth` | Sonnet | defect | `scripts/`, `session/api.rs`, `tests/pty_smoke.rs` |
| `phase-10a` | Opus lead | forward, 13 boxes | `session/{runtime,lifecycle,native_id,store,mod}.rs` |
| `phase-9a-facts` | Sonnet | forward, 2 boxes | `profile/**`, `launch.rs`, `shell/mod.rs`, `main.rs` |

Four is one above §9's stated ceiling of three, where "reviews start to
collide". It is being tried deliberately: two of the four are defect packages
that tick no box and therefore need no ledger or map edit at integration, which
is where a review's serial cost actually lands. **The question this round asks
is whether the ceiling is really about worker count or about how many
integrations need records written** — record the answer at integration.

### What a successor pays, measured

Dispatch cost the incoming orchestrator **~6% of context** from cold start to
four workers running and watched, including verifying the inherited state rather
than believing it. The predecessor's `ROUND-BRIEF.md` is what made that possible:
the packets, the worktrees and the validator run were all already done.

Against that, the orchestrator's own reading list is ~175k tokens. **The brief
is worth more than the reading list for the first hour**, because nothing in the
ten documents tells you which four packets are ready.

### Two packet defects in the first twenty minutes, and only one was the predecessor's

**1. The dispatch template pointed at a path that cannot resolve.** The brief's
worker instruction was *"Read `.agent-runtime/packet-<name>.md`"* — but
`/.agent-runtime/` is gitignored and exists **only in the main checkout**. None
of the four worktrees has it. A relative path there finds nothing, and the
report would have been written where no watch was looking, which is §57's
write-with-no-read arriving from a third direction. Caught before dispatch by
checking the directory existed in a worktree rather than assuming; every worker
got the absolute path plus one sentence redirecting every `.agent-runtime/` path
in its packet.

**2. `packet-phase-10a.md` named the wrong file for the schema migration.** It
said the convention lived in `src/session/store.rs` and that "migration 7 is the
most recent". Migrations live **only** in `crates/glasshouse/src/database.rs`,
`store.rs` contains none at all, and `SUPPORTED_SCHEMA_VERSION` is **8**.

The worker found this, and what it did with it is the point: it did not edit the
file it needed, because the file was in neither its `YOURS` nor its `FORBIDDEN`
list and its instruction was not to touch what the packet did not grant. It
stopped and asked, with the three options and its recommendation.

**That is the sixth consecutive round in which a worker was right against its
packet**, and the first in which the correction was about *which file a
convention lives in* rather than about a claim's substance. The orchestrator
verified it independently before granting — `grep -rln 'MIGRATIONS'` returns one
file, and all four packets were checked for a `database.rs` claim before it was
handed over.

**The transferable rule: a packet that names a file as the home of a convention
should name the symbol too.** `MIGRATIONS` would have been greppable and the
staleness self-evident; "the migration numbering convention is in it" is a claim
a worker can only take on trust or stop over.

### The `FORBIDDEN`/`YOURS` gap is a third category, and it cost a stop

`database.rs` was in neither list. The worker read that correctly as "not
granted" and stopped — the safe reading, and the one the packet's own stop
condition asks for. But it cost a round trip that a complete partition would not
have.

`validate_round.py` checks that the `YOURS` lists are **disjoint**. It cannot
check that they are **sufficient**, because nothing tells it what a package will
need to touch. That is §32 and §36 restated as a tooling gap: the round gate
proves no two workers collide and proves nothing about whether any of them can
finish. Worth building — for each packet, grep the box lines for the symbols
they imply and report any that resolve to a file in no list at all.

### Open questions for this round

1. **Does four concurrent workers actually collide at review**, or was §9's
   ceiling of three really a statement about how many *box-closing* packages one
   orchestrator can record? Two of these four tick no boxes.
2. **Does a stale packet cost more than a thin one?** Two of the three defects
   found so far were stale facts stated confidently, not facts omitted. A packet
   that says "find the migration convention yourself" is cheaper to write and
   might be cheaper to run.
3. **What did the two defect packages cost against what they returned?** Neither
   moves the progress number at all, and both were ranked above a phase.

### Batch 29's defect classes — four defects, four different failure modes

The round produced four process defects in its first hour, none of them in
product code, and they are worth separating because **they do not cost the
same and they are not prevented by the same thing.** Split at the outgoing
orchestrator's suggestion, which was right: a stale fact and an unchecked fact
look identical in a packet and are caught by different habits.

| # | defect | class | who caught it | cost |
|---|---|---|---|---|
| 1 | packet named `session/store.rs` as the home of the migration convention; migrations live only in `database.rs` | **asserted, never checked** — inferred from the handoff's prose without opening the file | the worker, before editing | one stop, one round trip |
| 2 | "migration 7 is the most recent"; `SUPPORTED_SCHEMA_VERSION` is 8 | **stale** — true when written | the worker, same turn | folded into (1) |
| 3 | dispatch template pointed at `.agent-runtime/packet-<name>.md`, a gitignored path absent from every worktree | **moved** — the rule changed and the second place that had to know was not told | the incoming orchestrator, before dispatch | none — caught pre-flight |
| 4 | orchestrator told the peer its results file held an unredacted account id; the file had redacted it at write time | **asserted, never checked** — verified that NVIDIA echoes the id, then claimed something about *where it was written* | the peer, from its own file | one correction |

**(1) and (4) are the same class and the class is the expensive one.** Both
began with a true fact — a migration landed recently; NVIDIA echoes an account
identifier — and ended in a false claim *adjacent* to it that nobody checked
because the neighbouring fact was solid. §39 already names this ("I verified the
fact and not the recommendation") and it has now happened twice more in one
round, once in a packet and once in a message to a peer.

**Two instances, one round, mode recorded — that is not a rate and must not be
read as one.** The outgoing orchestrator pushed back on a first version of this
paragraph that called the pattern "not incidental" on the strength of n=2, and
was right to: this project's own §34 and §60 say a count of one or two measures
nothing about a tail, and there is no reason process defects obey a different
arithmetic than flaky tests do. The instrument to settle it now exists — defects
typed by mode, per round — so **Batch 30–32 answer this, and until they do the
honest entry is the count.** Recorded here rather than resolved, which is the
whole point of a ledger.

**The cheap prevention is different for each class:**

- **asserted-never-checked** → name the *symbol*, not just the file. A packet
  saying "the convention is in `store.rs`" can only be trusted or stopped over;
  one saying `MIGRATIONS` is greppable in five seconds. Applies to messages too:
  quote the line you are claiming exists.
- **stale** → cite by number *and* text, which batch 28 already established, and
  let `discover.py` reprint both.
- **moved** → the round validator proves `YOURS` lists are disjoint and cannot
  prove a path resolves from where the worker stands. A packet path is not
  checked by anything today.

**What this says about who catches what.** Three of the four were caught by
somebody other than their author, and the two most expensive were caught by a
**worker refusing to act on its packet** rather than by any gate. That is the
sixth consecutive round in which a worker was right against its brief, and it is
the strongest argument in this ledger for the stop-condition wording — a worker
told to report rather than choose caught the defect that a worker told to use
its judgement would have absorbed silently by editing `database.rs` anyway.

**Note the asymmetry in visibility.** Defect 3 cost nothing because it was
caught before dispatch; defect 1 cost a stop because it was caught after. Same
class of error, two orders of magnitude apart in cost, and the only difference
is whether anyone looked before the work started. That is the argument for the
pre-dispatch check, not the post-hoc one.

### What the round gate should grow next: symbol existence, not just path existence

`validate_round.py` proves the `YOURS` lists are disjoint, that every path in
them exists or is marked `(new)`, and that every quoted box line matches the map
verbatim. It caught two real collisions the day it was written and it passed
this round's four packets cleanly.

**It could not have caught defect 1, and defect 1 was the expensive one.** The
packet said the migration convention lived in `session/store.rs`. That path
exists, so the gate was satisfied; the *claim about what is inside it* was false
and nothing checked it.

The fix follows from the prevention this round arrived at. If a packet must
**name the symbol** rather than only the file, then the symbol is a string a
script can look for:

    packet says:  the migration convention is in `database.rs` (`MIGRATIONS`)
    gate checks:  grep -q 'MIGRATIONS' crates/glasshouse/src/database.rs

That is a one-line check per claim and it converts the whole
asserted-never-checked class from "caught by a worker stopping" into "caught
before dispatch" — which is exactly the zero-cost-versus-one-stop gap the defect
table already measures between defects 3 and 1.

**Deliberately not built mid-round.** `scripts/tests/test_round_tools.py` is in
the gate and covers `validate_round.py`, so this is a change plus its tests, not
a one-liner — and practice §1 says the orchestrator's hands stay off code while
workers run. It is a well-specified packet for a Sonnet, and it belongs to
whoever runs Batch 30: own `scripts/validate_round.py` and
`scripts/tests/test_round_tools.py` together, add a `SYMBOLS` or annotated-path
syntax, and prove it by running the gate against **this round's own
`packet-phase-10a.md`**, which must fail on `store.rs`.

A gate that has never been run against a packet known to be broken is §20's
question waiting to be asked. This round supplies the broken packet.

## Batch 29 outcomes — four workers, three integrated, one parked on a platform divergence

| worker | tier | cost | outcome |
|---|---|---|---|
| `typing-throttle` | Opus specialist | ~$6.5, 52 min | **integrated** — 0 boxes, the round's most user-visible fix |
| `windows-truth` | Sonnet | ~$11.4, 48 min | **integrated** — 0 boxes, 3 of 4 items + 1 disproved |
| `phase-9a-facts` | Sonnet | ~$13.8, 41 min | **integrated** — 2 boxes (441 → 443) |
| `phase-10a` | Opus lead | ~$37, 1h43 | **parked** — 11 boxes built, 0 ticked |

**Four disjoint partitions produced zero merge conflicts**, against a `main` that
had moved four commits ahead of every worker's branch point. `git apply -3`
applied all four cleanly. The partition discipline works at four; that question
is answered.

### The round's question was wrong, and the real ceiling was elsewhere

It was opened asking whether four concurrent workers collide **at review**, on
the theory that §9's ceiling of three is really about how many box-closing
packages one orchestrator can record. **Nothing collided at review.** All four
reports were read and verdicted without contention.

**Integration is where it bound, and for a reason no one had written down: one
batch's platform failure stalls the whole combined tree.** The gate runs once on
everything, so `phase-10a`'s Linux regression made a single FAIL that said
nothing about which of four batches caused it. Separating that cost a second
full gate run — and the separation only worked because the partitions were
disjoint enough to reverse one batch cleanly.

**The transferable rule: partition disjointness is not only a concurrency
property, it is a *bisection* property.** Four batches that cannot be applied and
reversed independently would have left one red gate and no cheap way to attribute
it. Keep `git apply -R` reversibility in mind when sizing a round, not just
"two workers must not edit one file".

### The parked batch is the round's most valuable measurement

`phase-10a` was the largest package, the most expensive worker, produced the
best report, and **ticked nothing.** Its work is intact and its follow-up is
cheap. What stopped it:

    events_lifecycle.rs on one tree, one gate run
      macOS   5 passed, 0 failed
      Linux   3 passed, 2 failed

A new readiness bound whose outcome is a race, landing on opposite sides on the
two platforms — so whether a session *exists* depends on the operating system —
and breaking `capability-map.md:1730`, a **ticked** box.

**Three things this says, none of which is "the worker did badly":**

1. **The worker predicted it.** Its report said the bound was best-effort, that
   it "does not always win", and that Linux and Windows were unrun and that
   *"matters more here than usual"*. It was right, and it said so before anyone
   asked. A report that names its own weakest claim is worth more than one that
   does not, and this is the second round running where the most valuable
   paragraph was a worker's own caveat.
2. **A one-platform gate is not evidence for a cross-platform claim**, and this
   is the first time in this project that a *local* Linux leg — not a CI runner —
   caught a regression before it shipped. §27 built that leg as a substitute for
   billed CI; it has now paid for itself in a way CI could not have, because CI
   would have caught it *after* the push.
3. **Cost per box is the wrong denominator for a parked batch.** $37 bought
   eleven implemented boxes, two real pre-existing defects found
   (`SessionStore::close`'s out-of-transaction liveness read, and
   `answer_terminal_queries` as a second writer), seventeen killed mutations, and
   a platform-divergence proof the follow-up now inherits. Recording it as
   "$37, zero boxes" would be arithmetically true and analytically useless.

### The `asserted-never-checked` count is now five, and one was caught by tallying

Two more instances since the entry above, both in reports from workers whose
underlying work was sound:

| # | verified thing | unchecked claim beside it | caught by |
|---|---|---|---|
| 5 | 48 gates read and classified into a table | the **summary** of that table | tallying the table mechanically |
| 6 | no FORBIDDEN file touched | no file **outside `YOURS`** touched | `git status` |

Number 5 is the instructive one: the report's headline said 15/9/23/1 and its own
table says 12/10/25/1. **Both total 48**, which is exactly why it survived being
read — a wrong breakdown that reconciles to the right total is the hardest kind
to catch by eye, and it would have become the handoff's number, replacing one
unverified count with another in the very package commissioned to end that.

Still **not a rate** — six instances, one round, no denominator for how many
claims were made in total. Batches 30–32 still owe the answer. But the prevention
has now been validated twice: **check the artefact, not its description.** Both
catches cost one command.

## Batch 30 — five workers, 492 → 514, recorded after the fact

Integrated at `60f8c9f` by the orchestrator that ran it; the entry is written by
its successor from the commit and the reports, so treat the per-worker costs as
unrecorded rather than zero.

| worker | tier | boxes | closed |
|---|---|---|---|
| `phase-9k` | Sonnet | 16 | 6 — **zero production code**, all six argued from the existing tree |
| `phase-42` | Sonnet | 13 | 10 — the Unix-socket control API |
| `phase-41` | Sonnet | 15 | 9 — the project overview; **all six opens want Phase 32B** |
| `phase-32a` | Opus | 21 | 3 — **fifteen of its eighteen opens wait on Phase 32B** |
| `phase-35` | Sonnet | 14 | **0** |

**`phase-35` closed nothing and its orchestrator recorded why against itself:**
its partition was `routing/**` while every production entry point — `main.rs`,
`cli.rs`, `shell/**` — belonged to someone else. That is §32, *"put the caller's
file in the partition"*, which this project first recorded from batch 13–14.

## Batch 31 — two workers, sized by a seam query rather than a box count

Dispatched 2026-08-27 ~16:2x from `60f8c9f`, by the successor orchestrator.

| worker | tier | effort | boxes in play | worktree |
|---|---|---|---|---|
| `phase-32b` | Opus | high | 33 | `glasshouse-phase-32b` |
| `phase-47` | Sonnet | high | 6 | `glasshouse-phase-47` |

### §32 has now bitten three times, and the third was invisible

Before sizing either packet the successor ran one command:

    $ python3 scripts/discover.py --seam ResourceRegistry
    ZERO non-test call sites of `ResourceRegistry` in crates/**/src/**.

`main.rs` contains no reference to the registry or to quota at all. **Phase 32 and
Phase 32A — roughly 2,000 lines across `provider/registry.rs` and
`provider/quota.rs` — are reachable from nothing in the shipped binary.**

That is the same defect as `phase-35`'s and `lead-extract`'s, and it is the
**hardest of the three to see**, because the phase does not look stranded:
Phase 32 reads **11 of 12** and only its *first* line — "create a registry" — is
open. A phase that is 92% ticked with a dead centre does not advertise itself.

**The measurement: three occurrences, and the detector costs one command.**

| # | batch | phase | how it was found | when |
|---|---|---|---|---|
| 1 | 18–19 | `lead-extract`, memory | after integration — 0 of 25 | too late |
| 2 | 30 | `phase-35`, routing | after integration — 0 of 14 | too late |
| 3 | 31 | Phase 32 / 32A, provider | **before dispatch**, by `discover.py --seam` | in time |

**So the rule earns a mechanical form.** §32 currently says *find where each
capability's production caller will live*, which is a judgement. The cheap version
is: **before sizing a package, run `discover.py --seam <the phase's central type>`.
If it answers zero, the package's first deliverable is the caller, and the
partition must contain the file the caller lives in — or the round is already
lost.** Both packets this round were sized that way; `packet-phase-32b.md` owns
`cli.rs` and `main.rs` for no other reason.

### `validate_round.py` was mutation-tested rather than trusted

§20 says apply mutation discipline to gates. The round validator passed on both
packets, which is exactly the state open question 6 says to suspect. One quoted box
line was corrupted (`percentage` → `rate`):

    REFUSED — [box-lines-match-map] packet-phase-47.md:41 quotes `…error rate.`
    which does not match docs/product/capability-map.md:1763 verbatim

Restored from a `cp` backup, byte-identical, and it passed again. **The gate
bites.** Cost: about ninety seconds. This is the third gate in this project
verified this way and the first that was already alive.

### Questions this round is opened with

1. **Does the "seam query before sizing" detector generalise?** It caught the
   third instance in time. Run it on the next round's phases *before* dispatch and
   record whether it changed the partition. One command per phase.
2. **Is a 33-box package too large for one Opus worker, when 16 of the 33 are
   another phase's boxes that only need a reading taken?** Batch 20–21 found
   partition width, not box count, was the binding constraint; this tests the
   claim from the other side.
3. **Does Sonnet at `high` effort close a 6-box package that is mostly negative
   requirements?** Three of `phase-47`'s six are *absence* claims — no spend
   totals, no non-optional diagnostics, no animation — and §17 says this project
   has already shipped one absence test that passed for the wrong reason.

## Batch 31 outcomes — three workers, 25 boxes, and open question 1 answered

Integrated as `8b4c982` (514 → 539) with `c25448b` behind it. Local gate 13/13 on
the tree that carries all three.

| worker | tier | effort | wall | cost | in play | closed | $/box |
|---|---|---|---|---|---|---|---|
| `phase-32b` | Opus | high | 50m49s | $25.48 | 33 | **16** | **$1.59** |
| `phase-47` | Sonnet | high | 21m41s | $6.06 | 6 | **4** | **$1.52** |
| `phase-46` | Sonnet | high | 19m01s | $7.94 | 8 | **5** | **$1.59** |

### Open question 1 — answered, and the answer is "yes, at this size"

> *Does the Sonnet tier close boxes at Opus's rate on amber work? If cost per box
> is comparable, red-risk routing is the only reason to spend Opus.*

**Cost per box was $1.52–$1.59 across both tiers, in one round, on the same day.**
Three packages, two tiers, a fifteen-fold spread in package size, and the three
numbers land within four cents of each other.

**Read it with three caveats, none of which dissolves it.**

1. **The packages were not equally hard.** Opus took 33 boxes across a partition
   spanning `provider/**`, `cli.rs`, `main.rs` and `config/mod.rs`, and closed
   48%. The Sonnets took 6 and 8 in single-module partitions and closed 67% and
   63%. Equal cost per box at unequal difficulty is *better* for Sonnet than the
   raw number says, not worse.
2. **Sonnet was run at `high` effort deliberately**, on the user's standing
   instruction that it handles substantial chunks there. This is not a datum about
   Sonnet at default effort.
3. **It is one round.** Three points, one day, one orchestrator. Question 1 stays
   open for a second round; what it no longer needs is a *first* measurement.

**What it changes operationally:** size a Sonnet package up rather than splitting
it, and spend Opus on partition width and red risk rather than on box count. The
Opus package here earned its tier on the first count — its partition crossed four
modules and it had to settle a classification the map fixes architecturally — not
because 33 boxes are beyond Sonnet.

### The seam query paid on its first use

Batch 31's entry above predicted it would. `discover.py --seam ResourceRegistry`
answered ZERO before dispatch, the partition was widened to include `cli.rs` and
`main.rs`, and `glasshouse resources` — the caller that query demanded — is what
carried 16 boxes. **Without it the package was `phase-35` again: good work,
zero closures.**

Cost of the detector: one command. Recommend it as a standing pre-dispatch step
rather than an optional one.

### The Windows leg caught the PREVIOUS round's defect, not this one

`--windows-vm` failed with four dead-code errors under `-D warnings` — and
`msrv (windows)` passed on the same run, which is the tell that the tree really
was replaced and this was a compile error rather than the stranded-VM-process
failure that reports identically as `FAIL build`.

**Attributed, not assumed:** `api/` was introduced by `60f8c9f`, the round before;
this round's three packages touched it zero times. It shipped because
`--windows-vm` was not run after that commit.

**So the rule this buys: run `--windows-vm` on every round that lands, not on
every round that feels risky.** The local gate was 13/13 on `60f8c9f` and 13/13
here; neither says anything about Windows. That is now the **fifth** time a
Windows-only defect survived a green local gate in this project.

Fixed without a second VM round trip using §18's cfg flip, proved in both
directions — the pre-fix file flipped to the Windows shape reproduces the
identical four errors.

### Workers were right against their packets for the sixth and seventh time

- `phase-47` — the packet named `EventLog::recent_for_session` as line 1758's
  seam. Wrong: `EventLog`'s query methods are never called from `shell/**`; the
  real seam is `ShellState::activity`, populated in production at
  `shell/mod.rs:445`. The orchestrator checked that the methods *existed* and not
  that anything *called* them, which is §36's rule, and one grep would have caught
  it.
- `phase-32b` — asked the orchestrator to decide line 1229 rather than deciding
  it, and refused to reverse a design decision it disagreed with. Both correct.

**And two corrections belong to the orchestrator rather than to any worker**, both
recorded in `probe-quota-headers-2026-08-27.md`: design note D2 overreached (a
response *header* is not the *payload*), and "H1 is dead" generalised six sampled
hosts to a population — the worker's wider unauthenticated probe found the
counterexample the orchestrator's narrower authenticated one had missed.

### `asserted-never-checked` count: still six

No new instances this round. Every claim spot-checked in the three reports
reconciled against the artefact: the AnyRouter headers reproduced, the `§36`
zero-call-site result reproduced, the mutation ledger's §35 entries verified by an
independent orchestrator mutation that was killed by a binary-level test.

### A Windows flake, attributed by §40's stronger test rather than assumed

After the cfg fix, two `--windows-vm` runs on the **identical tree**:

    run 2   1240 passed, 1 failed   session::api::tests::
                                    interrupting_through_the_api_is_recorded_as_machine_initiated
    run 3   1241 passed, 0 failed   PASS build / PASS test / PASS msrv

**Same tree, two runs, two answers — which is the proof of nondeterminism**, and
§40's addendum says to prefer it over the `main`-comparison because it needs no
assumption that the two trees are otherwise comparable.

Attribution, established before the re-run rather than after: batch 31 touched
`session/api.rs` **zero** times, `60f8c9f` touched **no** `session/` files at all,
and the file's last change was `d35fe6a` — the commit that *hardened* these tests
after finding they passed against a child that never started. So the code under
test is unchanged since the last green Windows run, and this is not a regression
from anything in this round or the one before it.

**Reported as a rate, not a pass (§60): 1 failure in 2 observations.** That is a
high rate for a test nobody owns, and it is an *interrupt* test spawning a real
Windows child — precisely the area the handoff already records as "proven by
nothing". It joins the standing flake debt beside the 1-in-37 `pty_smoke`
`SIGABRT` rather than being waved through because a re-run was green.

**The trap this avoided:** run 3 alone would have read as "the fix worked, Windows
is green". It did work — `build` went from four dead-code errors to PASS — but a
single green run cannot distinguish a fixed defect from a flake that happened to
win its coin flip, which is the whole content of §60.

## The local clippy leg is weaker than the Linux one, and nothing said so

Batch 32's gate returned `PASS lint / clippy` and `FAIL lint (ubuntu) / clippy` on
the same tree, for one diagnostic:

    error: this block may be rewritten with the `?` operator
      --> crates/glasshouse/src/provider/telemetry.rs:599:12
      = note: `-D clippy::question-mark` implied by `-D warnings`
      = help: ...rust-clippy/rust-1.98.0/index.html#question_mark

**It is not a platform difference.** It is a toolchain-version difference:

    local (macOS)      clippy 0.1.96      <- `rustup toolchain list` has nothing newer
    container (Linux)  clippy 1.98.0      <- the container installs a current toolchain per run

The newer clippy has lints the older one does not emit. So **`PASS lint / clippy`
means "this machine's clippy passed", not "clippy passed"**, and the summary
presents the two legs as peers.

**Both workers ran clippy locally, both were clean, and both were right about what
they could see.** The lint was unreachable from where they stood. This is not a
worker-discipline failure and must not be written up as one.

### Why this is §20's shape for a third time

§20 asked *what change would make this gate fail?* and found two gates that could
not fail at all. This is the softer version: a gate that **can** fail, does fail
usefully, and has a sibling wearing the same name that is quietly weaker. A reader
of the summary sees seven `lint /` PASSes and two `(ubuntu)` PASSes and reasonably
concludes the tree is clean under clippy. On this machine, seven of those nine
prove less than they appear to.

Same family as the MSRV gate that resolved `rustc` from `PATH` (§20), the `chmod
000` test that ran as root (§27), and the `su -c` step whose quoting made its exit
status meaningless (§31): **the check ran, reported, and measured something other
than what its name claimed.**

### What to do about it, in increasing order of cost

1. **Read a green local `lint / clippy` as provisional** until the `(ubuntu)` leg
   agrees. It already runs in the same command; this costs nothing but a habit.
2. **Have `ci-local.sh` print both clippy versions in its summary** when they
   differ. One line, and it turns an invisible asymmetry into a visible one — the
   same move §54 made for `blind` versus a number.
3. Keeping the local toolchain current would close the gap, but that is the user's
   machine and their call, and it would not stop the gap reopening next quarter.

**The generalisable rule: when two legs of a gate share a name, check that they
share a version.** A gate's identity is its command *and* its toolchain, and only
one of those appears in the summary line.

## Batch 32 outcomes — and a correction to what batch 31 concluded

Integrated as one commit, 539 → 557, green on all three platforms (local 13/13,
`--windows-vm` 3/3).

| worker | tier | effort | wall | cost | in play | closed | $/box |
|---|---|---|---|---|---|---|---|
| `phase-35-classify` | Sonnet | high | 14m | $3.69 | 14 | **14** | **$0.26** |
| `quota-followup` | Sonnet | high | 50m | $25.56 | ~11 | **4** | **$6.39** |

### Cost per box is not a property of the tier. Batch 31 read it as one.

Batch 31 measured $1.52–$1.59 across three packages and two tiers and called it a
first answer to open question 1. **Batch 32 puts two packages at the same tier,
same effort, same day, twenty-five times apart.** So the batch-31 numbers agreeing
to four cents was a coincidence of package composition, not a signal about Sonnet.

**What cost per box actually measures is how much of the work was already done.**

- `phase-35-classify` closed fourteen boxes with **~20 lines**, because batch 30
  had already written **~717** into `routing/**` and closed zero for want of a
  caller. The cheap number is batch 30's cost showing up in batch 32's column.
- `quota-followup` wrote **~1,932 lines** for four boxes — genuinely new work — and
  then *honestly declined to close four more* it had built working readers for.

**Both effects push in the same direction and neither is about the model.** A
package that inherits finished work looks brilliant; a package that builds
foundations and refuses to overclaim looks expensive. Ranking workers on $/box
would reward the first and punish the second, which is exactly backwards — the
second one is the batch that made the first one possible.

**So: open question 1 goes back to open**, and the useful reformulation is not
"which tier is cheaper per box" but **"how much of this package is already
built?"** — which the seam query answers before dispatch, for free, and which
predicts the cost far better than the tier does.

The one operational claim from batch 31 that survives: **Sonnet at `high` handled
a four-file, 1,900-line package across `gateway/**` and `provider/**` with 19
mutations and no architectural mistakes.** That is a statement about capability at
that effort, and it is unaffected by the accounting.

### The finding worth more than either package's boxes

`quota-followup` built a working reader for Groq's rate-limit headers, proved it
produces a real `Percentage::Exact(99)` — **the first live percentage this product
has ever computed** — and left the box **open**, because nothing in the shipped
binary bridges a gateway-captured reading into the resource registry.

That is §5 and §36 applied by a worker to its own output, unprompted, against its
own interest. It is the third round running where the most valuable paragraph in a
report is the one explaining why a box stays open.

## Batch 33 — the zero-box package was the valuable one, and the numbers say the opposite

Integrated as `c40194e`, 557 → 565.

| worker | tier | wall | cost | in play | closed | $/box |
|---|---|---|---|---|---|---|
| `phase-48-cli` | Sonnet, high | 12m | $3.68 | 8 | **8** | $0.46 |
| `bridge-quota` | Sonnet, high | 34m | $10.75 | 4 (+2) | **0** | — |

**This is batch 32's lesson again, sharper.** `bridge-quota` cost $10.75 and closed
nothing, and it is the package this round will be remembered for: it killed the
packet's hypothesis with evidence, proved the process boundary is real, built the
durable store, located three wiring edits exactly, **and then established that the
wiring would not have closed the boxes anyway** because no host has both a shipped
template and both halves of a pool.

Any metric that ranks these two by boxes or by cost per box gets this round exactly
backwards. **Recording it here because the temptation to build that metric is
real** — it would be easy to compute and actively harmful.

### The orchestrator's packet was wrong about the partition, again

`packet-bridge-quota.md` named `shell/**` as a bridge candidate. One grep would
have shown `shell::run`'s quick-open always launches a native profile and never
resolves a gateway-backed one, so there was nothing live there to read. **That is
§36's rule, quoted in that very packet, broken by the person quoting it** — the
second time this session (the first was propagating a stale patch into
`packet-quota-followup.md` without checking it against the file).

**The pattern in both: I asserted a fact about the code from a report or from
memory instead of from the file.** The workers checked. The cheap prevention is
already written down — *check a declaration against the use, not the claim* — and
it applies to packets as much as to evidence.

### Windows caught this round's own work for the first time

Previous Windows finds were inherited (`60f8c9f`'s dead code). This time a test
written **this round** passed on macOS and failed on Windows: a new dispatch test
compared `doctor`'s output against an un-normalised fixture root while Windows
prints the canonical one.

**The fix deliberately does not depend on diagnosing the normalisation.** The
hypothesis was the 8.3 short form of `TEMP`, unconfirmed; §58 says a wrong cause
predicting the right symptom still produces a wrong fix. So the assertion stopped
comparing paths at all and identifies the project by name, which no normalisation
can touch — and the §35 property was re-verified afterwards by emptying the
dispatch arm.

**Five rounds of Windows runs, five findings.** Run it every round.

### The `session::api` flake now has a rate

    batch 31 run 2  FAIL      batch 33 first   PASS
    batch 31 run 3  PASS      batch 33 final   FAIL
    batch 32        PASS

**2 in 5, ~40%**, on an interrupt test spawning a real Windows child, on code
unchanged since `d35fe6a` and touched by none of this session's five packages. High
enough to deserve an owner. Recorded as a rate, not buried under a green re-run
(§60).

## Acking a false idle makes a live worker invisible to the heartbeat

Batch 34's worker armed **its own Monitor** on a background `cargo test` and then
sat waiting for it. `worker-watch.sh` read that as two false idles in a row — the
`Stop` hook fires at every turn boundary, and a worker waiting on its own job draws
no spinner. Each cost the orchestrator a turn to diagnose and re-arm.

**Then the fix made it worse.** Switching to a plain background wait on the report
file — the right signal, per §62 — meant no `worker-watch.sh` process was running
any more. `orchestrator-heartbeat.sh` asks exactly two questions (§47): *is a worker
waiting for review* (a marker in `.agent-runtime/idle/`) and *is one still running*
(a live `worker-watch.sh` process). Both answered no. Measured:

    pgrep -f worker-watch.sh   -> 0
    ls .agent-runtime/idle/    -> 0 markers
    the worker                 -> alive, "1 shell, 1 monitor still running"

So the heartbeat announced **"ORCHESTRATOR IDLE — nothing running"** with a worker
mid-mutation. Fixed here by re-arming a long-delayed `worker-watch.sh` purely so
the process exists for the heartbeat to see — a presence beacon, not a watch.

### Two transferable points

**1. A watch is load-bearing for something other than itself.** `worker-watch.sh`'s
*process* is the heartbeat's liveness signal, and nothing says so at either end.
Replacing a watch with a better watch silently removed a second mechanism's input.
**Before retiring a mechanism, ask what else reads it** — this project has now
built three write-with-no-read defects (§57) and this is the mirror: a read with an
undeclared writer.

**2. §28's unbuilt fix would have prevented the whole sequence.** It proposed:
*a pane whose worktree has grown since the last read is working, whatever its
screen shows.* `quota-live`'s worktree sat at exactly `+141/-45` across both false
idles — one `git -C <worktree> diff --shortstat` would have answered it, and the
same signal would give the heartbeat a liveness input that does not depend on
another watch being armed. **Proposed in §28, now wanted by two mechanisms.**

## Batch 34 — the five-package chain, and what the zero-box package bought

`7215d3d`, 565 → 569. `quota-live`: Sonnet high, ~$4, four boxes.

    32A              modelled capacity                                     3 boxes
    32B              the reader + `glasshouse resources`                  16 boxes
    QUOTA-FOLLOWUP   the gateway captures headers                          4 boxes
    BRIDGE-QUOTA     the durable store, and "a template is missing"        0 boxes
    QUOTA-LIVE       template + wiring, together                           4 boxes

**BRIDGE-QUOTA is why batch 34 worked.** Without its finding, this round would have
landed three `main.rs` edits into a path with nothing flowing through it, and 1217
would still be open with a live caller pointing at an empty cache. It cost $10.75,
closed nothing, and is the reason the next package cost $4 and closed four.

**Any ranking by boxes or by cost-per-box scores it last.** That is now recorded
twice in this file, because the metric is easy to build and would actively mislead.

### Four packages declined to tick 1217/1218, and all four were right

The property was structurally guaranteed from 32A onward. Every package could have
ticked it on the guarantee; none did, each saying in its own words that a guarantee
which has never fired in the shipped binary is not a closed box. It fired in batch
34 and ticked immediately.

**That is the evidence discipline paying a visible dividend rather than costing
one.** Had 32A ticked it, the four rounds since would have built a percentage
nobody was waiting for, and the box would have been a number that lied for a week.

### A weak mutation nearly produced a false finding, and §5 caught it

The orchestrator mutated Groq's base URL to an invented one; it **survived** —
apparently a hole in a test the worker had specifically claimed pins that URL.
§5 says a survivor is more often a weak mutation than a weak test, so the mutation
was inspected first: it had replaced the URL inside a **doc comment**, not the
value. Re-aimed at the real one, the test killed it at once.

**The wrong conclusion was one keystroke away** — a logged coverage gap that does
not exist, in the package whose entire value rests on that template being real.
The rule works; what makes it work is applying it before writing the finding down.

## Batch 35 — three concurrent Sonnets, and the same missing consumer twice

`7e1ccb8` / `994ba4f` / `2d8e569`, 569 → 604. Three Sonnet implementers at high
effort, started ten minutes apart, partitioned by file, `validate_round.py` run
before dispatch (clean, no collisions).

| | pairing-prior | mem-validity | phase-32d |
|---|---|---|---|
| boxes in packet | 12 | 24 | 20 |
| **boxes closed** | **1** | **22** | **12** |
| worker's own proposal | 0 | 22 | 19 |
| orchestrator's ruling | **+1** | agreed | **−7** |
| diff | +1741/−76 | +1759/−59 | +2555/−77 |
| worker cost | ~$7.4 | ~$14.2 | ~$14.9 |
| wall clock | 28 min | 38 min | 40 min |
| cost per box | $7.40 | $0.65 | $1.24 |

Total ~$36.5 for 35 boxes, ~$1.04 per box, three workers inside one 40-minute
window. Local gate **13/13** on the integrated tree, including the ubuntu clippy
leg.

### The finding: two packages built a mechanism nothing calls, for the same reason

`pairing-prior` left **eleven** Phase 9J lines open because
`native_pairing_prior_contribution` has no production caller. `phase-32d` had
**seven** Phase 32F lines refused for the identical reason —
`evaluate_reserve_spend` is called only from `tests/capacity_score.rs`. Add map
line 1293 and Phase 9J line 569 and that is **eighteen boxes in one round whose
sole obstacle is that Glasshouse has no component that ranks candidates and
decides.**

Neither worker was wrong to build its half; both were explicitly asked to. But
**the scheduling lesson is that a producer whose consumer does not exist is a
package that cannot close its boxes however well it is executed** — and this
round bought that lesson twice at full price. The seam query catches a *dead*
centre (§32); it does not catch a *missing* consumer, because the consumer is
not a symbol you can grep for yet.

**Cheap check to add before sizing any policy package:** name the function that
would call the thing you are about to build, and find it. If you cannot, the
package's boxes will not close and its real deliverable is the seam plus the
report.

### `discover.py --seam` produced a wrong verdict, and it changed a tick

    discover.py: 3 non-test call site(s) of `evaluate_reserve_spend`
    A box that depends on this seam can close: it has a production caller.

All three were inside `quota.rs`: two intra-doc links and the definition itself.
The verdict line counts matches, and a symbol's own definition and doc comments
match. §49 already says a match is a lead rather than proof; this is the first
recorded case of that warning changing an outcome. **The tool is still worth
running — it was right about `NormalizedCapacity` and about `CmuxPresentation`'s
zero — but its verdict sentence should be read as "look here", never as "yes".**

Worth a small fix when someone owns `scripts/`: exclude the definition line and
`///` comment lines from the count before printing the verdict.

### Review found something real in all three, and none was a gate failure

Every worker's gates were accurate. All three findings came from reading what
the mechanism connects to — the same place batch 13–14's findings came from.

1. **`pairing-prior`** — an unparseable config value silently reported *"from the
   default — nothing configured"*. The worker cited the module's own
   visible-degradation rule and then did not implement it; every sibling field
   prints a bad value back (`behaviour=nonsense`). Found by running the binary
   with a deliberately invalid value, which the worker's own transcript had not
   done.
2. **`mem-validity`** — the over-fetch that lets decay *promote* a result was
   called load-bearing in the worker's own report and had **no test**. Reducing
   `overfetch_limit` to the identity left all 1750 tests green, because every
   test in the file used a corpus smaller than its own `limit`. Found by
   mutating the thing the report called important rather than the things it
   listed as proven.
3. **`phase-32d`** — the seven-box override above.

**The transferable rule: mutate what the report calls load-bearing, not what it
lists as proven.** A worker's mutation table covers what it thought to test; the
sentence where it explains why something matters is where the untested claim
lives.

### Two workers policed themselves, and that is worth as much as the boxes

`pairing-prior` caught its own **vacuous** test: its first draft compared
candidates at 20 observations, where the native prior has already decayed to
zero, so deleting the entire evidence signal would have left the comparison
green. It noticed, lowered the count to 5, and re-ran.

`mem-validity` reported a **surviving** mutation rather than claiming
five-for-five, diagnosed it correctly as redundant defence (an early return
*and* an infinite half-life both protect invariants), removed both layers, and
watched a pre-existing test it had not written catch it.

Neither was asked for in the packet beyond the standing §41 instruction. Both
are the behaviour the rule exists to produce, arriving without supervision.

### Open question 1 gets more data, and the answer is holding

*"Does the Sonnet tier close boxes at Opus's rate on amber work?"* Three Sonnets,
35 boxes, ~$1.04 per box, no red-risk escalation needed and no architecture
disputed. The spread within the tier ($0.65 to $7.40) is again **how much was
already built**, not the model — `mem-validity` extended a settled schema and
ranker; `pairing-prior` wrote a subsystem from nothing and then honestly
declined to tick it. That is batch 31's correction confirmed a second time:
**cost per box measures the package, not the tier.**

### Sizing note: 20–24 boxes per Sonnet packet was right

All three ran 28–40 minutes — inside §1's 20–40 minute target, on the first
attempt, with packets of 12/24/20 boxes. Three returns did **not** collide
because the reviews were serial and the workers finished 7 minutes apart.
The ten-minute stagger did its job.

### Attributing a red Windows gate cost four runs, and the answer was "not you"

Batch 35's Windows gate failed twice on
`interrupting_through_the_api_is_recorded_as_machine_initiated`. The handoff
already called it a standing flake, so the cheap move was to accept that. Two
things argued against accepting it: the batch had failed **2 of 2**, and its lib
suite appeared **10× slower** (4.86s → 48.16s), which reads exactly like a
performance regression.

So a worktree was cut at the pre-batch commit and the Windows suite run against
it three times.

| tree | runs | fails |
|---|---|---|
| `9f60f07` pre-batch | pass 4.86s, pass 4.78s, FAIL 47.99s | 1 of 3 |
| `2d8e569` batch 35 | FAIL 48.16s, FAIL 48.01s | 2 of 2 |
| earlier checkpoints | — | 2 of 6 |

**Three in nine on unchanged code — 33%.** Batch 35 exonerated.

**The 10× slowdown was not a second symptom; it was the same one.** A pass costs
~5s and a failure ~48s, because the test waits out a 45-second deadline. Reading
suite wall-clock as an independent signal would have produced a confident, wrong
finding about performance — §58's shape exactly: *a wrong cause that predicts the
right symptom*. The tell was that 48s ≈ the deadline, which is one subtraction
and was nearly not done.

**What this cost, and the rule it buys.** Four Windows runs, about forty minutes,
to conclude "not you." That is the price of attribution on a 33% flake, and every
future orchestrator pays it again unless the flake gets an owner. **A flake left
unowned is not free; its cost is one attribution per batch, paid by whoever is
holding the gate.** Record the rate when you measure it — this entry exists so
the next orchestrator can skip the four runs and go straight to owning it.

**Method note worth keeping.** `glasshouse-windows-ci` honours
`GLASSHOUSE_CI_REPO`, so a detached worktree at any commit can be run against
the same VM. Comparing a suspect tree against its own base is three commands and
is the only thing that separates "your batch" from "this machine".

## Batch 36 — the consumer round, and two packets whose premises were wrong

`044496b` / `87897d2` / (routing-score), 604 → 631. Three Sonnets at high effort,
partitioned by file, `validate_round.py` clean before dispatch.

| | orchestrator-role | routing-ledger | routing-score |
|---|---|---|---|
| boxes in packet | 18 | 15 | 25 |
| **closed** | **14** | **6** | **7** |
| diff | +1222/−31 | +2674/−149 | +1163/−64 |
| cost | ~$5.8 | ~$18.8 | ~$11.6 |
| wall clock | 20 min | 36 min | 39 min |

### Two of three packets rested on a premise that was wrong, and both workers killed it

This is the round's headline and it is about **packet-writing**, not worker
quality. Both hypotheses were labelled killable (§44) and both were killed with
structural evidence rather than a shrug:

1. **`routing-ledger`** — the packet claimed `gateway/session.rs` sees enough of
   a turn to record a real observation. It does not: `gateway::ingress` is
   *deliberately incapable of reading a response body*, so first-byte,
   first-token, tokens, cost and tool rounds are unreachable **by design, not by
   omission**. Four boxes stay open on that alone.
2. **`routing-score`** — the packet listed the pairing prior as ready to wire.
   `PairingQuery::harness` is a required `IntegrationId`, all ten variants are
   third-party harnesses a user launches, and a disposable job is *Glasshouse's
   own internal call*. Three boxes stay open, and **Phase 9J's eleven need a
   different caller entirely** (`InteractiveRouting`).

**The orchestrator's own batch-35 finding was half wrong because of this.**
"Eighteen boxes wait on one missing consumer" folded together two different
missing consumers. The reserve half was right; the pairing half needed the
gateway session, not the disposable router. **A finding that names one cause for
boxes in two different files deserves one more check before it becomes a plan.**

### The cheap check that would have caught both

Both failures are the same shape: *the input the packet promised cannot be
constructed at the caller the packet chose.* Neither needed a seam query — they
needed one look at the **type the input requires** and one at **what the caller
has**. `PairingQuery::harness` is a required field; `DisposableCandidate` has no
harness. That is two greps.

**Add to packet-writing: for each input you claim is ready to wire, name the type
that produces it and the field on the caller that feeds it.** If you cannot name
both, the claim is a hypothesis and should be labelled as one.

### Three review findings, and two of them were the integrator's own work

- **`orchestrator-role`**: box 2 closed on an audit. The audit was correct — and
  an audit protects nothing against the next edit, so a structural guard was
  added in this project's existing pattern, with a CRLF twin per §14.
- **The integrator's `main.rs` wiring was a defect.** `EvidenceLedger::open(runtime)?`
  ran on every launch whether or not a gateway was needed, and `?` turned a
  telemetry failure into a failed session. Fixed to warn and continue.
- **The integrator's wiring was then invisible to the tests.** Removing the
  ledger from both call sites left the whole suite green. A source-scanning guard
  now fails on exactly that edit. **Both of these were found by mutating my own
  change with the same discipline applied to workers' changes** — which is the
  only reason they were found at all.

### `discover.py`'s fix paid for itself within the hour

The definition-vs-caller split shipped in `5db74b2` was used immediately to size
this round: it cleanly separated four real `InteractiveRouting` callers from six
doc mentions, which is how `main.rs:1120`'s live `DisposableRouting` decision was
found. **A tool fix that changes a tick once tends to change sizing every round
after.**

## Batch 37 — one worker, and the first round the Phase −1 gate shaped

`bdb349f`, 631 → 644. **Half the map.** One Sonnet at high effort, ~$13,
37 minutes, +904/−90, **thirteen boxes**.

| | gateway-evidence |
|---|---|
| boxes closed | 13 of ~17 |
| cost per box | **~$1.00** |
| mutations | 5 attempted, 5 killed, 0 survived |
| compute split (self-reported) | 60% implementation / 25% verification / 15% report |

### The gate's first use refused two packets before either reached a worker

Six commands, against the ~$30 the same class of mistake cost in batch 36.
**One of the two it refused was the "cheapest next win" the previous handoff had
recommended** — wiring the ledger reader into `DisposableRouting`. That
recommendation was written by an orchestrator that had just spent a whole round
learning the same lesson, and it was still wrong, because a recommendation is
not a feasibility argument. The gate is the difference.

### Cost per box, three rounds

| batch | workers | boxes | worker cost | per box |
|---|---|---|---|---|
| 35 | 3 | 35 | ~$36.5 | $1.04 |
| 36 | 3 | 27 | ~$36.2 | $1.34 |
| 37 | 1 | 13 | ~$13.0 | **$1.00** |

Batch 37 is the cheapest per box **and** ran one worker rather than three. The
saving is not parallelism and not the tier — it is that **no compute went into a
package that could not close its boxes.** Batch 36's ~$30 of impossible work is
the whole difference, and it is exactly what Phase −1 removes.

### The first Phase-1b report, measured

Structured facts with flagged `decisive_claims` instead of a 200-line narrative.
Review targeted the single claim that decided thirteen boxes — that the accept
loop's wiring is load-bearing — and verified it with one independent mutation
rather than unbounded rediscovery. **15% of the worker's own compute went to the
report**, down from what a narrative package spends, and the orchestrator's
review was bounded for the first time.

### A kill by somebody else's test is stronger than a kill by your own

`invert-condition` (`>` → `>=`) on the ranking comparator was killed by
`routing_policy.rs::order_dependence::…`, a **pre-existing test from
`lead-route`'s adversarial suite**, written months earlier by a different worker
for a different package. Nothing in this round's diff put it there.

**Worth building on deliberately:** when a package touches a policy another
package already wrote adversarial tests for, run those first. A test the author
could not have tuned to pass is the cheapest independent evidence available.

### The negative result that took three rounds

The pairing prior now has a production caller and **is structurally inert at
it** — `classify` never reads `route`, so every same-model failover candidate
scores identically. Batches 35, 36 and 37 each moved this forward and the answer
is that the mechanism cannot decide the decision it was wired to.

**That is a real finding and it was not cheap.** The cost is defensible because
nothing short of building the caller could have produced it — but it is the
second time in three rounds that a *type signature* decided a capability's fate
(the first: `PairingQuery::harness` being required). **Phase −1 asks for the
producing type and the caller's field; this suggests also asking what the
producer's output actually *varies with*.** A value that exists but is constant
across the candidates being compared is a caller-shaped gap the current four
links do not catch.

## Batch 38 — one box, a killed hypothesis, and a worker recovered from death

`pairing-config`, 644 → 645. One Sonnet, ~$13, 29 minutes, +489/-83, **one box**.

**Cost per box is $13 and that is the correct number to record**, not an
embarrassment to explain away. The package closed map line 576 *and* answered
the question that decides where Phase 9J goes next. Batch 34's ledger already
records that cost-per-box measures how much was already built, not the tier —
this is the same effect at n=1.

### The Phase -1 preflight settled an architecture question before dispatch

`profile/**` must not import `crate::config` — `Resolution`'s own doc says the
caller does the lookup, and `provider: Option<&'a Provider>` is that rule in
practice. The preflight found this while checking link 2, so **the packet told
the worker the shape** instead of letting it discover the ban mid-package and
rebuild.

That is a second, unbudgeted return from Phase -1: it does not only refuse
impossible packets, it **surfaces the architectural constraint that decides how a
possible one must be built**. Worth looking for deliberately.

### §66 — a worker killed mid-report has not lost its work

`API Error: Connection lost mid-response` killed this worker **while it was
writing its report**, after the code was finished and the full suite had run. The
watch reported it exactly — pane quiet, no done-signal, no report — and the
recovery was **one message**, not a re-dispatch:

> ...cut off while you were writing the report — all your code work survived.
> Write the report now. **Do not redo any code work.**

It produced a complete, accurate report on the next turn. Re-dispatching would
have discarded ~$13 and 29 minutes of finished work and produced a second diff to
reconcile.

**The measurement: recovery cost one turn against a 29-minute re-run.** Always
establish what a dead worker actually lost before deciding what to do — a
worktree diff and a live pane are two independent copies, and an API error
usually costs neither.

### Two cargo runs hung with zero CPU this session

Both were `cargo test` blocked at 0:00 CPU for minutes — once at 10 minutes, once
at 4 — in a shared private `CARGO_TARGET_DIR` while other cargo processes existed.
Killing and re-running with a **narrower test filter** succeeded immediately both
times.

**That heuristic as first written was too broad, and the orchestrator nearly
acted on it wrongly minutes later.** A third run showed the same "elapsed
minutes, zero CPU on the `cargo` process" and was **healthy** — the parent
`cargo` idles between test binaries, so zero CPU *on it alone* means nothing.

The distinction that actually separates the two:

| | stuck | healthy |
|---|---|---|
| CPU across the whole `cargo` **+ `rustc`** tree | sustained ~0 | non-zero, moving |
| live `rustc` children | none | at least one |
| load average | flat | moving |

So: **sum CPU across cargo *and* rustc, and check for live `rustc` children,
before concluding anything.** Two genuine hangs looked like that for many
minutes; the healthy run did not.

When it *is* stuck: kill it, narrow the test filter, re-run — and **check first
whether it left a source file mid-mutation**, because one of these did, and the
restore is the urgent part, not the re-run.

## Batch 39 — two concurrent Sonnets, and the gate refused the handoff's own top pick

Dispatched from `3350abf`. Two Sonnet implementers on disjoint partitions,
started about twenty minutes apart because the Phase −1 work for the second
package was done while the first ran.

| package | boxes claimed | partition |
|---|---|---|
| `failure-domain` | 33C 1371/1372/1375/1377/1378 + 35B 1547 | `routing/**`, `gateway/session.rs` |
| `memory-retrieval` | 21F 929/931/933/935/936/937/938 | `memory/**`, `main.rs`, `cli.rs`, `api/**` |

### The third consecutive round where a handoff's "cheapest next win" was impossible

`CONTINUATION.md`'s Part 2 opened with map line 1293 — *"reserve inspectable in
routing explanations… the surface now exists and is reached in production…
should be small."* Every clause of that is true and the conclusion is still
wrong.

`routing/disposable.rs:693-707` already pushes the reserve decision's reason into
the explanation, correctly. But the loop that reaches it is
`eligible.iter().filter(|c| !c.value().cost().is_free())`, and
`main.rs::disposable_candidates` (`:1256-1300`) builds **only** `Cost::Free`
candidates — it iterates `provider_config.free_models()` and hardcodes the cost.
**The filter is always empty in the shipped binary**, so `evaluate_reserve_spend`
never runs and the contribution can never appear. Link 4 fails: the consumer
cannot observe it.

1293 is blocked on the same product decision as 1550 — *may a background job
spend paid quota unasked?* — and neither closes alone.

**Three rounds, three refusals, and each recommendation was written by an
orchestrator that had just watched the previous one fail.** Batch 37 refused the
ledger-into-`DisposableRouting` pick; batch 38's preflight redirected the pairing
work; this one refused 1293. That consistency is the argument for the gate: the
failure is not carelessness, it is that *a plausible-sounding next step written
at the end of a round is not a feasibility argument*, and no amount of care at
writing time substitutes for two greps at dispatch time.

### The fifth link decided which package to dispatch, for the first time

`assurance-economics.md` added a fifth question after batch 37 — for any input
feeding a *ranking*, what does its value vary with, and does that differ between
the alternatives? Batch 39 is the first round where it was the deciding test
rather than a note.

- **Line 1547 passes it.** `Upstream::failover_candidates`
  (`gateway/upstream.rs:562`) filters out only the currently-serving index and
  returns every other backend, so a provider with two credentials yields
  candidates that share a provider alongside candidates that do not.
  `candidate.provider() == current.backend().provider()` genuinely differs
  across the alternatives being ranked.
- **Phase 9J's prior failed it**, three rounds and roughly $39 of worker compute
  ago, because `classify` never reads `route` and every same-model candidate
  scored identically.

Both signals reach the same caller. The difference is only whether the value
varies, which is exactly what the first four links do not ask.

### Two packages that were considered and not dispatched, and why

Recording these because a refusal costs one orchestrator turn and is the
cheapest artefact this process produces:

- **Phase 32E (burn rate, 0/10).** `GatewayQuotaCache::load`
  (`provider/telemetry.rs:1191`) returns *"the most recent"* reading and
  overwrites it. A burn rate needs two readings separated in time; there is **no
  history series**, so the producer does not exist. This is a build-the-producer
  package, not a wiring one, and sizing it as the latter would have repeated
  batch 36.
- **Phase 47's routing debug views (1757, 1766).** The routing explanation lives
  in gateway session state, in the gateway's process. A CLI debug view runs in a
  different process. This is probably Phase 15/16's cross-process blocker
  wearing new clothes, and it needs checking before a packet, not after.

### What this round is testing, beyond its boxes

1. **Does the fifth link generalise?** It selected this round's package. If
   `failure-domain`'s diversity contribution turns out to change which candidate
   wins — the load-bearing acceptance test — the check has now prevented one
   inert mechanism and enabled one live one.
2. **Does a wide partition still beat a narrow one (§32)?** `memory-retrieval`
   has `main.rs`, `cli.rs` and `api/**` alongside `memory/**` deliberately,
   because 935 and 937 need a caller. `failure-domain`'s is narrower but its
   caller (`gateway/session.rs`) is inside it.
3. **Do two concurrent Sonnets on genuinely disjoint partitions collide at
   review?** Batch 35 ran three and the reviews were serial; two is the smaller
   test of the same question.

### Batch 39 outcomes — 13 boxes, and the most valuable finding was in code neither worker touched

| package | boxes | cost | time | diff |
|---|---|---|---|---|
| `failure-domain` | **6 of 6** (33C 1371/1372/1375/1377/1378, 35B 1547) | ~$6.50 | 19 min | +674/−17 |
| `memory-retrieval` | **7 of 8** (21F 929/931/933/935/936/937/938; 939 stretch, not attempted) | ~$12.20 | 29 min | +1444/−77 |

645 → 658 (50% → 51%). **~$18.70 for 13 boxes: $1.44 per box.** Higher than
batch 37's $1.00 and for the reason batch 34 already recorded — cost per box
measures how much was already built, not the tier. Phase 21F needed a new CLI
verb, a new external-door test binary and a grouping type; Phase 33C needed one
type and one contribution because `on_provider_failure` was already there.

**Both workers corrected their packets, and both were right — that is now six
consecutive rounds.**

- `rustfmt <a mod file>` recursively formats every submodule it declares. The
  packet said what §37 prescribes and the worker followed it; formatting
  `routing/mod.rs` reformatted three submodules, two of them in its own
  `FORBIDDEN FILES`. Caught in seconds by `git status`, reverted. §37 has an
  addendum now.
- Bare `rustfmt` does not read `Cargo.toml`, so it defaults to edition 2015 and
  **hard-fails** on this crate's let-chains. `rustfmt --edition 2024` is the
  correct packet line. Both workers hit this independently.
- The packet's EXPECTED FILES had no home for a test of `api/unix.rs`'s door,
  while `api/mod.rs:35-37` says that door "is proven only by running the shipped
  binary… never by an in-process unit test." The worker added
  `tests/memory_query_api.rs` on the `tests/capacity_api.rs` precedent and said
  so. That is the right call and the packet was wrong.

### The mutation that mattered was about a write nobody was watching

`memory-retrieval`'s `remove-validation` mutation deleted `mark_for_review`'s
leading project-scope guard. The integrator re-ran it: the test fails with
`right: "active"` — it read the **foreign project's row back** and found it
flipped to `needs_review`. The call still returned a correct-looking error
either way, because the function's *trailing* `self.get(id)` re-checks scope
after the write.

**A test asserting only the returned error would have passed against a build
that silently corrupted another project's memory.** The worker's test reads the
row back at the raw-connection level precisely to catch that, and said so.

Following it up cost the integrator four greps and produced the round's most
useful artefact: **six `UPDATE memories … WHERE id = ?1` statements carry no
`project_id` in their own WHERE clause.** All five enclosing functions currently
have the leading guard, so there is no live defect — checked one by one, and an
initial heuristic that flagged two of them was **wrong**, because `supersede`
names its parameter `old` and `set_authority` splits `self` from `.get(id)`
across lines. Say the checked answer, not the scan's.

The objection worth recording is the one that nearly closed the question early:
`database.rs` *does* defend this table with triggers, and its own comment at
line 248 makes the argument — *"a query can forget to filter by `project_id`; a
`BEFORE INSERT` / `BEFORE UPDATE` guard cannot be forgotten."* **It does not
reach this case.** The trigger is `BEFORE UPDATE OF project_id`, so a status-only
write to a foreign row fires nothing. The triggers protect the *binding*; they
do not decide *who may write the row*.

That is a red-tier package for the next round, and it is the closest existing
work to Phase 1 line 110, which is unstarted and has no ledger entry.

### The fifth link's first prediction held

Batch 39 dispatched line 1547 because the domain signal varies across the
candidates being ranked, and declined line 1293 because its consumer is
unreachable. The dispatched one closed six boxes with a contribution that
**changes which candidate wins** — proven by neutralising it to `0.0`, which
makes the pair tie, and `best()` prefers the first, which the test lists as the
wrong answer. Contrast Phase 9J's prior at the same caller: built, wired,
mutation-proven, inert. One check, two opposite outcomes, both correct.
