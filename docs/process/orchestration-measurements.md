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

## Batch 40 — one Sonnet, three boxes, and a defence-in-depth change proven both ways

Dispatched from `696b368`; integrated onto `3492459`. **One** Sonnet implementer,
26 minutes, ~$7.53, +918/−48 across five files. Boxes 948, 949, 950 closed;
`951` proposed closed and **declined by the integrator**.

| package | boxes claimed | boxes closed | partition |
|---|---|---|---|
| `mem-revalidate` | 21G 948/949/950/951 (+944/945/946 argued) | 3 | `memory/store.rs`, `main.rs`, `cli.rs`, 3 test files |

**~$2.51 per box** at the worker, against batch 39's $1.44 and batch 37's $1.00.
The number is honest and the reason is visible: this package wrote its own
mechanism from nothing (four store wrappers, a reviewer gate, a CLI verb, a
listing command) rather than wiring one that already existed, and it spent a
quarter of its turn on a hardening change that closed **no box at all**. Cost
per box measures how much was already built — batch 32 established that — and
this round is another instance, not a tier signal.

### One worker, deliberately — and the user overruled it the same day. The cost model was wrong.

Batch 40 ran one worker and this ledger recorded the reasoning approvingly: the
weekly window was at 91%, the only other gated candidate shared this one's
partition, so a second worker meant an *ungated* package at ~$15 a miss.

**The user's correction: "sonnet workers are incredibly cheap, use two or even
three — you are the expensive part."** It is right, and the numbers were already
in this repository:

| | cost | note |
|---|---|---|
| `mem-revalidate`, Sonnet, 26 min | **~$7.53** | closed 3 boxes |
| the batch-40 orchestrator, Opus xhigh | **~$17.50** by integration | one round |
| the batch-39 predecessor pane, Opus xhigh | **~$62.27** at 54% context | 3h18m |

**The error was the denominator.** Every cost-per-box figure in this ledger
counts *worker* compute and silently omits the orchestrator, which is the larger
and — this is the part that matters — the **fixed** term. Gating, packet writing,
review, mutation re-runs, evidence, map, commits and the platform gate are paid
once per *round*, not once per package. Spreading them over one package instead
of three is the expensive choice, and no amount of worker frugality recovers it.

**So the rule is not "dispatch carefully, therefore dispatch one."** It is:
**gate three packages and dispatch three.** The gate is cheap — `discover.py
--seam` plus reading what it prints — and a refusal costs one orchestrator turn.
What is expensive is spending a whole round's fixed overhead on a single
package's worth of boxes.

**Where the original reasoning still holds:** an *ungated* package is still
negative expected value, and partition disjointness is still a hard constraint.
The fix is not to dispatch blind, it is to **do the gating for three and find
three disjoint partitions** — which took one orchestrator turn when actually
attempted, and produced `memory/**`, `routing/**`, and a read-only recon package
that cannot be premise-invalid by construction.

**A third worker does not need to be an implementer.** Batch 41's `gate-recon` is
read-only: it runs the Phase −1 five-link check across five unassessed phases and
reports verdicts. That is work the orchestrator would otherwise do at Opus rates
inside its own scarce context, and it is exactly the shape that should be pushed
down. **When no third implementation package is gated, the third worker should be
gating the next round.**

### The integrator's own mutation was worth more than the worker's five

The worker ran all five required mutations and killed all five. The integrator
then re-ran the decisive one **in both directions**, which is the part that
actually established the claim:

- guard removed, hardening kept → isolation test **passes** (the scoped `WHERE`
  carries the protection on its own — this is what the change bought)
- guard removed **and** hardening removed → isolation test **FAILS** (so the test
  is not vacuous and would have caught the original shape)

Only the second run proves the first means anything. A single mutation showing
"still passes" is indistinguishable from a test that cannot fail. **When a
hardening change is meant to make a mutation survivable, the evidence is two
runs, not one** — the same shape as §40's rule that a FAIL needs two runs.

### A box was proposed closed and declined, and the rule it turns on

`951` — *"avoid automatic revalidation work when the memory is not about to
affect any current task."* The worker's evidence was that no sweep code exists,
which its own report described as *"a structural/negative check, confirmed by
reading the diff rather than a runtime test."*

The SDLC's rule decides it: a regression test counts only when it **would fail if
the required behaviour were removed**, and nothing here would. The line also
presupposes automatic revalidation, which does not exist — the same shape as map
line 1748, un-ticked once for exactly that reason. **A worker being right about
the code and wrong about the box is the normal case**, and it is why the
integrator rules rather than the report.

### The packet was wrong in a way that would have cost a rerun, and the worker caught it

`cargo test -p glasshouse --all-features project_isolation` matches **zero
tests** — cargo's trailing argument is a substring filter on test *names*, and no
test in `tests/project_isolation.rs` contains that substring. Every target
reports `0 passed … N filtered out`, which reads exactly like a clean run. The
worker ran it literally, noticed, and used `--test project_isolation` (7 passed).

**Sixth consecutive round in which the worker corrected its packet and was
right.** The general form is §54's again: a command that silently matches nothing
looks identical to a command that passed. Whole-target verification commands must
be `--test <name>`.

## Batch 41 — three workers in parallel, and the first prospectively-caught inert input

Dispatched from `a08432c`/`b71b6e3`, integrated as `ae12e31`. **Three Sonnet
workers, started in one turn**, on provably disjoint partitions. Seventeen boxes.

| package | boxes claimed | closed | partition | cost |
|---|---|---|---|---|
| `mem-ladder` | 21E 914-918, 924 | **6** | `memory/policy.rs`, `memory/search.rs` + 2 tests | ~$7.75 |
| `route-evidence` | 35B 1541, 1542, 1548 (+1545) | **2** | `routing/**`, `config/pairing.rs` | ~$6.81 |
| `knowledge-view` | 25 1098-1104, 1106, 1107 | **9** | `shell/**` | ~$9.85 |
| `gate-recon` | none — read-only Phase −1 recon | n/a | writes one file outside the repo | ~$3.90 |

**~$28.31 of worker compute for 17 boxes — $1.67 each — and that number is still
the wrong one to optimise.** The orchestrator turn that gated all three, reviewed
all three, ran the gate three times and wrote the evidence is the fixed cost, and
it was paid **once** instead of three times. That is the entire finding.

### Gating three took one turn, which is the answer to the objection

Batch 40's recorded reasoning was that careful gating argues for fewer workers.
It does not. `discover.py --seam` plus reading what it prints settled three
packages in a single orchestrator turn, and `validate_round.py` proved the
partitions disjoint mechanically. **The gate is cheap; the round's overhead is
not.** What is expensive is spending a round's fixed cost on one package.

### The recon worker is the finding worth generalising

Only two implementation packages were gateable on disjoint partitions. The third
slot went to a **read-only** worker running the five-link check across five
unassessed phases. It cannot be premise-invalid by construction, and at ~$3.90 it:

1. **Refuted a claim two checkpoints carried.** Phase 47's debug views were
   recorded as blocked by a cross-process boundary; the gateway is a direct call
   in the *same* process (`main.rs:534`). The real blocker — a guard that is
   never read, and an explanation discarded into a `tracing` field — is fixable.
2. **Caught an inert input inside a live worker's packet.** `ContextState` is
   `Unknown` on 100% of real rows, so map line 1545 fails the fifth link. Relayed
   mid-round after independent verification.
3. Produced the next package (Phase 25), which became the third implementer.

**This is the first time the fifth-link failure was caught *before* the code was
written.** Phase 9J's prior cost three rounds and ~$39 to discover the same way
after the fact. **When only two implementation packages are gated, the third
worker should be gating the next round**, not idle.

### Two boxes declined for the same reason, in the same round — that is the test

`35B 1542` names *"observed success **and** reliability"* and
`ObservedEvidence::reliability` is `None` on every real row. `35B 1545` names
cache affinity and `ContextState` is `Unknown` on every real row. **Refusing one
and accepting the other would have been the inconsistency worth catching.** An
input absent or constant across all real data cannot support a box that names it,
whoever proposes it and however good the surrounding code is.

### A change to global ordering has a blast radius the packet did not scope

`mem-ladder`'s ladder broke a Phase 21B test in `memory_provenance.rs` — a target
the packet's verification list did not name, because the list was scoped to test
files whose names matched the feature. It failed on **both** macOS and Ubuntu in
the integration gate, which is the right net but the expensive one.

**Rule: scope a packet's verification by blast radius, not by name match.** A
change to search *ordering* can break any test that asserts an order, anywhere.

### A survived mutation, reported rather than buried

`knowledge-view` found that removing `OpenProjectKnowledge`'s `Err` arm kills no
test, because it lives in `shell::run()` — the real event loop, which nothing in
this codebase unit-tests. **It reported this against its own package.** Phase 41
has the structurally identical untested arm, so it is a pre-existing shape, and
the honest disposition is recorded debt rather than a weakened test.

## Batch 43 — a worker found a dead check inside the gate

Dispatched from `0af9bb3`, integrated as `4ec21b9`. Two Sonnet implementers plus a
read-only recon. **Four boxes closed (1762, 1764, 1314, 1315)**, two closed by a
worker and **declined by the integrator**, and thirteen Phase 33 lines left open
on one shared wall.

| package | closed | note |
|---|---|---|
| `evidence-table` | 2 | `packet_errors: []` — the first this session |
| `health-proof` | 4 proposed, **2 ticked** | a proof pass; `src/**` forbidden to it |
| `recon-phase51` | n/a | all 37 lines blocked, one shared cause |

### The finding: `validate_round.py`'s check 3 had been inert all session

`health-proof`'s packet quoted nine Phase 33 lines. The worker compared them
against `discover.py --phase 33` and found **five wrong at the same line
numbers** — two with their topics swapped. **The round had passed validation.**

`parse_box_lines` matched only lines whose *first* character was `☐`, the shape
the map uses. Every packet this project writes dresses the quote as a list item
labelled with its line number (`- **1311** ☐ …`), so the parser saw **nothing**,
and check 3 — *"every quoted box line matches the map verbatim"* — verified zero
lines while reporting PASSED.

**That is practice §68's defect inside the gate built to catch it**, and the
fifth costume of the shape this project keeps recording: `blind` not zero (§54),
`unknown` not a session id (§67), *"0 tests matched"* not *"tests passed"* (§68),
your own recommendation not the user's decision (§70), and now **a check that
matched nothing reporting PASSED**.

Fixed in `5971e9f` with two regression tests, verified in both directions: seven
mismatches now reported in the bad packet, zero on six correctly-quoted packets
from batches 40–43.

**The measurement worth keeping: a mechanical gate needs its own non-vacuity
check.** This project mutation-tests its product code as a matter of course and
had never asked whether its *process* checks could fail. One did not, silently,
for at least four rounds.

### A proof package is a distinct and cheap package type

`health-proof` was given **all of `src/**` as forbidden** and asked to prove or
refuse nine lines a recon had called "already satisfied". It produced four
closures, five refusals, and — more valuable — the reachability wall behind
Phase 33: `ResourceHealth` is written for every exchange and **observable by
nothing outside the gateway module** (`free_pool()` has zero callers in `src/`;
the one router reading it builds an always-empty pool; `api/unix.rs:331` says so
in its own doc).

**That finding is what stops the next four packages being written against a false
premise**, and it cost one worker that wrote no production code at all.

### The integrator declined two of its four closures

1320 and 1323 rest on *existing* tests, 1323 partly on a source-scan proof of
absence. Practice §14 records that shape as a trap and map line 1748 was un-ticked
for a vacuous absence claim. **Declining them costs a round; ticking them wrongly
costs the project's ability to trust its own ledger.** They are recorded as strong
candidates needing a mutation check, not as rejected.

## Batch 44 — the fifth consecutive round whose top recommendation failed the gate, and what that now means

Dispatched from `c81dd9b`, records committed as `2796575`. A resumed orchestrator
with a fresh context, a weekly window at **97%**, and the user's explicit budget
instruction: *cheap workers, stay light.* **Two read-only Sonnet packages, no
implementers**, chosen because the two implementation packages the checkpoint
recommended both failed Phase −1 at the orchestrator's own gate.

| package | tier | kind | note |
|---|---|---|---|
| `p51-eventlog` | Sonnet · high | read-only | design input for the Phase 51 event-log migration the user asked to have scoped deliberately |
| `ledger-audit` | Sonnet · high | read-only | audits **ticked** boxes whose line names a field no production writer sets |

### Two refusals, both of the checkpoint's own "next, in order" list

**1. "Give `ResourceHealth` an externally-readable surface — one consumer
unblocks 1311/1321/1322/1324."** Carried by two checkpoints as the cheapest real
work available. It does not connect, and the failing link is **propagation**:

- `resources_report` is called at `main.rs:140` from the **CLI dispatch**, whose
  only argument is `&runtime`. The gateway starts at `main.rs:534`, on the
  **session-launch** path. `glasshouse resources` never holds a `Gateway`, so a
  health section there renders an empty pool on every run — the identical
  always-empty-pool defect already recorded for the router's caller, one surface
  further out.
- `free_pool()` returns a **clone of in-memory state that nothing persists**.
  `api/unix.rs:331` already says so in its own doc.

