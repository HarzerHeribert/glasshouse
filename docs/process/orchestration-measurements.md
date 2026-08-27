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