The finding underneath it is the useful half: **`ResourceHealth` is much richer
than four checkpoints credited.** Degradation (`consecutive_failures` +
bounded-doubling cooldown), recovery (`WorkloadOutcome::Served` clears it),
availability (`is_available`), the health/quota separation
(`FreePool::is_available`), and even the enumerator §71 asks for
(`FreePool::observed()`, whose own doc says it exists *"for a settings or
diagnostic view"*) are **all already built**. The four boxes are not blocked on
behaviour. They are blocked on a decision nobody has made: **persist it, or show
only the current process's own pool.**

**2. "Phase 51's `purpose` alone — smaller than advertised, still real."** Also
does not connect, and this one is worse than premise-invalid — it points at a
**box that is already ticked.**

`NewObservation.purpose` (`routing/evidence.rs:247`) defaults to `None` (`:279`),
is written to SQLite (`:711`) and read back (`:1048`). The single production
writer, `gateway/session.rs::record_routing_observation` (`:278-325`), sets
route, harness, quota context, timing and outcome and **never** `purpose`; no
`with_purpose` builder exists. Meanwhile map line **1330 is `☑`** and reads:

> Record provider, route, model identity, authenticated quota context, harness,
> **request purpose**, and observation timestamp for each measurable turn.

Seven facts named, six recorded. And the gateway cannot supply the seventh:
`Exchange` (`gateway/ingress.rs:117`) carries `outcome`, `status`, `provider`,
`protocol` and `host` — **nothing purpose-shaped**, and `protocol` is already
recorded as `route`.

**That is the standard by which 1542 and 1545 were refused** — *"an input absent
across all real data cannot support a box that names it"* — applied to a box that
is closed. Whether it is one box or a cluster is what `ledger-audit` was
dispatched to establish, because **one lead is not a pattern** and un-ticking on
an orchestrator's single grep is exactly the over-confidence this project keeps
paying for.

*(A near-miss worth recording: `SessionRecord.purpose`, rendered at `main.rs:3096`
and `:3230`, is the Phase 10 session tag — a different field with the same name.
The orchestrator briefly read it as a consumer of the observation's purpose and
checked before writing it down. A grep for a field name crosses types silently.)*

### The measurement: five for five, and the conclusion has changed

`assurance-economics.md` records three refusals and notes that *"every one of
those was the previous checkpoint's own recommended next step."* With batches 42,
43 and now 44, it is **five consecutive rounds**. That is no longer a curiosity
about one careless handoff — the rate is too high for that, and the checkpoints
in question were written by careful orchestrators who had *just* learned the
lesson.

**So the recommendation itself is the defect, not the recommender.** A next step
written at the end of a round is written when the code is least fresh and the
reasoning most compressed, and it then arrives at the next session wearing the
authority of a finding. The existing rule — *run the gate before dispatch* —
catches it, but only after a fresh orchestrator has spent a turn believing it.

**The cheap fix is one of labelling, and it costs nothing:** a checkpoint should
record *what was checked and found true*, with its `file:line`, and mark anything
forward-looking explicitly as **a lead requiring re-gating** rather than as "next,
in order". The distinction this project already draws between a *fact* and a
*recommendation* when relaying to a worker (§39) applies to the handoff itself,
and it has never been written down there.

**Cost of the two refusals: roughly fifteen minutes of one Opus turn, before any
worker compute.** The alternative, at batch 36's measured rate, was ~$30 of
packages that could not close their boxes.

### Batch 44 outcomes — no boxes closed, one box *re-opened*, and that is the result

| package | tier | wall-clock | worker cost | output | boxes | verdict |
|---|---|---|---|---|---|---|
| `p51-eventlog` | Sonnet · high | ~8 min | ~$2.27 | 571-line report, 0 repo edits | 0 (read-only) | PASS |
| `ledger-audit` | Sonnet · high | ~6 min | ~$1.44 | 201-line report, 0 repo edits | **−1** | PASS |

**Two read-only packages cost ~$3.71 together** — half what one implementation
package costs — and both returned findings that change what the next round builds.
Neither edited a file; `git status --porcelain` was empty at both closes, which
was an explicit acceptance condition.

**`ledger-audit` confirmed the 1330 lead and found the general shape behind it.**
It audited all 27 ticked lines in Phases 33A, 33C and 35B and returned **24
SOUND, 3 PARTIAL**. The one that was un-ticked, 1330, was ticked from its
evidence entry's *summary* line while that same entry's *per-line disposition*
read `PARTIAL — open on purpose alone`. Both sentences had been in the file the
whole time.

**The systemic finding, which is worth more than the box:** in all three PARTIAL
cases **the evidence file was honest and the map tick was more generous than the
entry it rested on.** Nobody overclaimed in the ledger; the gap is between an
entry's summary and its own body. `check-evidence-coverage.py` verifies an entry
*exists* for a phase — nothing verifies it agrees with itself. **That is a cheap
gate nobody has written**, and it is the same family as every other check this
project has found reporting success while measuring nothing (§20, §31, §54, §68,
and batch 43's dead check inside `validate_round.py`).

**The two it flagged were deliberately not reversed**, and the distinction is the
ruling: 1377 and 1541 were closed *knowingly*, with the narrowing recorded at the
time. Reversing a weighed judgement needs more than one audit at the end of a
spent window; 1330 needed only agreement with itself. The sharper evidence is
attached to both for the next round.

**`p51-eventlog` turned the user's "scope the migration deliberately" into a
brief, and found four lines that need no migration at all.** `GatewayUnhealthy`
and `GatewayBackendChanged` already exist as `lifecycle_events` kinds with a real
production writer, so map lines 1836/1837/1851/1852 need an aggregate read method
and nothing else. It then argued the packet's decisive question both ways and came
down on a **split**: extend `lifecycle_events` only for facts already in its
vocabulary, new table for the memory- and routing-decision clusters — extending
the recorded design decision, which had named only the memory cluster.

It also found two lines blocked on an **absent feature** rather than on counting:
`Guardrail` has zero matches in `crates/glasshouse/src`, so 1842/1843 have nothing
to instrument.

### The orchestrator's own gating was the round's most valuable output, and it was free

Three findings came from the orchestrator reading source before writing packets,
at a cost of roughly one turn:

1. the `ResourceHealth` package's propagation failure (two refusals above);
2. the 1330 lead, which `ledger-audit` then confirmed and generalised;
3. **and a correction of finding (1) an hour later.** Having written *"persist
   health — a migration, Red tier"* into a commit, the orchestrator found
   `GatewayQuotaCache`: a versioned JSON file cache that already carries a
   gateway-only observation across the identical process boundary, whose own
   module comment states the problem in the same words. **Not a migration, not
   Red — an Amber Sonnet package with a shipped precedent and a copyable
   acceptance test.**

**The transferable rule from (3), and it is new:** when a propagation link fails,
**look for a sibling signal that already crosses the same boundary before
concluding the boundary is the problem.** Four checkpoints asked *"can anything
outside the gateway observe `ResourceHealth`?"* and correctly answered no. None
asked whether this codebase had already solved that exact crossing — and it had,
once, in the module the intended consumer already calls.

### Seventh consecutive round a worker corrected its packet and was right

`ledger-audit`'s packet named `docs/product/evidence/phase-35.md` for the Phase
35B audit; the correct file is `phase-35b.md`, and both exist. It read the right
one, said so in `PACKET ERRORS`, and confirmed every other citation in the packet
matched the tree *"exactly, down to the line numbers."*

### The round's own finding, turned into a gate before the round ended — `b00ed35`

`ledger-audit` found that **nothing checks whether an evidence entry agrees with
itself**, and that this is what let 1330 stay ticked. That is a ~40-line check,
so it was written rather than left as a lead.

`check-evidence-coverage.py` — which already owned the *entry exists* and *state
vocabulary* checks — grew a third: **a ticked box whose own entry calls it
`PARTIAL`/`OPEN`/`BLOCKED`/`NOT STARTED` fails the gate.**

**Proven both directions before it was committed**, which is §20's standard and
the one batch 43's dead check failed: re-ticking 1330 in a temporary map makes it
report *"map:1330 is ticked, phase-33a.md says PARTIAL"* and exit 1; the real tree
exits 0. Six regression tests, in `scripts/tests/` where `ci-local.sh` already
runs them.

**Two things about its construction are the transferable part.**

- **The join-then-normalize step has its own test.** Evidence files hard-wrap at
  ~76 columns while the map stores each box as one long line, so a matcher that
  compared them raw would find nothing and report clean — **which is precisely how
  `validate_round.py`'s box check sat inert for four rounds.** The failure mode of
  this check is identical to the failure mode of the check whose defect motivated
  it, so that step is asserted directly rather than implied.
- **The load-bearing test is the negative one:** an *unticked* box called `PARTIAL`
  must **not** be flagged. That is the ordinary, correct state of most of this
  ledger, and a check that fired on honest entries would be switched off within a
  day — §20's *"a gate that starts red teaches everyone to override it"*, from the
  other side.

It went in as `--strict-consistency` immediately rather than warn-only, because
§51's reason for warn-only is a backlog, and there is none: the ledger is clean
under it today.

**Three of this project's process checks have now been found reporting success
while measuring nothing** (the MSRV gate, `ci-local.sh`'s Linux leg,
`validate_round.py`'s box check). This is the first one written with that
failure mode assumed from the start.

---

## The 5x → 20x plan upgrade, 2026-08-29 — the baseline, and what it has to prove

> **This is implementation cost, not product design, and the two must not merge.**
> What it measures is *what building Glasshouse costs us* — an account, a plan, a
> weekly window, tokens spent by our own workers. It says nothing about what
> Glasshouse does, and none of it is a requirement.
>
> **It is specifically NOT Phase 51.** Phase 51 (Evaluation hooks) is a *product*
> capability: Glasshouse measuring whether its own features are useful to the
> person running it, for the user's alpha A/B work. That needs an event-log table
> inside the product. This needs `ccusage` and a shell script, and lives in
> `scripts/`. **A packet that reaches for `usage-snapshot.py` to satisfy a Phase 51
> line has crossed the boundary `docs/process/orchestration-practice.md` §50
> exists to keep**, and the reverse — building plan-usage tracking into the
> product because we happen to want it — is the same error mirrored.


The account moved from a **5x to a 20x Max plan** on **2026-08-29**, to buy more
parallel workers. This section is the before-half of that measurement, frozen
while it is still readable.

**How the moment was observed, since it decides which cycle is contaminated.**
At roughly 00:30 local this session's status line read `RL5=9, RL7=98`; at 00:54
it read `RL5=0, RL7=0`, with `RL7_RESET` **unchanged** at 2026-09-01 00:00. Both
counters reset without the window moving. The 5x weekly allowance had been
effectively spent hours earlier — the previous checkpoint recorded 92%, and this
session opened at 97%.

### The frozen baseline

`ccusage daily --json` is captured at `.agent-runtime/usage-baseline/pre-upgrade-2026-08-29.json`
— **42 days, 2026-06-02 to 2026-08-29, $9,278 API-equivalent, 16.0B tokens**.
It is frozen deliberately: ccusage can only read as far back as the agent logs
are retained, and those rotate.

Cycles on the account's real boundary (Tue 00:00, which is what `RL7_RESET` lands
on), via `scripts/usage-snapshot.py --report`:

| Tue→Mon cycle | active days | API-equivalent | tokens | |
|---|---|---|---|---|
| 2026-08-11..17 | 6 | $774 | 1.22B | |
| 2026-08-18..24 | 7 | $2,681 | 4.59B | last clean pre-upgrade cycle |
| 2026-08-25..31 | 5 | $3,009 | 6.01B | **both plans — do not judge from this** |

### Three things that would have made this comparison wrong

**1. `ccusage` is account-wide, and this project is five days old.** Its output
carries no project path — `daily` groups by date, `session` by session UUID.
`~/.claude/projects` holds **151 project directories, 114 of them Glasshouse**.
Since Glasshouse's first commit is **2026-08-24** and ccusage's history starts
2026-06-02, **most of that $9,278 is other repositories.**

So `scripts/usage-snapshot.py --glasshouse` reads the raw session logs directly
and sums only the Glasshouse directories. Glasshouse's own consumption:

| day | output | cache-create | cache-read | messages |
|---|---|---|---|---|
| 08-24 | 0.63M | 1.2M | 0.25B | 706 |
| 08-25 | 8.99M | 21.4M | 2.77B | 8,690 |
| 08-26 | 9.38M | 25.4M | 2.43B | 9,901 |
| 08-27 | 13.62M | 34.8M | 4.12B | 14,737 |
| 08-28 | 4.51M | 14.4M | 1.31B | 5,211 |
| **total** | **37.1M** | **97.2M** | **10.9B** | **39,245** |

**2. The logs are UTC and ccusage is local.** Around midnight Europe/Berlin the
two disagree by a day. **Compare whole cycles, never single days.**

**3. There is only one pre-upgrade cycle of box-closing to compare against, and
it is the contaminated one.** Boxes at each day's last commit: 58 (08-24) → 209
→ 392 → 631 → 688 (08-28). **All of this project's delivery happened inside the
cycle the upgrade landed in.** So "boxes per cycle, before vs after" cannot be
answered from history, and any claim that it can is reading the 08-18 cycle's
$2,681 as Glasshouse work when the repository did not yet exist.

### So what the upgrade actually has to prove

Not "did we use more" — that is guaranteed. The honest question is whether the
**ceiling stops being the binding constraint.**

On 5x, the measured shape was: **~630 boxes closed in four days, and then the
weekly window ran out** — 92% on day 4, 97-98% by day 5, with three days of the
cycle left. Work did not slow down because the map got harder; it stopped
because the allowance was gone.

**The test, then:** in the first clean post-upgrade cycle (**2026-09-01..09-07**),
does the window survive seven working days? If it does, the gain is the days that
were previously unavailable, and it is measurable as boxes closed in a cycle that
never capped. If the window still empties, the constraint was never the plan.

**And the second question the data already half-answers.** Of 37.1M Glasshouse
output tokens, **13.7M came from the main checkout — the orchestrator — more than
the next seven worker directories combined (3.5M).** The user's standing
correction (*"you are the expensive part"*) is not a hunch; it is the largest
single line in the account. **More parallel workers do not touch it.** What
touches it is team leads (§10), which move review out of the orchestrator's own
context — and review capacity, not quota, is what §9 measured as the real ceiling
at three concurrent workers.

**Record the post-upgrade half the same way**, or the comparison is two different
measurements:

    scripts/usage-snapshot.py --capture post-upgrade
    scripts/usage-snapshot.py --report
    scripts/usage-snapshot.py --glasshouse --since 2026-09-01

## Batch 45 — the deliberate parallelism test: eight workers at once

**This round exists to answer a question, not only to close boxes.** The user
asked, hours after the 20x upgrade, to *"fire a lot of parallel work"* — the
metaphor was a round mountain with many workers boring inward to meet in the
middle, *"needing a bit of adjustment on the final stretch."* That is a precise
description of the design below, including the part that costs.

**Eight workers, dispatched together**, all Sonnet 5 · high, all verified by
reading their panes (§67) rather than trusting the flag:

| package | kind | partition |
|---|---|---|
| `health-cache` | implementer | `provider/telemetry.rs`, `provider/resources.rs`, `gateway/**`, `api/unix.rs`, `main.rs` |
| `phase-41-overview` | implementer | `shell/**` |
| `recon-capability` | read-only | Phases 34, 34A, 34F |
| `recon-router` | read-only | Phases 34B, 34C, 34D, 34E |
| `recon-candidates` | read-only | Phases 35A, 35C, 35D, 36, 37 |
| `recon-context` | read-only | Phases 27, 28, 29, 30, 31 |
| `recon-control` | read-only | Phases 40, 42, 43, 44, 45 |
| `recon-capacity` | read-only | Phases 32A, 32C, 32E, 32G, 33B |

`validate_round.py`: **8 packets, no conflicts.**

### The shape, and why it is not just "more workers"

**Six of eight are read-only and cannot be premise-invalid by construction.**
That is deliberate. The binding constraint on this project has never been worker
throughput — it is that **five consecutive rounds opened with a recommendation
that failed the dispatch gate**. Gating is the bottleneck, and gating is exactly
what a cheap read-only worker can do in parallel while implementers build.

So the wave is: **two workers bore where the gate already passed, six bore toward
the next ring of gated work.** Wave two dispatches from what they find. That is
the mountain: many faces, one interior, and the tunnels only connect if the
gating was honest.

**The adjustment on the final stretch is `main.rs`, and it is the orchestrator's.**
Every package is scoped to its own module subtree; `main.rs` is given to exactly
one worker and forbidden to the rest, with the standing instruction to report the
patch rather than reach across (§32's rule, which cost a whole batch to learn).
Integration is where the tunnels meet, and it is serial by nature.

### What this measures, beyond boxes

1. **Does eight collide at review?** §9 measured three as the point where reviews
   start colliding, and that was an *attention* limit, not a quota one — so 20x
   does not move it. Six of these return structured verdict tables rather than
   diffs, which is the hypothesis: **recon reports are cheap to review, diffs are
   not.** If that holds, the real ceiling is *two or three diffs plus any number
   of reports*, and that is a more useful rule than "three workers".
2. **Does the weekly window survive it?** The 5x plan ran out on the fourth
   working day of a cycle. This is the first round where quota is not the
   constraint, and the cost is recorded per package.
3. **Does gating in parallel actually produce dispatchable work?** Six recons
   assessing ~130 open lines should yield wave two's packages. If they mostly
   return BLOCKED, that is the finding — it means the map's remaining work is
   genuinely gated behind missing producers, not behind orchestrator attention.

### Batch 45's recon half — the answer, and it is not the one the question expected

**Six read-only workers assessed 255 open lines — 43% of everything still open on
the map — for roughly $11, in parallel, in about twenty-five minutes.** Every one
returned a structured verdict table with `file:line` citations; none edited a
file; `git status --porcelain` was empty in the main checkout throughout.

| recon | lines | CLOSABLE | blocked/inert |
|---|---|---|---|
| `recon-capability` (34, 34A, 34F) | 31 | 1 (weak) | 30 |
| `recon-router` (34B–34E) | 50 | 11 | 39 |
| `recon-candidates` (35A, 35C, 35D, 36, 37) | 47 | 3 | 44 |
| `recon-context` (27–31) | 39 | 7 | 32 |
| `recon-control` (40, 42–45) | 32 | 10 | 22 |
| `recon-capacity` (32A, 32C, 32E, 32G, 33B) | 56 | 2 | 54 |
| **total** | **255** | **34** | **221** |

**The 221 is the product, not the 34.** Each blocked line is a package nobody
will now dispatch against, and this project's measured rate for dispatching
against a bad premise is ~$15 of unrecoverable worker compute per package. The
whole assessment cost less than one such mistake.

### And the finding is architectural, not administrative

Five of the six clusters name **the same missing thing**:

> **Nothing in the shipped binary makes a routing decision over a real candidate
> set.**

`routing::classify::classify` has exactly one production caller — the
`glasshouse classify` CLI diagnostic (`main.rs:144`). `Capabilities` is populated
by all seven adapters and read by exactly two things, a test and
`glasshouse doctor` (`integrations/mod.rs:1123`) — neither a router.
`WorkloadTier` reaches production only as a **hardcoded `Leaf` literal**
(`routing/disposable.rs:565`).

**So the remaining 593 open boxes are not 593 independent gaps.** A large share
sits downstream of one absent seam, and the map's phase ordering hides that
because the dependency runs sideways across phase families rather than down.
**No amount of parallelism closes a box behind that seam**, which is the most
useful thing this round could have learned before spending a wave of implementers
on it.

**`recon-candidates` proved the point by refusing to give work.** Asked to assess
five phases, it reported that every one presupposes a candidate set or a
session-start decision point that does not exist, and recommended **against**
dispatching from its own phases — *"dispatching any of them now repeats exactly
the mistake this packet exists to catch."* A worker declining to manufacture a
package is the outcome to reward; it is the same instinct that made `route-view`
refuse to fabricate a plausible table in batch 42.

### What this says about the parallelism question

**The hypothesis held: recon reports are cheap to review, diffs are not.** Six
reports were read, ruled on and consolidated inside one orchestrator turn without
collision, because each is a table with citations rather than a diff needing
mutation checks. §9's ceiling of three is a limit on **diffs**, not on workers.

**The operative rule is therefore better stated as: two or three implementers,
plus as many read-only workers as there is gated ground to survey.** That is not
what "increase parallelism" would have produced if read naively, and it is what
the 20x plan actually bought — not four times the implementers, but the freedom
to spend a whole wave on *finding out what is worth implementing.*

**One caution, recorded before it bites.** Six recons produce six CLOSABLE lists,
and a CLOSABLE verdict is a **lead, not a closure** — batch 43 declined two of
four proposed closures, and batch 44 re-opened a box that had been ticked for a
round. **34 candidates is 34 things to verify, not 34 boxes.** The orchestrator
already declined one of them (`1395`) on the recon's own contradicting evidence.

### Batch 45's implementation half — eight workers, and the review ceiling held

| worker | kind | cost | diff | claims |
|---|---|---|---|---|
| `health-cache` | implementer | ~$12.15 | +1207/-21 | 1311, 1321, 1322, 1324 |
| `phase-41-overview` | implementer | ~$9.91 | +561/-31 | 1657-1660, 1663 |
| `handoff-checkpoint` | implementer | ~$9.76 | +581/-6 | 1638-1640, 1642-1645 |
| `proof-router` | proof, tests only | ~$3.82 | 1 test file | 1413-1418, 1424 |
| `classify-caller` | implementer | ~$3.00 | +125/-3 | none by design |
| `codex-hooks` | implementer | ~$1.89 | +61/-11 | none — wiring |
| `compaction-events` | implementer | ~$1.50 | **none** | **refused, correctly** |
| six recons | read-only | ~$11 | none | 255 lines assessed |

**Roughly $53 of worker compute against an orchestrator that stopped at 69%
context.** `RL7` finished the round at **2%** — the 20x plan meant quota was never
the constraint, for the first time in this project's history.

### The three numbers worth carrying forward

**1. A recon's CLOSABLE is ~65% reliable, measured.** `recon-router` reported
eleven lines closable; `proof-router` tested each and **seven survived**. That is
the first time this project has put a number on it, and it justifies the proof
package as a standing tier: cheap, `src/**` forbidden, and it converts leads into
either mutation-proven closures or documented refusals.

**2. Six of seven implementation packets carried a deferral instruction**, because
a file they needed belonged to another live worker. That is the measured cost of
strict file partitioning and the reason §77's convergent co-editing is worth
trying.

**3. The review ceiling is a limit on diffs, not workers.** Six read-only recons
were read, ruled on and consolidated inside a single orchestrator turn without
collision. Five implementers produced ~2,500 lines that are still parked
un-integrated at handoff — which is exactly §9's ceiling doing what it does. **The
operative rule: two or three diffs, plus as many read-only workers as there is
gated ground.**

### The round's real defect rate was the orchestrator's

**Three packet defects, all mine, all caught by workers, none by me** (§75, §76,
§78): a recon's claim promoted into a producer link, a cited symbol with zero
non-test call sites, and a packet that forbade the file its own evidence lived in.
**Eleven consecutive rounds a worker has corrected its packet and been right.**

That streak is the strongest signal in this ledger, and it points somewhere
uncomfortable: **the bottleneck is not worker quality, and it stopped being quota
today. It is the correctness of what the orchestrator hands them.** One of the
three is now mechanically checked (`validate_round.py --strict-seams` /
`cited-seams`); the other two are still habits, and §76 records that habits fail
under load.

## 2026-08-29 — the local gate, measured properly, and a cache change rejected on its own numbers

A parallel session timed every step of `scripts/ci-local.sh` with caches warm,
then tested the one change everybody assumed was the win. **Two of the three
proposals landed; the big one was rejected by its own measurement.**

### Warm gate: ~3–4 minutes, and the compile caches already work

| leg | step | warm |
|---|---|---|
| macOS | fmt, clippy, rustdoc, msrv, doc/evidence checks, script tests | 0–2 s each |
| macOS | build under `-D warnings` | **0 s** — the flag switch recompiled nothing; cargo keys it correctly |
| macOS | `cargo test` | 80 s |
| Linux | tar copy + `chown -R` of the 7.7 GB volume | 1 s |
| Linux | `rustup component add` ×2 + toolchain install | ~10 s, re-downloaded every run |
| Linux | build, clippy, msrv | **0 s** |
| Linux | `cargo test` | ~80–90 s |

**The floor is test execution, not compilation.** Two suites dominate and neither
is CPU-bound: `terminal_loss` 23.7 s and `session_supervision` 14.1 s.

### The prediction I got wrong, and the retake that corrected it

I argued those two numbers were contaminated because they were sampled at load
5–6 with ~48 processes live — practice §40's documented trap, which had just
produced a false Windows FAIL for me in the same session. **The retake at load
2.6 returned 14.1/14.1 s and 23.7/23.7 s — identical to the loaded sample.**

They are **timer-bound, not load-bound**: `PATIENCE` at 20 s, `SETTLE`, and the
hangup/keystroke deadlines set them. §40's trap is real for *pass/fail* on pty
tests and does not generalise to their *duration*. The ~160 s floor stands, and
the way to move it is shorter deadlines or `nextest` parallelism, not a quieter
machine.

### Worktree cache seeding: proven sound, then rejected on cost

This is the result worth keeping. The proposal was to seed a new worktree's
`target/` from `main` with an APFS copy-on-write clone, because worktree gates
build cold. It was gated behind a four-point non-vacuity proof — a cache change
to the gate *is* a gate change — and **it passed the proof**:

| run | planted failure present | verdict |
|---|---|---|
| cold | yes | FAILed on exactly the planted test, both platforms |
| seeded | yes | FAILed on exactly the planted test, both platforms |
| cold | no | PASS |
| seeded | no | PASS |

Sound. And then rejected, because the numbers did not survive contact:

- cold gate **135 s**, seeded gate **122 s** → saves **13 s**
- seeding costs **54 s** (APFS clone of an 18 GB `target/`) + **34 s** (Linux
  volume copy) = **88 s**

**88 seconds spent to save 13.** A cold worktree only costs ~40 s per platform on
this machine, because the dependency caches were never the problem — the test
execution floor is. Recorded in `ci-local.sh`'s header so nobody re-proposes it.

**The transferable part: the proof and the cost decision are separate gates, and
passing the first does not carry the second.** It would have been easy to land a
sound change that made the gate slower overall. Ask for the measurement even
after the correctness argument succeeds.

### What did land

- `[workspace.metadata.ci] toolchain = "1.98.0"` in `Cargo.toml`, read by both
  `ci.yml` and `ci-local.sh`. **Both gates floated independently before this** —
  `rust:latest` and `dtolnay/rust-toolchain@stable` are different distribution
  paths with different lag, so the local gate could silently test a different
  compiler than CI claimed. This is a correctness fix wearing a cache fix's
  clothes, and it copies the MSRV job's existing "declared once" precedent.
- rustup home in a `glasshouse-ci-rustup-<version>` volume, shared across
  worktrees — toolchains are not source-dependent, so this is safe to share.

### The finding that came out of it, and it is a product defect

Four Linux gates produced **two red runs in `gateway::conformance`** on a tree
whose lib suite otherwise passed twice. Chased to a real mis-attribution bug in
`record_routing_observation` — see
`.agent-runtime/defect-routing-observation-misattribution.md`.

**It was never a failing gate.** Batch 45 went green on all three platforms and
this family is ~50% red on Linux, so that batch simply won its coin flips. A
parallel session running the gate four times for an unrelated reason found what
one green run each could not — which is an argument for repeated runs on the
platform that is not the developer's own.

## 2026-08-30 — cost per box across four days, and the first regime change

Commissioned as `GH-WORKFLOW-EFFICIENCY` after the user's judgement that
*"the KPI done requirements in the capability map per hour work and our token
spent is kinda bad."* Ruling and rules in practice **§86**; the numbers and the
commands that produce them are here.

### Method — do not trust a checkpoint's claim about itself

Every box count is read from the map at the commit, not from a handoff:

```sh
for r in $(git log --format=%h -- docs/product/capability-map.md); do
  echo "$(git log -1 --format=%ad --date=format:'%Y-%m-%d %H:%M' $r)" \
       "$(git show $r:docs/product/capability-map.md | grep -c '^☑')"
done | tac
```

`☑` is the tick; `- [x]` matches nothing in this map. Tokens are
`python3 scripts/usage-snapshot.py --glasshouse`. "Active hours" sums the gaps
between consecutive commits under one hour — a proxy that undercounts long
unattended worker runs, so the boxes/h column is an upper bound on productivity,
not a lower one.

### The table

| day | output | cache-create | boxes Δ | active h | boxes/h | **out per box** | cc per box |
|---|---|---|---|---|---|---|---|
| 2026-08-27 | 13.62M | 34.8M | +239 | 18.1 | 13.2 | **57k** | 146k |
| 2026-08-28 | 6.24M | 20.8M | +57 | 6.7 | 8.5 | **109k** | 365k |
| 2026-08-29 | 14.06M | 37.5M | +112 | 16.0 | 7.0 | **126k** | 335k |
| 2026-08-30 | 7.30M | 27.2M | **+9** | 8.2 | 1.1 | **811k** | 3.02M |

**The step is 1.9× → 1.2× → 6.4×.** Difficulty of the remaining boxes rises
smoothly by construction and explains the first two. It does not step 6.4×
overnight.

Cost per box from this ledger's own earlier rounds, for scale: batch 35 $1.04,
36 $1.34, 37 $1.00, 39 $1.44, 40 $2.51, 41 $1.67, 45 ~$53 for 23 boxes = $2.30.
**The 2026-08-30 session recorded in `handoff.md` as batch 55 spent ~$35 of
worker compute across four workers for one closed box.**

### The implementation-to-investigation ratio, and where it does *not* support the framing

    git log --format=%H --after="$d 00:00" --before="$d 23:59" -- <path> | wc -l

| day | commits | process-docs | scripts | code | map | process-doc : box-advancing |
|---|---|---|---|---|---|---|
| 08-27 | 71 | 43 | 20 | 31 | 23 | 2.0 |
| 08-28 | 39 | 24 | 2 | 9 | 11 | 2.2 |
| 08-29 | 110 | 52 | 27 | 44 | 30 | 1.9 |
| 08-30 | 57 | 36 | 10 | 24 | 13 | 3.3 |

**The volume of investigation barely moved.** If meta-work quantity were the
disease that row would have exploded, and it did not. What moved is the **yield
of each box-advancing commit: 11.4 → 5.2 → 4.2 → 1.5**, an 8× fall while the
count of such commits stayed flat (21, 11, 27, 11).

### What actually changed on 08-30

`docs/process/refusal-register.md`: **280 lines → 969**, +689 in one day, which
is **61% of the day's entire `docs/process/` growth** (+1133).

    h=$(git log --format=%H --before="2026-08-29 23:59" -1)
    git show $h:docs/process/refusal-register.md | wc -l   # 280
    wc -l docs/process/refusal-register.md                 # 969

And the decisive single instance: **`1d708ca` at 17:09 proved a recorded blocker
false and unblocked 38 lines.** Over the next 3h45m the map went 813 → 808 → 809.
Nothing was dispatched against those 38 lines.

**§83 was written on 08-29** — *"refusals are not an archive, they are the input
to the next package"* — **and 08-30 produced the largest refusal-documentation
burst in the project's history.** That is the finding: prose does not bind, which
is why §86's output is a numeric trigger and a table in `CLAUDE.md`.

### The counterweight, computed rather than assumed

Investigation is not waste and the record says so. Four audits have un-ticked
**ten boxes** — `c7eccb1` −2, `889da59` −1, `64a0d87` −2, `bd81e04` −5 — at a few
dollars of read-only worker each. Ten is 1.2% of 809 ticks, and correcting them
protects the meaning of the other 799, which is this project's whole product.
Batch 45's six recons cost ~$11 and assessed 255 lines.

**This session's −4 map delta is five corrections, not five failures**, and
reporting it as a loss would be the same error in the opposite direction.

### The orchestrator remains the largest single consumer, and the gap widened

`python3 scripts/usage-snapshot.py --glasshouse`, 2026-08-24..30:

| | output tokens |
|---|---|
| all Glasshouse dirs (213) | 60.22M |
| **main checkout — the orchestrator** | **21.98M (36.5%)** |
| next seven worker directories combined | 3.56M |

**6.2× the next seven combined.** §74 measured 13.7M vs 3.5M on 08-24..28; the
ratio has grown, not shrunk. The one place where the orchestrator's misses are
recorded rather than invisible is the un-tick count above: **all ten were found
by audit workers after the orchestrator had read the diff and ticked the box**,
and all ten are `cluster-b.py`'s mechanical shape. That is the specific reading
§86 moves off the orchestrator, and it is the only one, because it is the only
one with a measured miss rate.

### Open questions this entry creates

1. **Does the 250k out-per-box trigger fire on a genuinely hard batch?** It is
   set at 2× the worst healthy day. If it fires on a legitimate red-risk round,
   raise it once and record why — do not disable it.
2. **Does Green-tier skipping cost a defect?** Track every box closed at Green
   and check whether any is later un-ticked. The un-tick count is 10; if Green
   is wrong, that number moves and the tier is falsified by its own metric.
3. **The ledger stopped at batch 45.** Batches 46–55 have no entry here, which
   is `CLAUDE.md`'s "add every batch to its ledger" going unenforced for ten
   batches — and they are exactly the batches where throughput fell. The next
   orchestrator owes the batch-46..55 rows or an explicit decision to stop
   requiring them.

## 2026-08-30 — package sizing, and what an integration costs regardless of size

Companion to the entry above and to practice **§87**. That entry priced a *box*;
this one prices a *package*, and the two numbers only mean something together.

### Boxes per package

**Batch 55: thirteen packages, ten boxes closed — 0.77 boxes per package.**
Package count is the batch's dispatched packets and box count is the map delta:

    ls .agent-runtime/packet-*.md                                  # packets on disk
    git show <rev>:docs/product/capability-map.md | grep -c '^☑'   # at each end

**The outlier carries the rule.** `GH-SESSION-CONTEXT-DOOR` closed **five boxes
(1161–1165) in about 66 lines**, because those five map lines are fields of one
`SessionContext` rendered by a single `store.context(&id)` call — one mechanism,
five lines. `GH-AUTO-ROUTING-MODEL` was scoped the same way afterwards: six
lines against one existing selector. §87 turns that into a target of **3–6 boxes
per implementation package** and names the three map shapes that find such a
mechanism before any code is opened.

### An integration is priced by files touched, never by boxes closed

Five integration logs survive from this session. The blast radius each one ran:

    cd .agent-runtime
    for f in integrate-*.log; do
      printf '%-20s %-4s %s\n' "$f" \
        "$(grep -c '^test result:' "$f")" \
        "$(grep -o 'finished in [0-9.]*s' "$f" | awk '{s+=$3} END {printf "%.1fs", s}')"
    done

| log | test targets | reported test time |
|---|---|---|
| `integrate-cc.log` | 41 | 122.9 s |
| `integrate-gfb.log` | 53 | 148.8 s |
| `integrate-probe.log` | 10 | 33.9 s |
| `integrate-ros.log` | 46 | 105.6 s |
| `integrate-scd.log` | 56 | 120.2 s |

**Four of the five ran 41–56 targets, and package size did not predict which.**
`integrate-scd.log` is the sharp case: **3 files changed, 161 insertions — and
56 targets, the largest run here.** A small package pays what a large one pays,
so ten boxes shipped as five 2-box packages buys five of these where two 5-box
packages buys two.

**Two limits on this table, stated rather than smoothed.** The `reported test
time` column sums the `finished in Ns` lines and therefore **excludes
compilation**; the logs carry no elapsed-time line, so this ledger records no
wall-clock cost per integration — `time scripts/integrate.sh` if a later session
wants it. And logs are overwritten by name, so **five files is a floor on the
number of runs, not a count of them**; batch 55's seven integrations is the
session's own figure, not one re-derived here.

### The ledger gap is itself the finding

    grep -o '^## Batch [0-9]*' docs/process/orchestration-measurements.md | tail -1
    # -> ## Batch 45

**Batches 46–55 have no entry**, and they are exactly the batches across which
output-per-box went 57k → 811k. The instrument that would have shown the regime
change on day one was not being read, so the user found it on day four. This is
the third open question of the entry above, restated as a measurement: the cost
of the missing rows is the three days of delay, not the minutes the rows would
have taken.

### Open questions this entry creates

1. **Does the 3–6 box target survive contact with a phase whose open lines are
   genuinely independent?** It is a target, not a gate. Record the first package
   that should legitimately have been 1 box and say why, rather than stretching
   it to three.
2. **What is the wall-clock cost of one `integrate.sh` run?** Unknown here.
   `time scripts/integrate.sh <names>` once and add the row; until then the
   sizing argument rests on the target count alone, which is enough for the
   ordering but not for a budget.

## Batch 56 — seven workers at once, 79 lines, sized by mechanism at Fable tier

Dispatched 2026-08-31 ~02:30–02:45 by the first Fable 5 orchestrator session,
under the user's instruction to use Fable for larger packages, go more parallel,
and not surface decisions solely because they are complicated.

| worker | model / effort | lines | shape |
|---|---|---|---|
| launch-classifier | Fable xhigh | 21 | join + build: router request schema, classification on the acting launch path, tier as hard constraint, routing latency |
| mcp-server | Fable xhigh | 11 | build over an existing seam: stdio MCP transport onto `api::protocol::Request` dispatch |
| subscription-pressure | Fable xhigh | 11 | build: capacity bands and reset proximity reach the session router |
| routing-economics | Fable xhigh | 17 | producers for the six 34C refusals (reliability, latency, RPM), fallback chain, local-only, overhead report |
| routing-outcome | Opus high | 5 | Cluster B join: `task_outcome` → evaluation rows; RC-B ruled |
| user-control | Opus high | 9 | flags + evidence + two seams (mute, input precedence) |
| failure-taxonomy | Fable xhigh | 5 | ruling + migration 16: failure classes from framing, joined the round late as a declared co-editor of database.rs and evidence.rs |

**What this batch tests.** (1) Whether a 10–20-line package at Fable tier holds
together — batch 55 averaged 0.77 boxes per package; the mechanism-sized outlier
closed five. (2) A **six-way** co-edit on `main.rs` (previous record: three-way,
clean) — every packet was told to keep `main.rs` to the call site and put logic
in its own module. (3) Whether two rulings the previous orchestrator parked for
the user (Phase 43 MCP scope; Phase 51 RC-B outcome learning) hold when made
from the tree — both are recorded in `design-decisions.md` and each has a
package building on it.

**Expected refusals, stated before dispatch so they cannot be counted as
surprises:** 1610 (1294's shape — no completion signal), 1419/1436/1439 (no
per-model price table), and 1834 unless launch-classifier's tier carrier lands
first. Outcomes and tokens per closed box: **to be filled at integration.**


## Batch 56 outcomes — seven workers, 79 lines, 65 boxes, 824 → 889 in one morning

Integrated 2026-08-31 between 10:00 and 13:30 Europe/Berlin, one worker at a
time (every one co-edited `main.rs`), each with its own blast radius on the
merged tree.

| worker | tier | lines | closed | refused | open | integration |
|---|---|---|---|---|---|---|
| mcp-server | Fable xhigh | 11 | 11 | 0 | 0 | `integrate.sh`, clean |
| routing-economics | Fable xhigh | 17 | 13 | 4 (1419/1436/1439 no price table; 1440 no subscription classifier) | 0 | `integrate.sh`, clean |
| routing-outcome | Opus high | 5 | 1 | 0 | 4 (links named) | `integrate.sh`, clean |
| failure-taxonomy | Fable xhigh | 5 | 4 | 0 | 1 (1334 on tool rounds/repairs) | 3-way, clean; two overflow patches applied |
| subscription-pressure | Fable xhigh | 11 | 9 | 1 (1610 — 1294's shape) | 1 (1577 background half) | 3-way, 3 keep-both blocks |
| launch-classifier | Fable xhigh | 21 | 18 | 0 | 3 (no tier-ceiling producer) | 3-way, 7 blocks + one rustdoc link |
| user-control | Opus high | 9 | 9 | 0 | 0 | 3-way, 12 blocks, one hand merge (the routing-off wrapper around the classifier) |
| **total** | | **79** | **65** | **5** | **9** | |

**Boxes per package: 9.3** (batch 55: 0.77). **Output tokens per closed box:
~150k** (today's account-wide 9.69M output at 56 closed by 12:30; the day's
final figure is in the next checkpoint) against 811k the day before and the
250k ceiling. Every expected refusal named before dispatch was the refusal that
came back; no worker misreported; every report carried its five artifacts, and
two workers found and fixed defects in their own first drafts through their own
mutations (user-control's duplicate check; assumption-guardrails' constant-
derived assertion).

**What the six-way co-edit cost.** Integration was serial and hand-merged four
times out of seven; the merges were exactly the ones the workers' §77 notes
predicted, and the one that needed judgement (user-control's wrapper around
launch-classifier's block) was spelled out in both reports. Two ripples were
not predicted by any packet: a rustdoc `-D warnings` private-item link, and
migration 18's literal `version, 17` pins in three test files outside every
package's blast set (`memory_provenance`, `memory_store`,
`evaluation_observations`) — now a memory note and a packet rule.

**Three parked product questions were answered from the tree, not escalated**,
under the user's instruction not to surface things solely because they are
complicated: Phase 43 (MCP as a transport over the existing door), Phase 51
RC-B (the harness's own `TurnEnded` verdict), Phase 33 (framing is not
content). Each has a package that landed the same day.

**Batch 57** (dispatched 11:00–13:15, seven workers, 115 lines): assumption-
guardrails (43, reported: 42 closed), implementation-policy (30),
tracked-knowledge (7, reported: 7 closed), cmux-presentation (14),
memory-commits (8), evaluation-producers (5), tier-ceiling (8). Three of them
carry migrations 19, 20, 21 in that integration order.


## Batch 57 outcomes — 2026-08-31, seven packages, one orchestrator hand-off mid-batch

Dispatched by the Fable orchestrator (six, Opus high / Fable xhigh) and by its
Opus successor (one, Sonnet medium). Integrated by the successor in one
session: 938 → 953 committed (`baf6be0` +42 assumption-guardrails, `1c7d2c0`
+14 cmux-presentation, `1745` scope tests), and **52 more staged behind one
gate** — memory-commits 7, implementation-policy 30, evaluation-producers 4,
tier-ceiling 7, support-work-economy 4 — for 1005/1280 if it holds.

| package | model | lines | boxes | mutations | notable |
|---|---|---|---|---|---|
| assumption-guardrails | Fable xhigh | 43 | 42 | 59 killed, 1 survived → dead check deleted | 1044 refused (rolls nothing back) |
| cmux-presentation | Fable xhigh | 14 | 14 | 20/20 | migration 20; merged its peer's undo lines itself |
| memory-commits | Opus high | 8 | 7 | 9/9 | migration 21 (renumbered from 19 at integration); 1152 open, unmutated by design |
| implementation-policy | Opus high | 30 | 30 | 30/30 | found `mutate.sh --script`'s false-KILLED defect; closed the real SURVIVED with a test |
| evaluation-producers | Opus high | 5 | 4 | 6/6 | 1854 open (producer already existed; `load_all_dated` instead) |
| tier-ceiling | Opus high | 8 | 7 | 9/9 | refused OBJECTIVE 3 as decorative wiring — verified and upheld |
| support-work-economy | Opus high | 5 | 4 | 6/6 | 1608 refused, Cluster Q, with a tripwire |
| cmux-scope-tests | Sonnet medium | 1 | 1 | 1/1 (orchestrator-run) | 1745 reachable only after migration 20 landed |

**What integration cost, and where.** Every one of the seven co-edited
`main.rs`, so merges were serial — 39 + 40 + 15 + 1 + 0 + 0 conflict blocks — and
**every seam between two workers' additions in one file lost a closing
delimiter to the shared tail** (nine hand repairs across `database.rs`,
`main.rs`, `cli.rs`, `mcp.rs`, `unix.rs`). Three defects were found by the gate
and not by any report: `UNDO_19` not reaching past migration 20, a credential
pin carrying a stale draft column name, and the pin list's order. One gate for
the batch of five instead of five gates — practice §87's trap 2 applied.

**Tools fixed on the way:** the co-edit stop hook (invalid JSON on every
multi-file firing, `710394d`) and `mutate.sh --script` (false KILLED on every
deletion row, `efd6e65`). Register drift corrected: P2 closed, `with_purpose`
exists, 1129 refused in-source, 514 wrongly offered by the handoff.


## Batch 58 outcomes — 2026-08-31, three Fable packages in one hour

Dispatched sequentially the hour batch 57 landed (`d46ed16`), all three
Fable 5 at xhigh with mutual co-edits declared on `session.rs`, `evidence.rs`
and `main.rs`. All three reported within ~50 minutes. **21 lines, 21 boxes,
39 mutations killed, 0 survived.**

| package | lines | boxes | mutations | notable |
|---|---|---|---|---|
| escalation | 35C 1559–1566 | 8 | 12/12 | shipped without gate lines; integration's gate stands in and the evidence says so |
| affinity | 36 1581–1588 | 8 | 16/16 | one number kept, seven facets behind it; four packet anchors corrected |
| route-correlation | 33C ×4 + 1852 | 5 | 11/11 | the reader `phase-33c.md:101` named as missing; EXPECTED FILES omitted its only production caller |

Integration: two clean applies, one import-union conflict, one real
cross-package seam (`session_affinity`'s widened signature) — found by
`cargo check` on the merged tree, not by any report. Gate: one for the three.

## Batches 59–61 outcomes — 2026-08-31, an orchestrator hand-off Opus → Fable mid-board

Batch 58 landed at `1362f51`. What followed was integrated across the hand-off:
the Opus 5 orchestrator committed 1317 (`afd9ebb`), the two prove-it packages
(`1bd39b4` + `d584ad3`, ten lines the code already satisfied) and 1480
(`eb44707`), then handed off HOT with two workers live and one patch staged;
the Fable 5 successor integrated the staged patch (`1ba3f80`) and refilled the
board. **13 boxes across the span; every mutation reported KILLED.**

| package | lines | boxes | mutations | notable |
|---|---|---|---|---|
| rate-limit-scope | 33 1317 | 1 | see `phase-33.md` | a rate-limit failure read as provider-wide or model-specific from rows the router already keeps |
| prove-it-misc + prove-it-39 | 1174→open, 1533, 1551, 1212, Phase 39 ×6 + | 10 | see `d584ad3` | `1bd39b4`'s message outran its evidence step; `d584ad3` repaired it. 1174 became the RED finding `hook-extraction-detach` |
| tier-outcomes | 34F 1480 | 1 | 3/3 | committed by a background chain the predecessor armed before handing off; the successor verified it landed rather than redoing it |
| wire-file-memory | 28 1140, 1143 | 2 | 4/4 (+1 re-run by the integrator) | patch staged by the predecessor, applied clean by the successor; `phase-28.md` created — the phase had no entry |

Hand-off cost, measured: the successor's first useful act (a nudge to a stalled
worker) came ~25 minutes after launch, most of it reconciling a second live
orchestrator in the same checkout — the predecessor stayed up as the user's
requirements scribe and committed Phase 56A by pathspec while the successor's
patch sat staged. Two mechanical traps surfaced and are in memory: `cmux send`
with `\r` does not submit in a Claude Code pane (a worker sat idle 40 minutes
with the notice in its prompt), and a pathspec commit sweeps any uncommitted
edit to the same file. Fill: three Phase 55 prove-its (tests-only, Green), one
Fable translation package (T1) and one Fable entitlement package queued behind
`subscription-rules`.

## Batch 62 outcomes — 2026-08-31, the Fable orchestrator's first wave: one Red finding and fifteen criteria

Dispatched 16:0x–16:3x with six workers live at the peak (two Fable, one Opus, three Sonnet). Three landed in this wave; **16 boxes, every reported mutation KILLED.** Load average reached 38 on 12 cores with six cold `target/` builds — the seventh worker waited for it to fall to 6.

| package | lines | boxes | mutations | notable |
|---|---|---|---|---|
| hook-extraction-detach (Opus, Red) | 31 1174 | 1 | 2/2 | the packet's central hypothesis (a detached extraction) was **refuted with measurement**; the real defects were a macOS `O_NONBLOCK` accept-inheritance flake in the repro's fixture and a silently lost extraction (logging off by default) — fixed by a stderr notice following an existing precedent |
| prove-it-v1-routing (Sonnet, Green) | 55 1930–1937 | 8 | 8/8 | two SURVIVED on the first pass and the *tests* were strengthened (warm and fresh in one candidate list; the unit pinned beside the number) |
| prove-it-v1-sessions (Sonnet, Green) | 55 1917–1922, 1939 | 7 | 7/7 (+2 SURVIVED, retargeted) | `HarnessAdapter::id` has no production caller; `IntegrationId::slug` is the seam. **1921 was reported `open` because the packet paraphrased the criterion with a clause the map does not contain** ("with its owning orchestrator"); the orchestrator overruled its own packet |

Two packet errors were the orchestrator's, and both are the same shape: per-line guidance in a prove-it packet is read by the worker AS the criterion. Quote the map line's words in the objective; put the suggested seam in a separate sentence marked as a suggestion. The Green tier's own rule held: neither prove-it needed a diff read — the ruling came from the report's artifacts and one grep each.

## Batch 63 outcomes — 2026-08-31, four disjoint packages in one integration

One `integrate.sh` call, one blast radius (89 targets over 19 changed files), one commit (`e03f252`). **9 boxes; every mutation KILLED.** The one red was the pinned pairing tripwire whose own doc comment names exactly this event; the integrator applied the report's quoted one-line fix on the merged tree and the lib target went green (1734).

| package | lines | boxes | mutations | notable |
|---|---|---|---|---|
| gateway-translate (Fable, Red) | 56 1949 (+1948/1950/1956 PARTIALLY VERIFIED) | 1 | 4/4 (first batch's verdicts void on an exit-127 `--test-cmd` misuse — detected and re-run by the worker) | codecs around one canonical form; byte-for-byte relay kept and tested; the worker's honest `open` on the three every-pair lines accepted |
| harness-efficiency (Sonnet, Amber) | 56 1951 + 39 1629 | 2 | 4/4 | tokens carried only where rows have them; `print-zero-for-null-tokens` KILLED |
| prove-it-v1-orch-memory (Sonnet, Green) | 55 1925–1929, 1938 | 6 | 6/6 | Phase 55 now 21/23 |
| extract-model-error-wording (Sonnet, Green) | (1174 wording; no box) | 0 | 1/1 | dispatched from the Red report's §8 within the hour |

Recurring defect, now in memory (`detached-gates-die-without-exit-lines`): three detached worker gates died mid-run without exit lines today; both affected workers idled on a `tail -f` that could never fire and looked like "done, no report". The recovery each time: `ps` for the real process (excluding the watchers that match the name), then a `cmux send` + `send-key Enter` nudge.

## Batches 64–66 outcomes — 2026-08-31 evening, the 56A critical path and the translation matrix

| package | lines | boxes | mutations | notable |
|---|---|---|---|---|
| entitlement-pool (Fable, Red) | 56A 1962–1964 (+1973 held) + 1947's consumer | 3 | 6/6 | four packet errors, all the orchestrator's; the worker's own limit held 1973 open |
| prove-it-54a (Sonnet, Green) | 54A 1899–1907 | 9 | 9/9 | 1908 open — its words name CI runners |
| gateway-translate-t2 (Fable, Red) | 56 pairs 2–3 (lines stay open by quantification) | 0 | 4/4 | found protocol_fit asking the table BACKWARDS; prescribed both integrator fixes verbatim |
| protocol-fit-direction (Sonnet, Amber) | (fix; no box) | 0 | 2/2 | T1's shipped pairing finally classifies Translated |
| entitlement-env-scrub (Sonnet, Amber) | 56A 1973 | 1 | 2/2 | the held clause discharged; every launch path scrubs |

**13 boxes this span; 1067 → 1080/1305 (82%).** The dead-gate defect fired twice more (t2's blast, the scrub's) — the ps-then-nudge recovery is now routine at under two minutes each. The stale-workspaces watch caught its first forgotten pane fifteen minutes after being written.

## Batch 67 + the investigation swarm — 2026-08-31 evening

Broker (56A-3) landed `0587f4a` (1953/1966/1967/1968/1969) — but only after the
user, at 14% weekly quota, asked for an investigation swarm to burn it before
midnight. **12 read-only agents** (6 Opus adversarial, 6 Sonnet quality), each
writing findings + a proposed-fix diff to its OWN gitignored file (no
cross-contamination; a worktree can never collide with `.agent-runtime/swarm-*`).
**57 findings.** Ruled in `.agent-runtime/swarm-2026-08-31/ACCEPTANCE.md`.

The swarm paid for itself immediately: the adversarial routing agent found
`burn_urgency` rewarding a reset already in the past (+1.0), which inverts line
1967 — caught and fixed *in the same commit as the box it would have falsified*.
Six more high-severity defects accepted and queued (cmux `send --text` injection
past a documented guarantee; `deny_harness` typo silently voiding a deny rule
for want of `deny_unknown_fields`; a credential reaching `Debug` and
`glasshouse.log` unredacted; a dead memory dedup check; a restart-vs-resume
identity bug; translation streaming the wrong tool-call id). 26 quality findings
(atomic-write dup across 4 caches; a 1018-line `launch_session`; a `require(id)`
helper for 15 sites) batched into behaviour-preserving cleanup packets.

Two process notes: the swarm's concurrent load flaked one real-binary
subprocess test (`v1_1907`) during the broker blast — it passes isolated, the
classic §34 load flake; and an untracked helper (`swarm-collate.sh`) dirtied the
tree and made `integrate.sh` refuse until committed — the swarm findings dir
itself is gitignored, which is what keeps it collision-free.

## Batch 68 — 2026-08-31 late evening, the swarm's fixes dispatched eight-wide

The user's instruction was to spend the remaining weekly quota before the
midnight reset: *"delegate aggressively … if no work packages are left to give
out, spawn workers to find untested edge cases which could cause crashes … or
better yet do that in parallel while working on the newer packages."* Eight
workers dispatched in one wave, every partition file-disjoint and every
`FORBIDDEN FILES` block naming the other seven's files:

| package | tier | model / effort | source |
|---|---|---|---|
| session-restart-identity | Red | opus / high | swarm break/pty-session #1 |
| translate-stream-order | Amber | sonnet / high | swarm break/gateway-translate #1 |
| config-hardening | Amber | sonnet / high | swarm break/config-entitlements #1–#3 |
| memory-dedup-subject | Amber | sonnet / medium | swarm break/memory #1 |
| cmux-send-escape | Amber | sonnet / high | swarm break/cli #1 |
| memory-injection-bounds | Amber | sonnet / high | swarm break/memory #2–#5 |
| break-store-db | investigation | sonnet / high | NEW lane — the persistence layer the 12-agent swarm never covered |
| break-cli-surface | investigation | sonnet / high | NEW lane — CLI dispatch/parsing edges beyond break/cli's five |

**Six of the eight close no capability-map box.** They are defect fixes against
`.agent-runtime/swarm-2026-08-31/`'s accepted findings, so every packet carries
`lines: []` and records its proof under `gates:` instead — the facts-block
schema already allows this, and it keeps `evidence_from_report.py` from being
handed a box it must not authorise.

**Phase −1 was re-derived from current source for all six fix packets before
dispatch**, not taken from the swarm reports: `push_str("\\r")` still raw at
cmux.rs:492; `deny_unknown_fields` still 0 in config/mod.rs; `duplicate_key`
still uncalled in production; `consider_restart` still returning its `ended`
vector unchanged at runtime.rs:1353; no `canonical::Order` in the tree;
`MAX_OBSERVED_PATHS` absent and `break` still at inject.rs:368. Every defect
was live at dispatch time.

**A validator trap worth recording.** `validate_round.py`'s FEASIBILITY regex
ends `\s*(?::|[-—–]|$)`, and `\s*` crosses newlines — so a block whose
first body line is `- Producer:` has that bullet's dash consumed as the header's
own delimiter, and the check then reports `never names: producer` for a packet
that plainly does. Five packets failed this way at once. The fix is one lead-in
sentence between `## FEASIBILITY` and the first bullet; the skeleton
`new-packet.sh` emits passes only because its first body line starts `TODO`.

**Two investigation lanes, each naming its successor** (§86): `break-store-db`
→ `swarm-fixes-store`, `break-cli-surface` → `swarm-fixes-cli`. Both are
read-only on the tree but build and run the shipped binary against scratch data
dirs — the persistence lane is aimed squarely at the shapes unit tests cannot
reach (two processes on one database, SIGKILL mid-migration, a truncated file,
a schema version from the future), which is where every real Glasshouse defect
has been found.

### Outcomes

| package | result | mutations | notable |
|---|---|---|---|
| memory-dedup-subject | fixed | 1/1 KILLED | one `key` binding fixed both the dead check and the over-match |
| session-restart-identity | fixed | 3/3 KILLED | **corrected the packet**: the lifecycle written is `Failed`, not `Stopped`, because `consider_restart` returns early on `status.success()` — severity unchanged, since `guard_start` short-circuits on `is_live()`, not on the variant. One mutation tested *placement*, not presence |
| config-hardening | fixed | 1/1 KILLED | also closed `ProviderConfig::credential_env`'s same hole (declared as scope overflow; same file, met the packet's stated condition) |
| translate-stream-order | fixed | 1/1 KILLED | **corrected the packet's test location**: the named file's only fixture is single-open by construction and cannot produce the hazard; built a full-pipeline test over a real loopback `TcpStream` instead. Its mutation output shows call_A's fragment arriving under call_B's `item_id` |
| cmux-send-escape | fixed, **after a refused first attempt** | 1/1 KILLED | see below — the escaping approach was wrong and measurement is what showed it |
| memory-injection-bounds | 4 findings fixed | 2/2 KILLED | four separate unpatched-tree repros quoted |
| dedup-provider | 4 findings fixed | 1 SURVIVED → then KILLED | **the valuable mutation**: the crash-safe temp-then-rename pattern had no test that could tell it from a direct write. Reported as a finding, then a directory-permission test written that does kill it. Also caught a packet error — the "fifth atomic-write site" this orchestrator cited is inside a test, not production |
| break-store-db | 3 findings, all reproduced | n/a | a **single flipped bit permanently wedges a project**: `read_record`'s `get_unwrap` panics, `PRAGMA integrity_check` still says `ok`, and every later `sessions`/`status` exits 101 until the file is hand-edited |
| break-cli-surface | 2 findings + 12 clean negatives | n/a | `shim --name` writes an executable outside `--dir` (`check_name` guards `harness` and `profile` but not `name`, and `Path::join` discards the base for an absolute argument) |

**The ruling of the batch: escaping cannot carry a backslash through cmux.**
`cmux-send-escape` implemented the swarm's proposed fix (double the
backslashes) and honestly flagged that it had not verified cmux unescapes them.
Measured against a live pane: a literal `\r` **does** submit (the finding is
real); `A\\B` renders as `A\\B`, so doubling is **not** collapsed; and
therefore `\\r` **still submits**, leaving a stray backslash behind. cmux has
no escape-of-escape, so the only correct move is refusal — now
`CmuxError::PayloadHasBackslash`, whose message never echoes the payload.
This is §88's *"verify where the report names its own thin spot"* paying for
itself: the worker was right to doubt, and unit tests could never have shown it.

**Gate reliability under self-inflicted load.** Two workers' `blast-radius.sh`
runs came back non-zero, and both attributed correctly per §34 — failures only
in files neither had touched, each passing alone under `--test-threads=1`, one
across four repeated runs at load average 7.7–9.7. Measured here at the same
moment: **12.80**. Eight concurrent workers plus an integration blast radius
saturates this machine, and a saturated machine makes gates lie. The
orchestrator therefore **held two validated packets rather than dispatch into
unreliable gates** — the first time this batch that the constraint was machine
capacity rather than review capacity, and worth recording as the real ceiling
alongside §74's review-collision one.

## Batch 70 (2026-09-01) — the ruling's whole program landed in one day

Six packages: four Sonnet + one Opus dispatched as one validated wave (all
four packets cross-validated, no file conflicts, two declared coedits), plus
one Sonnet infra package by user instruction mid-batch. **Twelve map boxes
closed** — 372 (both clauses), 34F×8 (the model-capability record), 56A×3
(fallback order, per-entitlement rules, broker e2e — **Phase 56A 13/13**) —
plus two crash-class defects (zero-byte DB wipe, the fifth tilde input) and
the two-lane blast radius. 1096→1099 by evening, 84%.

**Tier evidence.** Sonnet-high closed a NINE-line package (tier-axis) with
exactly one review-caught defect — the 1482 scoping leak, fixed by the same
worker within the hour of the ruling. Sonnet-medium handled the defect pair
including a concurrency interaction the packet's sketch missed (and flagged
its scope overflow instead of shipping it silently). Opus-high on the Red
broker package produced the batch's best report: six KILLED mutations, two
packet errors caught (one contradicting a closed contract — resolved
correctly without inverting it), and its blocked sub-step delivered as an
exact two-file patch the orchestrator landed verbatim (one rename), tested,
and mutation-killed. **The default is still not Opus** — but Red kept
earning it.

**What review caught that gates did not:** the 1482 context-blind leak
(reading the decision's diff); the 372 tripwire firing by design (the
worker's STOP, the orchestrator's flip). **What gates caught that review did
not:** four premise-stale tests encoding "an unpinned launch is native,"
found only on merged trees — the exact cross-patch class integration exists
for.

**Orchestrator error, twice, same shape:** gate-failure enumeration by
truncated grep — a multi-line flake family hid a real `entitlement_pool` red
(shipped in e8d0823's wrong gate claim, corrected in 689bc03) and a rustdoc
red (shipped in 689bc03, fixed in 9bea0e5). Rule now in memory: count
`test result: FAILED` lines first, list every failing TARGET, attribute per
target — and rustdoc/clippy are failures the test-grep never sees.

**Open question answered a little:** review collision (§74's ceiling) held
at five workers only because their finishes staggered naturally; the two
coedit members finishing within minutes of each other still serialized ~40
minutes of integration wall-clock. The two-lane blast radius attacks the
other half of that cost; the fixture-reuse successor (Gatekeeper) is named
and unbuilt.

## Batch 72 (2026-09-01, evening) — the bridge lands, and the gate economics go from paper to plumbing

Five Sonnet packages (bridge, v1_1907 race fix, argv-log hoist, entitlement
tranche, score-terms in flight) plus the orchestrator's integrate.sh wiring.
**Six map boxes closed** (57A complete: 1991–1996 — a Glasshouse session can
now run Claude Code behind the context firewall in shadow mode). 1112→1118.

**The measured before/after of the SDLC change:** the bridge paid the last
~40-minute inline full sweep; the entitlement tranche, integrated an hour
later under integrate.sh's new targeted default, paid **~6 seconds** (four
distance-zero targets + rustdoc, 19 full-trace targets deferred to the
wave's trailing background sweep). Worker spin-up: persistent caches held
13.4s→1.3s on re-dispatch.

**Empirics beat recon twice:** the bridge's live capture showed real Bash
payloads carry no exit_code and failing Bash never reaches PostToolUse —
correcting both the recon's "uniformly text" claim and the core's
conservative-but-wrong Some(0) gate. And a one-settings-document collision
(a second --settings silently discards the first) was avoided because a
pre-existing verified doc comment recorded it — evidence written down once
paying off months later.

**Worker-question protocol worked:** the tranche worker hit the
machine-busy rule mid-work and ASKED (picker prompt) instead of either
waiting silently or violating it; the answer was policy, not judgment
(worker full sweeps are the trailing gate's job now). One genuine mid-turn
generation stall (tokens frozen 3+ min) was interrupted and resumed clean.

**Flake ledger:** v1_1907's TCP race is FIXED (blocking accept, 20/20).
settings_persistence and events_lifecycle remain the known in-lib/fixture
families (each green twice alone today); session_supervision showed
load-race failures on two trees — watch whether it recurs on a quiet
machine before suspecting the fixture conversion.

## Batches 73–75 (2026-09-01, evening) — three small packages, and the first day the ratio argued back

Written by the Opus 5 orchestrator that inherited the board at `4f0c1cf`.
**These rows were missing when this session started** — batches 73, 74 and
75 had all landed and none had a ledger entry. That is trap 6 firing
exactly where §87 predicted it would: the span with no rows is the span
where the ratio moves.

**What landed.** Map `1118 → 1129`, eleven net boxes across four commits:

| commit | package | boxes | shape |
|---|---|---|---|
| `25deef0` | `score-terms-35b` | 1537, 1538 (+1534 held open) | provider health and structurally-partitioned marginal cost enter candidate scoring |
| `984f503` | `subscription-estimator` | 1244/1245/1246/1250/1251/1254 | 32C's estimator, derived on read — no table, no migration |
| `9cc1180` | `style-cache-declaration` | 618 | a style change declares its measured cache cost |
| `4f0c1cf` | `session-style-surface` | 619, 620 | restyle warns before costing a warm session; the 9K surface cluster closes |

**The number that matters, and it is not flattering.** Output tokens per
net closed box:

- **two-day window (08-31 + 09-01): ~96k/box** — 29.41M output over
  822 → 1129 boxes. Comfortably under §86's 250k ceiling.
- **2026-09-01 alone: ~254k/box** — 4.31M output over 1112 → 1129 (17
  boxes closed today across batches 72–75).

The two-day figure is inflated: the map was restructured on 08-31 (the
`^☑` total went 1509 → 1534 across that day's commits), so a share of that
+307 is re-marking rather than closing. **The honest reading of today is
the 254k one, and it sits on the ceiling rather than under it.** This
session therefore ruled that its next dispatch had to be implementation,
and declined a Phase 9K measurement-channel investigation (627–630, the
register's §83 "attack the channel" candidate) on exactly that basis. The
rule fired for the first time since it was written, and it fired correctly:
the alternative was an investigation package on a day already paying a
quarter of a million output tokens per box.

**Why the packages were small, and what that cost.** Three of the four
closed 1–3 boxes. `subscription-estimator` was the only §87-shaped one —
six map lines that were facets of one `estimate_subscription_headroom`
call — and it closed more boxes than the other three combined. Batch 75
split what was arguably one mechanism (the response-profile style surface)
across two packages and two integrations, paying trap 2's fixed integration
cost twice for three boxes.

**A trailing sweep survived an orchestrator handoff, which had not been
tested.** The wave-75 sweep (`blast-radius.sh --since 984f503`) was started
detached by the predecessor and was still running, three lanes wide, when
this session inherited the board. Its log lived in the predecessor's
scratchpad — a directory this session does not own but *can* read, since a
scratchpad is an ordinary path. The successor's instinct was to re-run the
sweep; that would have put **two full sweeps on one checkout**, the precise
condition the `never-run-two-integrations-at-once` memory records as having
made four gates lie. The duplicate was started, detected by
`ps -o pid,ppid,lstart -ax` within ninety seconds, and killed.

**Rule now, and it generalises past sweeps:** a handoff inherits *running
background jobs*, not just workers and worktrees. Before starting any
long-running job named in a checkpoint, `ps` for it first — the checkpoint
says what was started, never what is still running.

### Open questions this entry creates

- Is 254k/box today a regime change or three small packages in a row? The
  next batch answers it, and the answer is only visible if its row is
  written. Two rows, not one, is the whole instrument here.
- Does the 250k rule need a same-day arm? It is written as a two-day
  average, and a two-day average containing a map restructure hid a
  ceiling-level day. The rule's *spirit* fired only because the
  orchestrator computed both numbers rather than the one it was told to.
- `subscription-estimator` closed six boxes and left four (1247, 1248,
  1249, 1252, 1253, 1255 open at the time). This session packaged four of
  those six as `estimator-signals` and refused 1247 and 1253 at Phase −1 —
  1247 has nothing persisting a prior plan to compare against, and 1253's
  *"so the scheduler can improve"* has no consumer while nothing scores on
  the estimate. **Both belong in the refusal register**, and neither was
  there when this session looked.

## Batch 76 (2026-09-01, evening) — eleven boxes, three workers, and four orchestrator errors the process caught

The successor Opus session's first batch, inherited hot from the Fable
orchestrator at `4f0c1cf`. Map **1129 → 1140 (86%)**, eleven net boxes, nine
commits, three Sonnet-high Amber packages.

| package | boxes | note |
|---|---|---|
| `estimator-signals` | 1248, 1249, 1252, 1255 | 32C's estimator gains a learned reset window, a second horizon, an override and a disable switch |
| `firewall-reducer` | 1997–2003 | Phase 57's semantic rung; 17 → 24 of 27 |
| `pricing-channel` | **none — both held open** | mechanism + production caller landed; see below |

**Cost.** 2026-09-01 output totalled **5.36M** for ~17 boxes closed across
batches 72–76, ≈ **315k/box** — still on the wrong side of §86's 250k
ceiling, and the second day in a row it has read that way. The two-day
figure (~96k) remains flattered by 08-31's map restructure. **Treat the
same-day number as the real one until a clean two-day window exists.** This
session declined an investigation package on exactly that basis.

### The one that did NOT close, and why that is the entry's most useful row

`pricing-channel` built Phase 32G's price-metadata channel — `PriceTable`,
`load_from_dir`, and `expected_marginal_cost` learning to say *unknown*
instead of collapsing to free. Two mutations KILLED, six tests, clippy and
fmt clean. **Both boxes were held open anyway.**

Its own report named the reason in its limits: *"no production caller wires
`PriceTable::load_from_dir` into main.rs yet"* — `main.rs` was held by two
other workers and forbidden to it. That is `cluster-b.py`'s shape, the shape
behind **all ten** wrongly-ticked boxes in this project's history. The
script was run before any ruling and confirmed it: every reference lived in
the two new files or their own doc comments.

The orchestrator added the caller (in `session_router`, the one function all
three ranking paths share) and **still held both boxes**, because no one had
yet watched a `pricing.toml` change what the shipped binary prints — the
worker's proof stopped at the `SessionRouter` API, and a live
`glasshouse route --task` attempt produced no ranked candidate for an
unrelated profile-config reason and was abandoned after two tries rather
than dug into.

**This is the first time the shape was caught BEFORE the tick rather than by
a later audit.** Eleven instances, ten of them retrospective. The cost of
catching it early was one held package and one named successor; the cost of
the other ten was an audit worker each.

### Four orchestrator errors, every one caught by a mechanism rather than by care

Recorded because the pattern is identical in all four: **a section written
from scratch where a correct idiom already existed.**

1. **A premature "your gate has finished"** told to `estimator-signals`. The
   wait loop matched the *test binaries'* paths, not `blast-radius.sh`
   itself, so a gap between binaries ended it. **The worker checked `ps`,
   disputed it, and was right.** Wait on the script's own pid
   (`while kill -0`) — which the worker had already done correctly.
2. **A bare `scripts/blast-radius.sh` in VERIFICATION COMMANDS**, in the
   same section that said the full sweep was not owed. `estimator-signals`
   resolved the contradiction the expensive way: 4 changed files traced
   **802 symbols into ~90 test targets and 66 lib filters**, 25+ minutes,
   and the load produced **three reds that were all load** — including
   `handoff_lines` timing out at 20.05s while the main sweep passed that
   same target **1-of-1 in 2.82s at the same moment**. `firewall-reducer`'s
   packet, written by the *predecessor*, said `--targeted` and "your full
   sweep is NOT owed". The idiom existed; it was not copied.
3. **A one-sided co-edit claim.** The §77 ritual went into the new packet
   only; `firewall-reducer`, already running, was never told to claim the
   same files. The barrier read **1/1 claimants for a file two worktrees
   were both editing**, and the Stop hook caught it. Reconciliation was
   clean (hunks disjoint, closest pair 47 lines apart), but the bookkeeping
   was wrong. `validate_round.py` mechanises this — it demands a literal
   `COEDIT:` line in **both** packets — and the next round's two packets
   were written that way.
4. **A doc-comment-only change gated on rustdoc plus a compile.** `6117446`
   fixed three product sources that cited a process document; one of them
   was pinned by a **tripwire test asserting on the text of that very
   comment**, and the trailing sweep went red. §79's rule — *once a grep
   names a file, run its target; do not read it and decide* — was quoted at
   two workers by the same orchestrator that then broke it. Fixed forward
   in two commits (the second because the comment *explaining* the boundary
   rule quoted the forbidden path literal and tripped it).

**None reached main uncorrected.** A worker caught one, a hook caught one, a
validator now blocks one, and the trailing sweep caught one — which is the
trailing-sweep model working exactly as ruled on 2026-09-01: a red arrived,
the line kept moving, the fix landed two commits later.

### Two gates were red on main and nobody was running them

- **`check-doc-boundary.sh`**: three product sources cited
  `refusal-register.md`. Fixed by promoting both refusals into
  `design-decisions.md` as decisions about behavior, which is what they were.
- **`check-evidence-coverage.py`**: reported *"Phase 56A: 13 ticked boxes
  with no evidence entry"*. The evidence existed and was thorough; no
  heading spelled `Phase 56A` in the form the script matches. **Its own
  docstring records this false-positive shape biting twice before.** This was
  the third. Fixed on the documentation side rather than by loosening the
  check — 101/102 → 102/102, no box touched.

**Rule:** run every gate in CLAUDE.md's Verification list at the START of a
session, not before a commit. Two of six were red on inherited `main`, and
neither had anything to do with the work in flight.

### Open questions this entry creates

- Same-day out-per-box has now read ≈254k and ≈315k on consecutive
  measurements. Is that a regime change, or the cost of a session that spent
  heavily on inherited-gate repair and packet authoring rather than on boxes?
  The next batch's row is the only thing that answers it.
- Every one of the four errors was a from-scratch rewrite of an existing
  idiom. Would a packet **template** derived from the last accepted packet —
  rather than `new-packet.sh`'s empty skeleton — remove that class outright?
- `PriceTable::empty()` tripped `validate_round.py`'s seam check while being
  production-reachable as `Self::empty()`. The check greps one spelling. Two
  such near-misses in one session (this and the 56A heading) suggest the
  scripts' matchers deserve a pass of their own.


## The co-edit barrier under-reports, and the reason is not the packet (2026-09-01)

Recorded after the Stop hook caught an unreleased `main.rs` barrier three
times in one session. Two separate defects, and only one of them was fixed
by the obvious remedy.

**1. The declaration is enforced; the ACTION is not.** After the first
incident the fix looked clear — the §77 claim had gone into one packet and
not its peer's, so `validate_round.py`'s `COEDIT:` requirement (which
refuses a shared path declared in only one packet) was the mechanism. It
works: the next round's two packets both carried the line and both
validated.

**And the barrier still read `1/1` for a file two worktrees were both
editing.** `input-size-producer`'s packet contained
`scripts/coedit.sh claim main.rs input-size-producer` in plain text, the
worker modified `main.rs`, and it never ran the command. The validator
checks what the packet SAYS. Nothing checks what the worker DOES. A
`PreToolUse` hook on `Edit|Write` matching a path with an open co-edit
declaration could close this — the project already has
`coedit-peer-notice.sh` on that same matcher — but it is unbuilt, so treat
the barrier as advisory and `integrate.sh`'s refusal-on-shared-file as the
actual gate.

**2. The mechanism EXISTS AND FIRED. The orchestrator filtered it out.**

*This section originally said releasing had no mechanism and proposed
adding one to `integrate.sh`. That was wrong on both counts, and the
correction is the more useful finding.*

`integrate.sh` §6 is a co-edit release nudge (packet
`GH-INTEGRATE-RELEASE-NUDGE`). It names any barrier a just-integrated
worker still holds and prints the exact command. **It fired in all three
integrations** — `grep -c 'still hold a co-edit barrier'` returns 1 for
every log:

    One or more just-integrated workers still hold a co-edit barrier (practice §77):
      scripts/coedit.sh release main.rs

It was never read, because the orchestrator piped each integration through
`grep -E 'test result:|every traced target|FAILURES'` and discarded the
rest. CLAUDE.md says the opposite in as many words: *"Integrate with
`scripts/integrate.sh <name>...`, **and read what it prints**."*

And the "add release to `integrate.sh`" idea was already considered and
correctly rejected **by the script itself**, in a comment at that very
section: it never releases, because releasing *"asserts reconciliation
happened, which is a ruling, the same reason it already refuses to commit
or tick a box"*. The proposal was also incoherent as stated — the sequence
was "release after commit", and `integrate.sh` never commits.

**The generalisable rule, and it is the third instance of this shape
today:** a filter applied to a tool's output is a decision about what that
tool is allowed to tell you. The truncated-grep rule (batch 70) and §79's
"once a grep names a file, run its target" are the same lesson from
different angles. **Read the tail of `integrate.sh` in full.** It is short,
it is the one place the script tells you what you still owe, and it is
already right.

**What the barrier is actually worth.** In all three incidents the
reconciliation itself was trivial and verified: hunk ranges were compared
each time and were disjoint every time — 47 lines apart at the closest in
`config/mod.rs`, and ~3,300 apart in `main.rs` between `report_hook_with`
(~7914) and the routing sites (999–4622). Nothing was ever merged by hand
and nothing was invented. The barrier's value this session was **not**
catching a collision; it was the Stop hook refusing to let the round end
with the bookkeeping wrong.

## Batch 77 (2026-09-01, night) — five packages, seven boxes, and the day's errors turned into four gates

Packages: `pricing-recorded` (Sonnet, Amber; closed the held 1305/1306 on
four shipped-binary tests), `firewall-observability` (Sonnet, Amber),
`claude-compaction` (Sonnet, Amber; Phase −1 was **wrong** in the packet —
it cited `precompact_memory.rs` as Claude coverage and every test there is
`codex`; the worker filed `packet_errors` and settled it empirically on the
2.1.257 binary), `input-size-producer` (Sonnet, Amber; 1298/1299/1304
ticked, **1307 held on the worker's own SURVIVED mutation** — deleting
`.with_cost(cost)` from the only production writer passes 130 tests), and
`audit-batch-76-77` (recon; found the 2003 "enforced by shape" overclaim,
which the implied mutation then KILLED and the evidence was corrected).

Map **1142 → 1149** (+7 this batch, +2 more from 1305/1306 whose packages
were batch 76's). Phases 57 and 7 closed. Phase 32G 2/10 → 5/10.

**The error that mattered:** `645d6cf` — a measurements-correction commit —
carries `input-size-producer`'s entire 1005-line implementation, because
`git add -A` was typed after `integrate.sh` had applied the diff and before
the ruling. Pushed. Corrected forward in `cd62e83`'s evidence and message.
Six other orchestrator errors this batch, all the same shape (asserting
from something adjacent rather than verifying): a "gate finished" from a
wait loop matching the wrong process; a bare `blast-radius.sh` in two
packets (25+ minutes, three load reds); a duplicate wave sweep started
beside the predecessor's; a ledger entry that called a working mechanism
broken because a grep filtered its output; a Phase −1 citing tests for the
wrong harness; a permission prompt misread as a blanket grant.

**What was done about it, in the same batch — user ruling "fix it all":**

| gap | mechanism now | test |
|---|---|---|
| `git add -A` / `git commit -a` after an integration | `guard-destructive-git.sh` blocks every sweeping stage form; the message prints `git status --short` and the incident | `test_destructive_git_guard.py` (16 blocked forms, 17 allowed) |
| two gates in one tree; "is a gate running?" unanswerable | `blast-radius.sh` takes a per-tree lock, refuses a second start with exit 3, releases on exit; `--status` answers the question | `test_blast_radius_lock.py` |
| bare `blast-radius.sh` in a packet | `validate_round.py` `gate-is-targeted` refuses it; `new-packet.sh` pre-fills `--targeted` | `test_round_tools.py`, `test_new_packet.py` |
| a declared co-edit that was never claimed | `coedit-claim-guard.sh` refuses the first edit until the claim exists | `test_coedit_claim_guard.py` |
| `test_worktree_boundary.py` had no entry point — ten tests, five days green, never run by the gate | self-run block added | itself |

Not mechanised, recorded as rules: two runs before attributing a red;
read `integrate.sh`'s tail in full; quote the map line in a Phase −1, and
name the harness the cited test actually runs.

**Cost:** the fixes were the orchestrator's own context (~90k output
tokens), no worker. **Open at close:** 1307 (successor: one shipped-binary
test on the `entitlement_broker` fixture driving a real fallback; KILLED →
close), `map-index.py --check` red on HEAD (229 IDs shifted +68 from 1949 —
inherited, older than this batch's eight map commits, and a reconciliation
ruling, not a script run).

---

## Batch 78 (2026-09-01, late night) — two inherited workers unstalled, two boxes, one phase closed, and a package refused at Phase −1

**Inherited hot**, per the handoff's own instruction: two Sonnet workers
dispatched ~23:20, both blocked. Not on the work — on a **permission prompt**.
Each had stopped at *"Allow reads outside the working directories?"* before
reading its packet, because the packet lives in the main checkout and the
worker's cwd is its worktree. `6e0850e` had fixed a **different** prompt (the
project `permissions` block); this one is the user-level
`blockReadsOutsideWorkingDirectories`, which a tracked project setting cannot
answer. Both had burned ~5 minutes of wall-clock at 7% context doing nothing.
One `send-key Enter` each and both ran to completion inside twenty minutes.

**This is the failure mode the watches are blind to by construction.** A
blocked worker's pane is *not* quiet — the prompt is on screen and the token
counter is stopped, so `worker-watch.sh`'s liveness heuristics read it as
alive. The orchestrator found it only by reading both panes on inherit.
**Read the pane on inherit; a fresh worker at 7% with a clean worktree is
blocked, not thinking.**

| line | tier | result |
|---|---|---|
| 1239 | Sonnet low, Green | closed — **Phase 32B CLOSED 14/14** |
| 1307 | Sonnet medium, Amber | closed — the held box, closed by its own named successor |

Both tests-only, file-disjoint, **one** `integrate.sh` call, targeted gate
green (entitlement_broker 40, session_router 19, rustdoc clean), committed by
pathspec, pushed.

**1307 is the hold rule paying off a second time.** Yesterday it was held on a
SURVIVED mutation with its successor named exactly: *one shipped-binary test on
the `entitlement_broker` fixture.* That is what landed, and it took the priced
path rather than the free-model escape the packet permitted — so the limit the
hold anticipated does not apply. The mutation that caused the hold is KILLED.
**Naming the successor inside the hold is what made this cheap.**

**1239 is the case for verifying the tick rather than the report.** All five
artifacts were present and the report was accurate. But its third assertion,
`unread >= empty`, is satisfied by **equality** — both price to exactly `0.0` —
so it proves nothing, behind a failure message that reads as though it would
catch the very case. The separation lives in a different mechanism (the
line-1587 affinity facet), and the orchestrator **re-ran that mutation on the
merged tree** rather than accepting the worker's KILLED, precisely because it
was the half the new test does not watch. KILLED. The ledger entry is longer
than the tick and says which assertion is load-bearing and which is decorative.

**Two stale ledger claims corrected, both found while establishing Phase −1**
— and both were blockers that would have stopped a future package:

1. `phase-32b.md`'s 1239 paragraph said *"nothing asks a routing question of
   capacity"*. True when written; false since 1598 closed.
2. `phase-33c.md`'s *"a cadence signal deliberately not collected"* cited
   `retry_after: None` at `gateway/session.rs:564`. Line 1319 wired it; the
   signal now travels `stated_retry_after` → `RateLimited{retry_after}` →
   `ResourceHealth::fail` → `FreePool::is_available`.

**A ledger blocker is a claim with a date on it, not a fact.** Both were
written by careful orchestrators and both rotted within days of the line that
falsified them. Check the blocker before believing the phase is blocked.

### The refusal that is the batch's real output: 1263

`cluster-b.py` flagged `recent_credential_spend` (`routing/evidence.rs:1837`)
— real, tested, **zero production callers**. The register's row for 1263 said
it was blocked on *"Phase 32G, which is 10 open / 0 closed"*, and 32G is now
6/10 with `cost_micro_usd` written and reading back. Every visible sign said
*wire it up, close the box.*

It is a trap, and Phase −1 caught it. `main.rs:8613` states in production that
`gateway::ingress` **relays a body it is designed never to parse**, so the
token columns have never been written on the relay path. The only writer is
memory extraction, which produces no row at all under the default
configuration. Wired today, the spend reader would sum memory-extraction calls
and call the result the user's spend. **Refused before dispatch**; the register
row now carries the live blocker instead of the stale one.

Cost of the refusal: orchestrator reading. Cost had it been dispatched: a
worker package plus the un-tick an audit would eventually have produced.
**This is what the gate is for, and it is the third time a package has died at
Phase −1 rather than after.**

**Dispatched forward:** `paced-retry` (1368, Amber, Sonnet high) on the
now-proven cadence chain — the gap is that `FreePool::is_available` has no
caller anywhere in `src/gateway/`, so the accept loop forwards the next
request to a credential its own previous exchange put in cooldown. Policy
decided by the orchestrator in the packet, not left to the worker: refuse
locally with the stated wait, never hold the accept loop.
`audit-batch-78` (read-only) runs beside it against this batch's own ticks.

**Open at close:** the wave's trailing full sweep is deliberately **not** run
yet — one integration is not a wave, and two workers are compiling. It is owed
once `paced-retry` integrates.

---

## Batch 79 (2026-09-02, small hours) — six boxes, two phases closed, one package refused, and five stale ledger blockers

Continuation of batch 78's session. **Map 1149 -> 1154** across both batches;
this batch closed **1152, 1368, 1330** and landed the fix-forward for its own
regression.

| line | tier | result |
|---|---|---|
| 1152 | Sonnet medium, Green | closed — **Phase 29 CLOSED** |
| 1368 | Sonnet high, Amber | closed — Phase 33C 12/15; **the only production behaviour change of the night** |
| 1330 | Sonnet high, Amber | closed — Phase 33A 11/15 |
| — | Sonnet high, Amber | `paced-fixtures`, the fix-forward for 1368's own regression |
| — | Sonnet high, read-only | two audits; all seven ticks examined were confirmed |

### The three findings worth carrying forward

**1. A restraint line is mutation-proven by violating the restraint.** 1152 had
code, a passing shipped-binary test, and reachability since 2026-08-31. It was
open on a *missing mutation*, because the prior worker reasoned that adding a
checkpoint write "would be inventing a feature rather than removing one." That
rule does not fit a line whose content is *"X must not do Y"* — there, the
defect **is** the addition, and requiring deletion-only mutations would make
every restraint line in this map permanently unclosable. **A rule about
mutation vocabulary was quietly deciding a capability question.** The violation
compiled and was killed by the assertion that names the restraint in its own
message.

**2. A packet may fix a policy; it may not fix one that contradicts a shipped
test the packet never looked at.** 1368's packet said the policy was decided
and not the worker's to revisit: guard on `FreePool::is_available`. That is not
implementable. `ResourceHealth` folds a provider-**declared** wait together
with a cooldown Glasshouse **invents** after ordinary failures, and Phase 9I
line 534 deliberately keeps the invented kind probeable by real work. The
literal guard broke a conformance test whose third ordinary `503` must still
reach the provider. The worker's blast radius caught it, it narrowed via
`quota_headers()` without touching the forbidden file, and — the part that
matters — **it verified rather than asserted**: passes at HEAD, fails with the
literal design, passes with the narrowed one. Its narrowing is a *more faithful*
reading of the line than the packet was.

**3. A Phase -1 consumer search must cover match arms.** 1330's packet asserted
*"no consumer treats `purpose IS NULL` as meaningful — verified."* False.
`RoutingOverhead::from_consumption` had `None if group.harness_recorded =>`
feeding `coding_agent_requests`. The orchestrator grepped
`purpose.is_none()`, `purpose == None` and `purpose: None`; **a match arm is
none of those shapes.** Wiring the stamp alone would have silently zeroed
interactive coding cost (lines 1464/1832/1833) from that build forward. The
worker caught it, fixed it, and asked for the diff to be reviewed. `grep
'purpose'` in the consuming module would have found it; three narrower greps
did not. The arm shipped with **no direct test**, and the follow-up package
(`consumption-arm`) was dispatched rather than recorded and forgotten.

### The trailing sweep earned its keep, for the second time

`blast-radius.sh --since 6fd1888` ran ~35 minutes against the wave and found
**exactly one** failing target: three reds in `gateway_failure_taxonomy`, from
1368 shipping on a green *targeted* gate that skipped 13 full-trace targets.
Everything else green — bin, rustdoc, all other targets.

Attributed **mechanically rather than by a second run**: all three fixtures
drive one gateway with one credential through a sequence containing a `429`
with `Retry-After`, so cases after it now meet the new guard. The fix-forward
worker repaired them **non-uniformly** and was right to: a sibling credential in
the first test forces a real failover and would have weakened its load-bearing
`failovers == Some(0)` assertion (it tried it and observed `Some(1)`). Zero
assertions removed — verified by `git diff` at integration, not taken from the
report — and 1368's own mutation still KILLED afterward.

### Five stale ledger blockers in one session, and two of them were traps

| line | stale claim | outcome |
|---|---|---|
| 1239 | *"nothing asks a routing question of capacity"* | CLOSED |
| 1368 | *"a cadence signal deliberately not collected"* | CLOSED |
| 1546 | *"cadence … not yet read by anything"* | packaged |
| 1263 | *"blocked on Phase 32G, 10 open / 0 closed"* | **still refused** — relay-path `ingress` reader |
| 1419 | *"no candidate set"*, *"no per-model price"* | **still refused** — `Cost` is a `Free`/`Metered` enum, not a price |

**The last two are the dangerous shape:** the stated reason rots, the line stays
shut, and an orchestrator who re-checks only the stale half packages it. 1263
was caught at Phase -1 *before dispatch* — `recent_credential_spend` has zero
production callers and looks exactly like a Cluster B wiring job; wired today it
would sum memory-extraction calls and call them the user's spend. **Correct the
row AND state the live blocker**, which is what `c885038` and `f894f7a` do.

**A ledger blocker is a claim with a date on it, not a fact.** All five were
written by careful orchestrators and all rotted within days of the line that
falsified them.

### Two operational traps, both invisible to the mechanisms that exist

1. **A blocked worker looks alive to its watch.** Two inherited workers had done
   nothing — both sat on *"Allow reads outside the working directories?"*. The
   prompt is on screen and the token counter is stopped, which
   `worker-watch.sh` reads as thinking; it fired only its benign "quiet but
   still moving" note. **A fresh worker at ~7% context with a clean worktree is
   blocked, not thinking.** The tracked `permissions` block from `6e0850e`
   cannot answer this one — it is the user-level
   `blockReadsOutsideWorkingDirectories`.
2. **`new-worker.sh` reported "DID NOT ACCEPT the prompt" three times and the
   prompt had already landed** — the submit fired while the pane still showed
   `/rc connecting…`. `send-key Enter` alone fixes it; re-sending the prompt
   appends a second copy and `ctrl+u` does not reliably clear it.

Neither is mechanised yet. Both are one-line checks at inherit and at dispatch.
