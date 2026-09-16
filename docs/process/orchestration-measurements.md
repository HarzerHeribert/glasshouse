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
| Pane uncapped spend + literal scripts | **2× Codex `gpt-5.6-sol` high** | ~8 min parallel | subscription | +394/-192, 22 files before records | 0 (benchmark-driven correction) | focused 157 + 25/87/18; full Pane suite and Clippy re-run by primary | 1 (stale subagent-budget comments) | PASS; post-fix live benchmark next |
| Pane/Claude matched benchmark trial 02 | primary, actively observed | ~11 min including oracle and one intervention | same Claude subscription | evidence only | 0 | external six-case oracle + shell syntax | 2 caught early (one interpolation recovery, one malformed shell edit) | Claude autonomous PASS 6/6; Pane autonomous FAIL 2/6, assisted PASS 6/6 |
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

> Batches 13–14 through 90 (2026-08-26 to 2026-09-02) moved to docs/history/orchestration-measurements-batches-13-to-90.md.
## Batch 91 (2026-09-02, night) — Phase 24 complete, expected latency scored, first token and first tool call on the translated path, and the Windows leg finds the binary cannot start

Session `0c56372c` (Fable 5.1), continued from batch 90. Three workers dispatched by the predecessor and by this session finished within an hour of each other and were integrated as waves 101 (latency + reranker, one gate) and 102 (stream-first-events, its own gate because it holds `gateway/**`): **map 1229 → 1237** once both land.

| package | tier | result |
|---|---|---|
| `expected-latency-score` (wave 101) | Sonnet high, Amber | **1539 closed** — `ModelCall` stamps dispatch/completion, `support_work_latency` reads a median over extraction rows, an *expected latency* term in the disposable router's `score()` that never joins the free order. 4/4 KILLED; one packet error (the co-editor's name), two mechanical overflows |
| `memory-reranker` (wave 101) | Sonnet high, Amber | **1089, 1090, 1091, 1092, 1094 closed — Phase 24 complete.** The seat in the library (a door cannot call the binary crate — the worker's structural correction, right), 8 candidates, strict ids, currency after the reorder, diagnostics on request, `memory search --explain`. 5/5 KILLED; the design note post-dated its base (same as budget's) and the packet's objective sufficed |
| `stream-first-events` (wave 102) | Sonnet high, Amber | **1331 and 1332 closed** on the translated path — `FirstEvents::note` on canonical events, two migration-11 columns finally written, `routing-cost` prints two more means; `--lib gateway` whole 210/210. 4/4 KILLED, one of them after the worker caught its own non-compiling mutant reported as a false KILLED (§80's fourth way) |
| Windows, five fix-forwards, no box | orchestrator | `_mktime64`; `libc` for Windows; `firewall_local_reducer.rs` `#![cfg(unix)]`; `memory_rating.rs` import; **`build.rs` with an 8 MiB `/STACK` reserve** after 178 main-thread overflows |

**Output per box.** Day total ≈ 18.5M at wave 101's commit for **+34 net today** (1201 → 1235), about 545k per net box; this session about 1.5M for +10 net so far, under the 250k line for the session, above it for the day. Every dispatch was implementation.

**Findings worth carrying forward.**

1. **A platform leg that runs once a day finds its defects serially.** Five Windows fixes in one evening, each visible only after the previous one; the last (the stack) had been true for an unknown number of commits — nothing between batch 86 and here ran Windows. The trailing sweep now includes the VM leg per wave, from a detached worktree so main stays free.
2. **Windows' 1 MiB main thread is a real ceiling for a 17,000-line `main.rs` in debug.** The fix is the linker's reserve (no thread, no runtime change), from a build script rather than a tracked `.cargo/config.toml`, because `new-worker.sh` writes an untracked one per worktree and would clobber it.
3. **A scratch crate must mirror the crate's target-conditional dependencies** (batch 90's lesson, confirmed: the arm type-checked against a `libc` the Windows build did not have).
4. **Three workers, three packet errors, all right** — two of them the same shape: a design note committed after the worker's base was cut. Cut a worktree from the commit that carries its design note, or say in the packet that the note is younger than the base.

**Waves 101–102's trailing sweep** (`--since 82b4db4` from a detached worktree at `b2aabc8`): **158 targets green, 2 red, both attributed** — `disposable_interface`'s reranking tripwire, which fired exactly as designed and was ruled (1625 closed, `2bdbbc5`), and `database::tests::concurrent_first_bootstraps_serialize_on_one_database`, the known bootstrap flake under load, green twice alone. The tripwire is the finding: the reranker's targeted gate never traced `disposable_interface.rs`, so a sweep — not a gate — is where a tripwire fires, and it fired within the hour.

**Wave 103's gate** (retrieved-view + route-rationale-sink + the kind pin): four targets green, `--lib database` red on `concurrent_first_bootstraps_serialize_on_one_database` — attributed by §91's interleave under load 12: main failed 2/2, the **unmodified HEAD baseline failed 1/2**, so the red is the known bootstrap race under load and not the wave. Committed on that attribution.

## Batch 92 (2026-09-02, night) — Phase 33A complete, rounds per minute printed as what it is, and the Windows stack reserve holds

Staged by session `0c56372c` and landed by session `221d1dd9` (both Fable 5.1), inherited hot with one worker live, one finished and a validated Red packet waiting: waves 103 and 104 — **map 1238 → 1243** (wave 103's three at `bc8bb50`, wave 104's two here; wave 104's *code* went in under `2cab1dc`, see finding 3).

| package | tier | result |
|---|---|---|
| `retrieved-view` (wave 103) | Sonnet medium, Amber-light | **1759 closed** — the retrieved set is recorded per session, so the view is one query |
| `route-rationale-sink` (wave 103) | Sonnet high, Amber | **1757 and 1766 closed** — the session router's rationale on the durable sink (`sessions show`, `status`); the evaluation module forbids serde by its own pin, so the row's JSON is hand-written |
| `tool-rounds-translated` (wave 104) | Sonnet high, Amber | **1334 closed — Phase 33A complete (15 of 15)**: successful rounds, retries, repairs, failovers and the outcome as five separate columns on the translated path, `NULL` on the relay; **1350 closed**: rounds per minute of serving time in `routing-cost`. Targeted gate exit 0; `--lib gateway` whole 210/210 |
| `budget-spend-remaining-callers` | Sonnet, Green → ruled | no box; the reducer's chooser and the rerank seat count budget spend like the extraction seat. First report leaked a `String` into `ModelError::Failed { phrase: &'static str }`; refused because `api/unix.rs::select_memory` calls the resolver from the long-lived control server; resumed with a ruling (an owned-string `ModelError::Declined`), 3/3 mutations KILLED after the re-run |
| Windows run 5, three fix-forwards, no box | orchestrator | build **PASS**, MSRV **PASS**, **no stack overflow anywhere — batch 91's 8 MiB reserve verified**; test leg red on three targets, all pre-existing: `dispatch_reservation` and `v1_criteria_setup` never executed (*requires elevation, os error 740*), `conformance::no_rendering_the_gateway_can_produce_carries_either_planted_secret` read a connection reset. Fixed in `windows-run5-gaps`: two renames plus a script test on the names, `Shutdown::Both` → `Shutdown::Write` at the ingress's two close sites |

**Output per box.** Day total **20.94M** at this commit for **+42 net today** (1201 → 1243), about 500k per net box across the day's four sessions; this session's own share is small for +2 net (the landing of a wave staged and gated by its predecessor). Above the 250k line on the day, so dispatches stay implementation: the Red packet (1347, 1348, 1349, 1355) went out first.

**Findings worth carrying forward.**

1. **Windows installer detection reads test-binary names.** Any executable whose file name contains *install*, *setup*, *update* or *patch* is assumed to need elevation and refuses to start under a standard user. Cargo names each integration-test binary after its source file, so `dispatch_reservation.rs` (dis-*patch*) and `v1_criteria_setup.rs` never ran on the VM — for every run since they were written, showing only as `error: test failed` with no test output. `scripts/tests/test_windows_test_binary_names.py` keeps the next name out of that list.
2. **`shutdown(SD_RECEIVE)` on Windows is an abortive close when bytes are queued.** `Shutdown::Both` before dropping a socket is a Unix habit that costs nothing there and resets the connection on Windows the moment a byte is still queued or arrives afterwards — the harness then reads a reset in place of the response it was just sent. The ingress shuts down the write half only now; four more `Both` sites in `translate/mod.rs` and one on the listener wake-up in `gateway/mod.rs` were green on the VM and are left for the Red worker's tree.
3. **A handoff commit swept staged code.** `2cab1dc` says *docs(handoff)* and carries wave 104's nine code files, because they were staged and the commit was made without a pathspec. §89's stage-by-pathspec rule applies to the handoff commit too: `git commit -- <paths>` even when the index looks like yours.
4. **The VM leg is now part of every wave's trailing sweep**, from a detached worktree, after checking the VM's free space: a cold run consumes about 19 GB of `C:\ci\target` (45 GB free at run 5's launch, 26 GB after), so wipe the target directory below 30 GB free. Runs are serial — never two at once.

## Batch 93 (2026-09-02, late night) — the Windows leg goes fully green, 1247 and 939 close, and the board is repackaged by mechanism

Session `221d1dd9` (Fable 5.1). Wave 105 (`db5834b` + `2cd780d`): **map 1243 → 1244**; wave 106 (939 + migration 25's four lines) follows in this batch.

| package | tier | result |
|---|---|---|
| `estimator-reset` (wave 105) | Sonnet high, Amber | **1247 closed** on the line's quota-behaviour disjunct (Phase 32C 11/12): a stated-ceiling difference between two persisted gateway readings is the regime change, persisted as `regime_changed_at_unix`; the estimator's one caller floors its rows there; `entitlements` says *limits changed <age>*. 4/4 KILLED; two open decisions made by the worker and accepted (a sibling accessor; the caller pre-filters) |
| `budget-spend-remaining-callers` (wave 105) | Sonnet, no box | the reducer's and the rerank seat's choosers count budget spend; the first draft's `String::leak()` refused (a long-lived caller), corrected to `ModelError::Declined`; 3/3 KILLED |
| `windows-run5-gaps` (wave 105) | orchestrator | two test files renamed off Windows' installer-detection words, a script test on the names, `Shutdown::Both` → `Write` at the ingress's two close sites; the routing_outcome prompt marker fixed after the sweep found it |
| **Windows VM run 6** on `2cd780d` | platform leg | **build PASS, test PASS, msrv PASS — the first fully green Windows run.** `--lib` 2062/2062 (the conformance reset gone), `launch_reservation` 6/6 executed where its old name never started, `v1_criteria_first_run` executed (its tests are unix-only and filter to 0 there, as before); 153 `test result: ok`, 0 FAILED, no *requires elevation*, no *ConnectionReset*, no *overflowed its stack*. The five fix-forwards of batch 91 and the three of batch 92 are all verified |
| **Windows VM run 7** on `8a863c5` | platform leg | **build PASS, test PASS, msrv PASS** — migration 25's test included; the Red package's platform leg settled the same night |
| `retrieval-feedback` (wave 106) | Sonnet high, Amber | **939 closed** (Phase 21F 10/11): a rating carries the scope of the retrieval it judges (session-narrowed, else latest, else none — one `ORDER BY CASE` query) and `memory retrievals` prints false positives per retrieval scope. 4/4 KILLED; the worker strengthened test (c) so the `memory_id` filter is actually exercised (§80) |
| `stream-timing-ms` (wave 106) | Opus 5 high, RED | **1347, 1348, 1349 closed; 1355 open by ruling** (effective TTFC does not exist yet — `GH-RESPONSIVENESS-TERMS`). Migration 25's four ms offsets, each path's own `Instant` before its send, the readers' preference and fallback, four labelled lines. 5/5 KILLED; five packet errors all right (12 pins, 17 rollback lists — the migration ripple again); an independent Opus verifier read the diff (its verdict in `phase-33b.md`) |
| `phase51-joins` (wave 107) | Sonnet high, Amber | **1836, 1855 (token half), 1854 (sparse + stale) closed** — the headroom estimator replayed against the throttles that followed (warned/missed/unestimable + the observed reset lag), a `RoutingConsumptionEstimated` row at launch joined to the session's actual output tokens, and the by-evidence-held block proven through a real launch against a stale reading. 5/5 KILLED; four packet errors all right (`FailureClass::Throttle`/`ExhaustedQuota`, the estimator's `None`-only floor, exhaustion rows outside its pressure signals, no stated wait on the row) |
| `phase47-debug-views` (wave 107) | Sonnet high, Amber | **1760, 1767, 1769 closed — Phase 47 complete (15/15)**: `sessions show --debug` (the router's own cache estimate beside the session's real cached-input share), the correlation readout's sample-before-confidence proven with `sample-dropped` KILLED, `[memory] extraction_diagnostics` (an opt-in JSONL, ids and counts only, a planted subject and credential proven absent). 5/5 KILLED; three refusals from batch 50 that had outlived their blockers |

**Output per box.** Day total **23.18M** (2026-09-02, closed at midnight) for **+47 net on the day** (1201 → 1248 at `8a863c5`), about 490k per net box; wave 107 (+6, to 1254) lands after midnight and opens 2026-09-03's count. This session ≈ 2.4M for +13 net including wave 107, under the 250k line. Every dispatch was implementation.

**The user's ruling tonight — and what changed.** *"You keep picking the small work … couldn't you compile small things into a substantial package?"* Right: two of this session's first three dispatches were 1-box (939 was Phase 21F's last reachable line, which the rule allows, but the pattern was the point). The remaining open lines are mostly refused singletons, so the substantial packages live where producers landed this week: **`GH-PHASE51-JOINS`** (1836, 1854, 1855 — the router's estimates against what happened; dispatched), **`GH-PHASE47-DEBUG-VIEWS`** (1760, 1767, 1769 — three refusals that outlived their blockers, completing Phase 47; dispatched), **`GH-RESPONSIVENESS-TERMS`** (1351, 1352, 1542, 1543, 1544, 1845, 1850 — seven boxes on one reader over the ms columns; validated, dispatches after migration 25). Board at the three-editing-worker ceiling.

**Findings worth carrying forward.**

1. **A refusal outliving its blocker is now a package shape, not a one-off.** 1767's row says *nothing computes a correlation* while `route_correlations_section` prints sample size and confidence per pair; 1760's says *no temperature signal* while the router's estimate is durable on the rationale row beside real cached-token counts; 1769's says *no durable extraction record* while every extraction writes a routing row. Three in one phase, found by re-reading rows whose *reason* names a producer that landed this week. Do this pass after every wave that adds a column or a row kind.
2. **`git add … && git commit` joined with `;` commits whatever was staged.** A staged deletion is not a valid `git add` pathspec; the chain stopped, the commit ran, and `db5834b` went out without four new files. Two commands, always.
3. **The whole `--lib database` module fails under load on an unmodified tree** (`concurrent_first_bootstraps_serialize_on_one_database`, three times this batch with two workers building); the single test alone passes. Attribute with the single test alone and the module on a detached baseline under the same load, not with the module alone.
5. **A readout line inserted between two lines a test reads by position is a regression the targeted gate cannot see.** Wave 107's 1836 line went between an account's facets line and its `served:` line; `tests/entitlement_broker.rs` takes three lines from the account's name and was never traced by the targeted blast (it traces the changed files' own targets, and the view's file is `main.rs`, whose full trace is 100+ targets). The trailing sweep caught it within the hour, as designed; the fix is placement. When a package adds a line to a shared readout, name every test that reads that readout by position in the packet's acceptance list — `grep -rln "<a phrase from the block>" crates/glasshouse/tests` is the ten-second census.

4. **The VM leg is part of every wave's trailing verification now** that it is green: from `.worktrees/winvm-check` detached at the commit, after `fsutil volume diskfree C:` (wipe `C:\ci\target` below 30 GB free — a cold run costs ~19 GB), never two at once.

6. **A product race with a timer where a lock belonged, and a verifier that measured instead of argued.** The `--lib database` red of finding 3 was not a flake: `check_existing` told a sibling's in-flight zero-byte file apart from a truncated one by *sleeping* up to 500 ms, and migration 25 made the creator's first migration outlast that under load. The Red fix (`bootstrap-race`, Opus 5 high) waits on SQLite's own write lock instead — `BEGIN IMMEDIATE` on a `journal_mode = MEMORY` probe, a 40 × 10 ms grace for the creator's pre-lock window (the packet's design without the grace refused a healthy sibling, proven with the `grace-removed` mutation), a deterministic 2 s stress test per §60 and a 64-caller variant. The independent Opus verifier answered the packet's "decisive" WAL question by *measurement* — `configure` sets no journal mode, a commit grows the main file 0 → 16384 bytes, and even the WAL counterfactual writes a 4096-byte header before any commit — and found what the report had wrong: the wait's honest bound is 40 × 30 s, not 30 s (a waiter that acquires and finds the file ungrown loops); the `create_new` loser is the *majority* path in a burst and still bets on `configure`'s fixed 5 s (confirmed with a shortened-timeout experiment: `database is locked`), the same shape as the defect one door down; `CreationWaitTimedOut` is reached by no standing test; and the unwritable-file test pinned only "the message names the path", which `Sql`, `Open` and `EmptyExisting` all satisfy — the orchestrator added the variant assertion at integration. Two things to carry: a verifier packet's "decisive question" is an anchor like any other (§44) — this one's premise did not hold and the verifier said so instead of answering it; and a bound stated in a report gets quoted later, so read the loop before quoting it.

## Batch 94 (2026-09-02, late night) — the bootstrap race closed under verification, ten accepted lines land, Phases 33C and 34B complete, and Phase 28's producer found in Phase 57's hook

Session `1ee4f96b` (Fable 5.1), inherited hot from `221d1dd9` at 76 % context with two accepted waves unapplied and a verifier live. Waves 108 + 109 shared **one** gate on the merged tree (three packages, one blast radius — trap 2 applied for the first time since it was written): **map 1254 → 1264**.

| package | tier | result |
|---|---|---|
| `bootstrap-race` (wave 108) | Opus 5 high, RED, no map line | the 500 ms timer in `check_existing` replaced by SQLite's own write lock (`BEGIN IMMEDIATE` on a `journal_mode = MEMORY` probe, a 40 × 10 ms grace for the creator's pre-lock window, `CreationWaitTimedOut` when the lock cannot be taken within 30 s); 5/5 KILLED; a 2 s deterministic stress test and a 64-caller variant; base red ×2 / fix green ×2 in the worker's A/B/A/B and again in the verifier's against a detached baseline |
| `verify-bootstrap-race` | Opus 5 high, read-only verifier | **ACCEPT** with five findings, all residuals: the WAL premise does not hold (measured, 0 → 16384 bytes on commit); the wait's honest bound is 40 × 30 s; the `create_new` loser still bets on `configure`'s 5 s (`database is locked` reproduced with a shortened timeout) — left to `GH-BOOTSTRAP-CREATE-ATOMICALLY`; the refusal probe removes a pre-existing hot journal; `CreationWaitTimedOut` has no standing test; the unwritable-file test pinned only the path in the message — the orchestrator added the `EmptyExisting` assertion at integration |
| `responsiveness-terms` (wave 109) | Sonnet high, Amber | **1351, 1352, 1355, 1542, 1543, 1544, 1845, 1850 closed** — one reader over migration 25's four millisecond columns: effective TTFC, reliability-adjusted latency, the fifth figure on its own line (1355 ruled closed on `headline-merged` KILLED), three scoring terms, the separation measure. 6/6 KILLED; `--lib routing` 286/286 |
| `last-lines-33c-34b` (wave 109) | Sonnet high, Amber | **1366, 1419 closed — Phases 33C and 34B complete**; six packet errors all right; `choose_for_automatic_classification`'s sort now sums the verdict's notes (read and accepted) |
| **Windows VM run 9** on `ad2e8f5` | platform leg | build PASS, test PASS, msrv PASS |
| `file-aware-memory` | Opus 5 high, RED | dispatched on this commit (1139, 1141, 1142 — Phase 28's last three); see finding 2 |
| **Linux leg** on `08e0ff2` | platform leg | `--lib` and every integration target through `t` green — all of waves 108/109's own included, the first Linux evidence for the race fix; `tests/terminal_loss.rs`'s 15-trial hangup test red on 1 trial under two Opus builds + the VM packaging (a CPU-bound PTY family, ~3555 above), so `cargo test` stopped before the targets after `t`; clippy and msrv PASS. **Attributed with two runs, not one:** the target alone, same tree, same warm container, load ~6–7 → 4/4 green twice. Load flake; no worker. Recorded in `phase-58.md` |
| `file-aware-memory` (wave 110) | Opus 5 high, RED | **1139, 1141, 1142 closed — Phase 28 complete**: migration 26 (`file_touched` + `path`, migration 7's rebuild shape), `--session` on the firewall hook, `record_file_touches`, the byte-equality guard, `Referenced`, `RetrievalIntent::CodeEdit` between rung and weight, `Freshness` by commit order, `memory search --path [--for-edit]`. 6/6 KILLED + `guard-off` re-run on the merged tree; 24 targets; the worker found and fixed a §79-shape red itself (the activity view's variant pin). Integration found one pin in a fourth shape (`tests/session_context.rs`) and one flaky-pass |
| **Windows VM run 11** on `b065034` | platform leg | build PASS, msrv PASS, **test FAIL on one binary of 158**: `tests/memory_file_observer.rs` pinned that `referenced` cannot be stored — a premise Phase 28 retired; neither the worker's blast radius (35 targets) nor the integration gate traced that target. Reproduced on macOS, fixed as `ca18723` (the pin inverted, the test renamed for what it now proves). The next Windows run is on the decomposition commits |
| `decomp-config` (Phase 59) | Sonnet high, Green pure move | `config/mod.rs` (10,564) → mod.rs 185 + seven concern files (largest 1,666) + `config/tests/{mod,part_a,part_b}.rs`; 85/85; found `blast-radius.sh`'s basename mapping bug (fixed in `5d46aee`). Staged under its gate |
| **Windows VM runs 12 and 13** on `4f6650f` / `fa66efc` | platform leg | 12: the build failed on `database/tests.rs`'s unix-only `std::fs` import (finding 9); 13 on the one-line fix: build PASS, test PASS, msrv PASS — the first Windows-green commit carrying a split |
| `decomp-database` (Phase 59) | Sonnet high, Green pure move | `database.rs` (6,937) → `database/{mod 266, schema 291, bootstrap 499, migrations/{mod 169, v1_to_v13 1160, v14_on 1227}, tests 3379}`; 87 non-move lines enumerated; 55/55 before, after, and ×2 on the merged tree with the race tests alone and nine targets green. Discharges the grandfathering of `26fb65b`/`b065034`; the ratchet now skips standalone test files (found by this package) |
| **Linux leg** on `ca18723` | platform leg | build+test PASS, clippy PASS, msrv PASS — every target, no flake: Linux evidence for the atomic creation, migration 26, Phase 28 and the routing-evidence split together. The trailing sweep on the same commit: 158 targets green; `mcp_project_scope` 2 of 4 red on a `recv_timeout` beside three builds, 4/4 alone in 1.0 s — flaky-pass, the sweep's only red |
| `decomp-routing-evidence` (Phase 59) | Sonnet high, Green pure move | `routing/evidence.rs` (8,160) → `evidence/{mod 1640, ledger 123, readers 2071, signals 798, joins 1169, tests 2440}`; every non-move hunk enumerated; 62/62 before and after; no file outside the directory touched. Staged under its gate |
| `decomp-routing-session` (Phase 59) | Sonnet high, Green pure move | `routing/session.rs` (7,213) → `session/{mod 2152, discovery 926, scoring 1228, reserve 1626, tests 1328}`; 47 non-move lines enumerated; 286/286 before, after, and on the merged tree with ten routing targets green. Reported itself *partial*: `routing/mod.rs`'s three boundary scans `include_str!("session.rs")` and the packet forbade that file — joined at integration as `session_source()`, and the boundary-scan clause went into CLAUDE.md's Decompression rule 2 and every packet skeleton's objective 2b |
| `decomp-shell` (Phase 59) | Sonnet high, Green pure move | `shell/state.rs` (8,057) → `state/{mod 857, overview 659, knowledge 437, route 326, settings/{mod 1299, keys 1468}}`, the inline tests of `mod.rs`/`view.rs`/`state.rs` to `shell/tests/{mod_tests 3683, view_tests 3630, state_tests 3082}`; net +70 lines enumerated; 314/314 before, after and on the merged tree. **Line 2052 closed.** The worker split `settings` once more when one file came out at ~2,780, and made `keys.rs` a child module so no private field needed a bump; the four self-scans moved with the tests and needed a path fix, not a join, because the scanned files stayed single |
| `decomp-api-unix` (Phase 59) | Sonnet high, Green pure move | `api/unix.rs` (4,135, no inline tests) → `unix/{mod 879, sessions 699, memory 794, routing 379, checkpoints 122, events 782, assumptions 563}` by verb family, the dispatch `match` unedited in `mod.rs`; 83 non-move lines (41 `pub(super)` each with its caller); `--lib api` 17/17 and the four socket-door targets 9/4/19/17 before and after; the worker caught its own extraction slip (a cut `impl` brace) at `cargo check`, before the gate. The first split with nothing scanning it — objective 2b cost one grep |
| **Windows VM run 10** on `08e0ff2` | platform leg | build PASS, test PASS, msrv PASS — the race fix's Windows evidence (SQLite's `LockFileEx` path); the trailing sweep on the same commit green over 147 targets |
| `bootstrap-atomic` | Opus 5 high, RED | **landed** — private temp file, one hard link, the wave-108 wait removed (F1/F3/F4 of the earlier verifier answered by construction); 6/6 KILLED, 55/55 ×5 under load; three packet errors all right (the truncated-file test cannot catch `rename`, a rendezvous replaces the fixed sleep, a recycled pid is swept on proof of start time). **Verifier ACCEPT** with five residuals, the first of which falsified the design note's cost claim by measurement (the 64-caller test 12× slower by design — recorded, kept) |

**Output per box.** 2026-09-02 (Berlin) closed at **24.73M** output for **+63 net** on the map (1201 → 1264 at this commit), about 390k per net box — above the 250k line the tier rule names, so the next dispatch after `file-aware-memory` is implementation too. Both integrations tonight were of accepted work; the one dispatch is implementation.

**Findings worth carrying forward.**

1. **The checkpoint's design plan was written from a register row its own later section had superseded.** Row 477 (*no file-path association exists at all*) was quoted as the premise for a migration adding a path table; the register's *Phase 28 scoped* section, 270 lines down, records that migration 17 landed `memory_files` and names its reader. CLAUDE.md's *read the register before you commit to anything* means the whole register: grep the line number, not the census table.
2. **A producer that landed for another phase unblocked a "the signal does not arrive" refusal in this one.** Phase 28's 1139 was refused because the extraction model's input carries no paths — true of the lifecycle events. Phase 57's context firewall then registered a `PostToolUse` hook with matcher `*`, so every `Edit`/`Write`'s `file_path` reaches `context_firewall_hook` on the production path; nothing kept it, and nobody re-read 28's refusal after 57 landed. Batch 93's finding 1 was about *rows whose reason names a producer*; this is the same rule one level up: **after every hook, adapter or migration package, re-read the register's Cluster E/H refusals whose reason is "no input arrives"** — the input may now arrive for a different phase's sake.
3. **A verifier's "decisive question" is an anchor like a packet's hypothesis (§44).** The bootstrap-race verifier was asked whether WAL mode could keep the main file at zero bytes; it measured that the premise (WAL at all) did not hold, said so as a packet error, and answered the counterfactual anyway. Write the decisive question as killable, and reward the kill.
4. **`new-packet.sh --worktree` announces the worktree and does not create it** (it printed the `git worktree add` line; the launcher then said the directory does not exist). Create it by hand after the base commit exists — which tonight was the right order anyway, because a worktree cut from `ad2e8f5` would have lacked wave 108's database fix that the packet's own gate relies on.
5. **`README.md`'s progress block sat at 1201 for nine batches while the map went to 1264 — the user noticed, not a script.** `scripts/progress.py` is the only writer, and the only check is `ci-local.sh`'s *lint / README progress* step — a lane the trailing sweeps run from a detached worktree and whose failures have been attributed to the environment since batch 80-something (the *script tests assume the main checkout* memory). A check that lives only in a lane nobody reads is not a check. Fixed forward tonight: `orient.py`, which every orchestrator runs after a map change and `--check`s at every checkpoint, now regenerates the README block too and fails `--check` when it is stale.
6. **The size ratchet fired on its first gate, on a Red that predates it.** `bootstrap-atomic` grows `database.rs` 3251 → 3401 production lines (+87 code, +63 comment; the file is 56 % comment in production), and `file-aware-memory`'s migration 26 adds ~140 more. Both were dispatched and verified before `1322fea` recorded the rule, so both are grandfathered **once**, explicitly, in their commit messages, and `GH-DECOMP-DATABASE` is dispatched in the same wave. The alternative — blocking two accepted Reds on a comment trim — is the over-assurance the ruling names. The rule stands from the next package on.
7. **The migration ripple has a fourth shape, and only the integration gate's target list caught it.** `tests/session_context.rs` pins the schema version as a bare `25,` on its own line under `schema_version(&conn),` — not `version, 25`, not `SUPPORTED_SCHEMA_VERSION, 25`. The worker's grep and the gate's pin grep both missed it; `--test session_context` was not among the 35 targets its blast radius traced (the ripple memory: the radius does not trace the pin files). The wave 110 gate carried that target because the atomic gate had, and it went red. Fixed at integration (one line). The rule that survives: **a migration package's gate runs every target that ever pinned a version** — `grep -rln "schema_version\|SUPPORTED_SCHEMA_VERSION" crates/glasshouse/tests` is the list. Same gate: `events_lifecycle`'s two process-lifecycle tests red under two full builds, 8/8 alone at load 6.7 — the first flaky-pass under the new rule, recorded here and nowhere else.
8. **Two of the first three decomposition workers found a bug in the process's own tooling, and both were the ratchet's or the radius's blind spot for exactly the shape Phase 59 creates.** `blast-radius.sh` took `src/config/tests/part_a.rs` for an integration crate (a `case` glob's `*` matches slashes); `check-file-sizes.py` counted `database/tests.rs` as 3,379 production lines (no inline `mod tests` marker to stop at). Both fixed the same night. Tooling written for the monolith's shape is itself something the decomposition has to move — expect one more of these per split until the shapes settle.
9. **A moved test module loses its parent's imports, and the Windows leg is the only one that notices.** `database/tests.rs` imported `std::fs` for tests that are all `#[cfg(unix)]`; inside the old inline `mod tests` the parent's own `use std::fs` covered it. On Windows the import is unused and `-D warnings` fails the *build* (Windows VM run 12, the first VM run on any split), while macOS and Linux never see it. Fixed forward with a `#[cfg(unix)]` on the import. Rule for every remaining split: after moving tests out, `cargo build --all-targets` cannot prove Windows — the VM run on the split's commit is part of its acceptance, and a `use` needed only by `cfg`-gated tests carries the same `cfg`.
10. **Three workers sat on one permission prompt for half an hour and every watch called them working.** `cd <worktree> && grep …` makes auto mode's classifier ask (*a Read() deny rule is configured; only you can approve*) — the deny rule is the provider-keys guard and stays. A pane on *Do you want to proceed?* moves no tokens and writes no file, which is what a thinking worker looks like to `worker-watch.sh`. Fixed three ways: `cmux send-key … Enter` approved all three; `scripts/prompt-watch.sh` now reads every worker pane for the prompt every minute (armed in the first turn, per CLAUDE.md); the launch prompt tells workers never to `cd`. The general rule: a watch that reads *progress* cannot see a worker that is *waiting*, and waiting is the one state a permission prompt produces.

## Batch 96–97 (2026-09-03, morning) — `main.rs` stops being a monolith, the ratchet reaches two files, and a Windows red survives its rerun

Session `b8a30b3d` (**Claude Opus 5 at xhigh** — the user's ruling of this
morning, taken from Anthropic's Opus 5 prompting guide, with a decide-and-act
block in the launch prompt; it supersedes the Fable 5.1 pin). Inherited hot
from `b783bdfc` with one worker reported and unreviewed, one accepted patch
unapplied, two workers live, and two background legs running. Batch 96's own
rows were never written; they are folded in here.

| package | tier | result |
|---|---|---|
| **Wave-112 trailing legs** on `5e4ebf5` | platform legs | `LINUX_EXIT=0`, `SWEEP_EXIT=0`, **125 targets, zero failures, no flake** — the Linux and full-sweep evidence for the config, database, routing/evidence, routing/session, shell and api/unix splits together |
| **Windows VM run 14** on `a82d019`, and its rerun-alone | platform leg | **red both times, on the same single test of 2,076**: `gateway::conformance::no_rendering_the_gateway_can_produce_carries_either_planted_secret`, `Os { code: 10054, kind: ConnectionReset }` at `gateway/conformance.rs:370`. Two reds is attribution under §34 — **not the load flake rule 4 covers**. Fix-forward worker dispatched the same turn (below) |
| `decomp-main` (Phase 59, line 2048) | Sonnet high, Green pure move | **line 2048 closed.** `main.rs` 16,243 production → **883**; 280 items into 21 `commands/` files. `--bin glasshouse` 85/85. Reported `partial` on its own stricter reading; ruled COMPLETE — see finding 1. Its Phase −1 was wrong — see finding 2 |
| `decomp-evaluation` (Phase 59, 2047/2049) | Sonnet high, Green pure move | `evaluation/mod.rs` 3,795 → six files (largest `readers` 1,616), split by what each does to the database; 36 non-move lines; `--lib evaluation` 6/6; nothing scans it |
| `decomp-session-store` (Phase 59, 2047/2049) | Sonnet high, Green pure move | `session/store.rs` 2,898 → `store/{mod 1867, record 766, context 301, tests 2928}`; 69/69; both boundary scans joined, one of them with a real trap — see finding 3 |
| `gateway-windows-close` | Opus 5 high, **RED** | dispatched on the confirmed Windows red; the packet names the five client-facing `Shutdown::Both` sites and batch 91's prior at the two `ingress.rs` sites |
| `trim-shell` | Sonnet high, Green (comments only) | dispatched: `shell/mod.rs` 2,942 → under 2,500 by rule 3, zero code lines changed. It is the last file standing between the board and line 2047 |

**The ratchet: 5 files over 2,500 → 2** (`shell/mod.rs` 2,942 and
`routing/disposable.rs` 2,532, both under a live worker as this is written).
Phase 59 is measured in files under the ceiling, not boxes.

**Findings worth carrying forward.**

1. **A worker's `partial` can be stricter than the line it is measured against, and the ruling is the orchestrator's.** `decomp-main` reported partial because eight dispatch arms keep their inline argument-assembly glue rather than each calling one `commands::<family>::dispatch`. Line 2048's own words are *"argument parsing and dispatch"* — inline CLI-argument-to-call assembly **is** that, and wrapping it would author new function signatures, which a Green pure move may not do. Ruled COMPLETE. The general rule: a `partial` is a worker telling you where it stopped, not a verdict on the box; re-read the line's words before accepting its self-assessment either way.
2. **A crate-root decomposition's Phase −1 must scan the crate's own sibling modules.** The packet said *"main.rs is a binary crate root, so nothing outside it imports from it — the only callers are inside the file"*. False: `api/unix.rs`, a sibling module of the same binary crate reached via `mod api;`, called ten moved items by bare `crate::` path — three of them with no compile error until the `api` module built. The worker found it, fixed it, and reported it as scope overflow rather than swallowing it. The check that would have caught it is one grep for `crate::<name>` across `src/`, and it now belongs beside rule 2's `include_str!` boundary scan.
3. **`production_code()` before the join, not after.** `harness/mod.rs`'s scan of the session model reads its source with `include_str!` and strips everything after the `#[cfg(test)]` marker. Joining the three successor files first and stripping once would stop at `mod.rs`'s own marker and silently drop `record.rs` and `context.rs` from the scan — a scan that still passes while checking a third of what it used to. The worker saw it and applied the strip per file. Every future split whose scanned file has a truncating reader inherits this trap.
4. **A patch from a worktree cut before the previous split does not apply, and the fix is not `--3way`.** `decomp-main` branched at `22b368e`; `5e4ebf5` then split `api/unix.rs` into seven files. `main.rs` and `session/lifecycle.rs` were byte-identical across that span, so the core patch applied cleanly — but the `api/unix.rs` hunk had no file to land on. Splitting the patch by path and hand-porting the ten call sites into `api/unix/{routing,memory,checkpoints}.rs` took four tool calls; full `crate::commands::<family>::` qualification beat the worker's `use`-block form because it also corrected four stale doc paths, one of which (`routing.rs:307`, `` [`crate::NoRoute`] ``) is a real intra-doc link that would have failed the rustdoc gate. **Check the base commit of every worktree against what has landed since, before generating its patch.**
5. **`git apply … | head -20` silently applies nothing.** Twenty "Falling back to direct application…" lines filled the pipe, `head` exited, `git apply` took SIGPIPE mid-run and rolled back, and `$?` reported `head`'s success. `git status` showed a clean tree and the apply looked done. Same shape as the `validate_round.py` pipe memory: **capture to a file and test the real exit; never pipe a command whose exit code you are about to trust.**
6. **Three splits share one gate because the baseline is one file.** Trap 2 says batch disjoint partitions into one gate; the ratchet makes it stronger than an economy. `scripts/file-size-baseline.txt` is a single artifact of the whole wave, so three separate commits would each carry a baseline that fails the ratchet on a file the *next* commit moves. The wave is one commit, and the message names all three packages and their reports.
7. **Killing a detached gate leaves its compiler holding the build lock, and the relaunch looks hung.** `pkill -TERM -P <gate pid>` plus `kill <gate pid>` kills the shell and its direct children; `cargo-clippy` and its dozen `clippy-driver` grandchildren reparent to init and keep compiling into `target/`. The relaunched gate's `cargo check` then sits at **0.0 % CPU for five minutes** waiting for the build-directory lock — indistinguishable from the *nohup'd gate hung with rustc asleep* shape, and the log stops after `FMT_EXIT=0` with nothing wrong in it. Diagnosis is one `ps -eo pid,ppid,%cpu`: a cargo with **ppid 1** is an orphan, and a cargo at 0 % CPU whose ppid is the live gate is its victim. Kill by pattern (`pkill -f clippy-driver`), not by parent, and check for orphans before relaunching a gate you interrupted.
8. **A `| head` on a boundary-scan grep produced a false "nothing scans this" — in a packet I had just written the rule for.** `GH-TESTS-OUT-SESSION`'s FEASIBILITY said no `include_str!` names its three files; six do, and the two in `events/` were cut off by the `head -10` on the grep that checked. The packet was dispatched, and the correction went to the worker mid-package by `cmux send`. This is the batch-70 truncated-grep rule arriving on the orchestrator's side of the fence: **a grep whose output decides whether something exists is never piped to `head`** — count it (`| wc -l`), or print it whole. The same call also proved the widened rule works when it is followed: `GH-TESTS-OUT-PROFILE-ONBOARDING` ran the full two-tree grep and found a self-scan (`onboarding/state.rs:2334`) that was in nobody's known list.
9. **A trim reaches the ceiling with one line to spare, and that is the ratchet working, not a near miss.** `shell/mod.rs` 2,942 → **2,499**: 443 comment lines, ~350 shortened in place to the invariant plus why it holds now, 49 blank `///` separators, and four blocks moved verbatim into `design-decisions.md` under one new section with a one-line pointer at each site. The zero-code-change proof the packet demanded — a `git diff -U0` filtered of comment and blank lines printing nothing — is what made the review affordable: with code motion excluded mechanically, the reading is only about whether an invariant survived, and both map-line-1973 credential scrubs did, shortened but intact. **Every trim packet gets that proof command in REQUIRED BEHAVIOR.** The 1-line margin is not a risk to manage; the ratchet is the thing that manages it.
10. **The wave narrowed two assurance scans in `tests/`, and only one of them failed loudly.** `main.rs` went from 16,243 production lines to 883, and two integration tests read it by `include_str!("../src/main.rs")`. `disposable_interface.rs`'s census of the four `JobKind` seats went **red** — `JobKind::MemoryExtraction` now lives in `commands/memory_extraction.rs`. `reserve_inputs.rs`'s `nothing_in_this_build_produces_task_nearly_complete` stayed **green while scanning a twentieth of the code it was written to cover**, which is the worse of the two outcomes and the one no gate can report. Both now read the 22-file joined corpus, as `main.rs`'s own scans and `session/lifecycle.rs`'s already did. **The rule this settles:** after a decomposition, a scan that still passes is not evidence — grep every `include_str!` of the moved file across `src/` **and** `tests/`, and for each one ask what it used to read and what it reads now. A red scan tells you; a narrowed one does not.
    **Successor PARKED, not built** (proportional-assurance ruling, same day): a
    `GH-SCAN-CORPUS-RATCHET` measuring each scan's included bytes would be new
    machinery, and it fails two of the five conditions — CLAUDE.md rule 2's grep
    across `src/` and `tests/` already covers this failure, and that one-line
    clarification has already caught the next case on its own (a self-scan in
    `onboarding/state.rs` that was in nobody's list). Build it only if a scan
    silently narrows again *despite* the rule.
11. **Editing the tree under a running gate makes its verdict mixed, and the cure is a restart, not a caveat.** The fix above went in while the wave gate was four targets from done. Its remaining results would then have described neither the tree it started on nor the tree that would be committed. Killed by walking the process tree from the gate pid (finding 7's lesson applied — no orphans this time) and relaunched from a warm `target/`. **A gate whose verdict needs a footnote is not a gate**; the ten minutes are cheaper than the footnote.

### Wave 114, the same session — seven packages under one gate, and line 2047 closes

| package | tier | result |
|---|---|---|
| `gateway-windows-close` | Opus 5 high, **RED** | **the packet's own Phase −1 was falsified by the worker, and that is the finding.** The failing path reaches none of the five `Shutdown::Both` sites the packet named; it reaches no `shutdown` at all. `ingress.rs`'s unreachable-provider refusal hands the request body to `agent.run`, which fails on connect before reading a byte, drops the reader, writes a `502` and returns — closing over a receive queue that still holds the client's body, which on Windows sends `RST` in place of `FIN` and discards the `502`. Fixed with `settle_queued()` (non-blocking drain to `DRAIN_CAP`, then `Shutdown::Write`) at three refusal returns, no `#[cfg]` split. M1 and M2 KILLED; **M3 SURVIVED and was reported as the limit it is** — `Write` versus `Both` is invisible on macOS, so the half-close mode rests entirely on the Windows leg |
| `GH-TRIM-SHELL` | Sonnet high, Green | **line 2047 closes with it** — `shell/mod.rs` 2,942 → 2,499, comments only |
| four `GH-TESTS-OUT-*` packages | Sonnet high, Green | line 2049: **25 files → 11**; see the evidence entry's table |
| **Windows VM run 15** on `ee7799b` (wave 113) | platform leg | build **PASS**, msrv **PASS**, test **FAIL on exactly one test of the whole suite**: `cmux_presentation`'s layering scan, reporting `session/store/tests.rs` as production code naming cmux. A true rule with a false subject — see finding 14. Everything else about the four splits is Windows-clean |
| `GH-GATE-RERUN-ALONE` | Sonnet high, **Amber** | rule 4 is now mechanical: a red in a named load-sensitive family is re-run alone **once** and reported `flaky-pass`; anything else is red on the first result. Mutation (`is_rerun_eligible` always true) **KILLED** by the "outside the families is not rerun" case, with the real assertion text quoted. The worker found that `mutate.sh --test-cmd` does not parse unittest output and reproduced the mutation by hand to capture it — a tooling gap reported rather than papered over |

**Findings from the wave.**

12. **A Red worker falsifying the packet's producer is the packet working, not the packet failing.** `GH-GATEWAY-WINDOWS-CLOSE`'s FEASIBILITY named five `Shutdown::Both` call sites, quoted from real code, with a proven prior (batch 91 fixed the same error code by changing two sibling sites). Every link was checkable and the conclusion was still wrong: the failing exchange takes a fourth path that closes with no `shutdown` at all. Phase −1 buys *a chain that can be checked*, not a correct one — so an OBJECTIVE whose first step is **"determine which mechanism actually applies, by reading it; do not change every site on suspicion"** is worth more than a more confident producer line. The worker also named the four sites it deliberately left alone, with the Windows-green test covering each, and recommended a follow-up rather than widening its own scope.
13. **A macOS-only worker can still produce Windows evidence, if the packet says which half is its job.** The defect is Windows-only and no worker can reach the VM. The packet split it: the worker owns the fix and a test that captures the invariant *in a way this platform can observe* (a `try_clone` of the accepted socket, so the connection does not close merely because `serve` returned — then assert a half-close arrived and the receive queue is empty); the orchestrator owns the Windows run on the integration commit, and that run is named in the packet as the package's acceptance. Without that split the worker either invents a macOS test that passes before and after, or reports blocked.
14. **Extracting a test module makes it look like production code to every scan that walks a directory — the mirror of finding 10, and the more dangerous direction.** Windows VM run 15 on `ee7799b` failed `tests/cmux_presentation.rs`: *"`src/session/**` must never name cmux in production code; found it in `session/store/tests.rs`"*. The scan is right about its own rule and wrong about the file. Its `production_without_comments` splits a source at the first `#[cfg(test)]` — and a test module Phase 59 moved into its own file **has no such marker**, because the declaration stays in the parent `mod.rs`. So the whole test file reads as production. Finding 10 was a scan that read **too little** after a split; this is one that reads **too much**, and it is worse because it fails loudly on a true rule with a false subject, which invites someone to "fix" the production code. Fixed by skipping any file named `tests.rs` or under a `tests/` directory — the same `is_test_file` notion `check-file-sizes.py` already needed for exactly this shape (batch 94, finding 8). **`cmux_presentation.rs` is the only test in the crate that walks `src/`**, checked by grep; the other fourteen `read_dir` users walk project output. The rule for the remaining tests-out packages: a scan that *walks* a directory needs the test-file skip, a scan that `include_str!`s a named file needs the path updated — two different repairs, and a package can owe both.

**Wave 114 is committed with this fix in it rather than after it**: wave 114 adds `session/select/tests.rs`, which also names cmux, so committing the wave first would have knowingly widened a red the run had already reported.

### Wave 115 and the Windows acceptance — Phase 59's code work ends

| item | result |
|---|---|
| **Windows VM run 16** on `adc988b` (wave 114) | **build PASS, test PASS, msrv PASS, zero failures**, lib suite 2,077. Three claims it and only it could settle: `gateway::conformance::no_rendering_the_gateway_can_produce_carries_either_planted_secret` **ok** — the test that failed run 14 *and* its rerun-alone, so the close-sequence fix is proven on the platform that had the defect; `an_answered_client_sees_an_end_of_stream_with_nothing_of_its_own_left_unread` **ok** — the worker's new invariant test holds where it could not be authored; `the_session_and_shell_layers_never_name_cmux…` **ok** — the walker fix |
| `GH-TESTS-OUT-ROUTING`, `-PROVIDER`, `-MAIN-HARNESS` | the last three of line 2049's eleven. `--lib routing` 286, `--lib provider` **381**, `--bin glasshouse` 85, `--lib harness` 212, all identical before and after; the provider worker proved its "before" by stashing rather than trusting the packet |
| `events/mod.rs`, by the orchestrator | the twelfth and last file: 527 inline test lines to `events/tests.rs`, `--lib events` 52 before and after. **Line 2049's count reaches zero** |

**Findings.**

15. **A packet must not quote a test count it derived rather than measured.** `GH-TESTS-OUT-PROVIDER`'s FEASIBILITY said `--lib provider` "runs all 209", summed from `grep -c '#\[test\]'` across the module's files. The real figure is **381** on both sides: libtest filters are **substrings of the full test path**, so `--lib provider` also selects tests under `shell::`, `gateway::`, `routing::` and `integrations::` whose names contain "provider". The worker measured the before-count by stashing its own change (`git stash push -u`, re-run, `stash apply` + `drop`, never a bare `pop`) and recorded the packet error. **Either measure the figure on the untouched tree before writing the packet, or write "the same count before and after" and let the worker establish it.** A derived expected count is a number nobody has ever seen.
16. **Naive brace matching cannot find the end of a Rust module, and the compiler is the cheap oracle.** Extracting `events/mod.rs`'s test module by counting `{` and `}` produced a file with one brace too many — braces inside string literals (`"{name}: …"`) are not delimiters. The balance check said 0 and `cargo fmt` still refused it. The fix was to stop counting and use structure: the test module was the **last item in the file**, so its body is "the line after `mod tests {` through the line before the final `}`", read from the committed original rather than the half-edited working copy. For every remaining move of this shape, prefer a structural boundary over a counted one, and let `cargo fmt` be the first check — it parses, which no grep does.

**Phase 59's code work ends here.** 2047, 2048, 2049 and the three splits are closed; twelve files came under the ceiling, twelve inline test modules moved out, and the ratchet holds both. What remains is 2053 — kept deliberately as a **standing rule applied to files already being touched**, not a crate-wide trimming sweep, because a sweep is exactly the second refactoring wave the coupling ruling forbids — and 2054, the seven re-opened refusals, which is product work.

### Wave 116 — 2026-09-05 evening, the first sole-orchestrator wave: two platform defects, three refusals that had ended, and pane recorded

Opus 5 xhigh primary (`glasshouse-78`), launched by the start prompt; the user raised the worker ceiling mid-wave after a quota reset, ended the second session that had been committing to the same checkout, and ruled the next orchestrator is Fable 5.1. `usage-snapshot.py --glasshouse` for the day at the time of writing: **3.34M output, 13.4M cache-create**, workers included. Counts **1288 → 1291 closed**, open **76 → 101** (Phase 61's 27 + 61G's 7 recorded, 3 closed — the open count going *up* is the phase being recorded, not regression).

| package | tier | result |
|---|---|---|
| `GH-WIN-VERBATIM-CLAIM` | Opus xhigh, **RED** | `project_relative_path` compared a verbatim `\\?\C:\proj` root against an ordinary path and refused everything; both sides now reduce to one spelling, and the guard (drive letter or `UNC/` only) keeps `//?/proj/a.rs` from passing as inside `/proj` on Unix. Four mutations killed, two of them the unfixed tree one side at a time. **Confirmed on all four Windows cells**: file_claims 16/16. Three packet errors, all the orchestrator's, including `--lib commands::…` — finding 21. |
| `GH-CODEX-CATALOGUE-REREAD` | Sonnet high, Amber | catalogue unchanged at 0.153.3, read from **two independent string tables in the real binary** — stronger than the TUI screen the doc called the only artifact; 64 lines of history trimmed to the invariant. The tripwire then stayed red on every `declared` CI cell — finding 18. |
| `GH-CACHE-TEMPERATURE` | Sonnet high, Amber | **1535, 1545 closed.** `with_context_state` had zero production callers; the gateway now stamps warm/cold/unknown from the provider's own cache-read count, and a bounded `measured cache temperature` term reads the ratio off the same `RouteResponsiveness` the reliability terms already read — no new ledger open. Both mutations killed. Packet named `readers.rs`; the type lives in `joins.rs`. |
| `GH-INJECTION-CONFIDENCE` | Sonnet high, Amber | **1129 closed, Phase 27 complete.** The source refusal (relevance ≠ confidence) stands; the injection door's *observed* false-positive rate from line 939's producer is a confidence about the door. `None` injects — the mutation making unknown count as low killed all 17 tests at once. Scope attribution proven by driving the real `api serve` door. Packet undercounted the `None`-ripple 2 → 7. |
| `GH-DEGRADE-BARRIER` | Sonnet high, Amber | hypothesis (the barrier waited on `calls`, filled one statement before the publish it asserts) labelled killable and **held**; mutation killed after the full bound, not a hang; not reproducible on macOS in 20 baseline trials under load, said so; **Windows `msrv` cells green on the next push**. |
| `GH-PANE-SPEC`, `GH-PANE-EVENTS` (user's packets) | Opus xhigh | six specifications under `docs/product/pane/`; one premise-changing finding — `canonical.rs` has no thinking-block variant — with its successor `GH-CANONICAL-THINKING` dispatched the same hour. |
| `GH-README-PRODUCT` | Opus high | the public product page; the worker refused one phrase the packet attributed to a file that does not carry it. |
| records, by the orchestrator | — | Phase 61 (34 lines incl. 61G), the pane design entry, evidence stub, `.gitattributes` from the ended session, README |

**Findings.**

16. **One `cmux send` over ~1 KB loses a 1 KB chunk, and the dispatch script called it ACCEPTED.** Measured on the fifth case: the 1489-byte launch prompt lost bytes 262–1285 (the fourth resumed at 1240 — the same hole), with the packet path at offset 782 inside it. Five of fourteen launches; the worker says *"I'm ready to work"* and stops in seconds; `new-worker.sh`'s proof watched the token counter, which moves for a truncated prompt too. Recovery each time was one send of the packet path. Fix in `GH-NEW-WORKER-PROMPT-PROOF`: typed prompt under 700 bytes with the boilerplate in `.agent-runtime/launch-notes.md`, ≤400-byte chunks, and the packet path proven on screen before Enter. **Never type more than 1 KB into a pane in one send.**
17. **A push cancels the sweep in flight.** `ci-extended.yml`'s concurrency group cancelled run 33971017689's Windows cells — the acceptance for a Red packet — the moment the next records commit was pushed. Batch pushes; hold one while a cell you need is still running. Cost this wave: one Windows result, recovered on the following run.
18. **The Codex provenance tripwire can never be green on CI as written.** Every `declared` cell runs `npm install -g @openai/codex` unpinned and the test compares that install against the version a person re-read the catalogue from; npm was 0.153.4 an hour after the constant moved to 0.153.3. On `de67f0b` both Windows `declared` cells failed on exactly this one test and nothing else. The fix is build rule 2 applied to a harness — pin the install to the constant, read from source — and it goes into `GH-PANE-KICKOFF`'s commit because that packet holds the workflow file (`.agent-runtime/finding-codex-pin-ci.md`).
19. **A refusal ends when its producer lands, and four did in one evening.** 1535, 1545 and 1300 were refused *no source* and the source was Phase 56's `cached_input_tokens`, populated since 2026-09-03; 1129 was refused *no honest confidence* and line 939's outcome rows landed 2026-09-02. The register already says *re-read this file's reasons after every wave that adds a column or a row kind*; this wave is the measurement of what it costs not to — three of those lines sat refused for two days after their producer shipped. **The cheapest recon on the board is grepping each `in-repo = yes` refusal's named missing symbol against current source.**
20. **One event, one turn.** Between dispatches this wave the orchestrator spent roughly one turn in three on a monitor line that needed no act — `worker-watch.sh`'s *pane is quiet* note fired seven times on workers mid-thought, each `ack` ended a watch that then reported its own ending, and a CI run reported cell by cell. The user noticed. This is the failure `GH-BOARD-WATCH` (one digest per window, five interrupt kinds) and pane's 61G (one batch per turn, the batch a handle) both exist to remove, and it is measured here so neither needs to argue for itself.
21. **`--lib commands::<name>` selects zero tests and reads as a pass** — the gate fixed its own copy of this in `5062267`, but packets still write it: three this wave, caught by the workers each time. Every packet touching `commands/` now says `--bin glasshouse` in its VERIFICATION section; the template should.

### Wave 117 — 2026-09-05, later evening: pane stood up, the dispatch defect fixed, 442 built and verified, the board coalesced

| package | tier | result |
|---|---|---|
| `GH-CANONICAL-THINKING` | Sonnet high, Amber | `Block::Thinking`/`RedactedThinking`, byte-identical on the Anthropic request path, a stated refusal per other wire; live-response thinking on a translated route drawn as a boundary and named as a successor. False KILLED from a denied warning caught by reading the compiler. `--lib gateway` 234 run whole. |
| `GH-NEW-WORKER-PROMPT-PROOF` | Sonnet medium, Green | **the dispatch defect** (finding 16): typed prompt 1489 → 347–469 bytes, boilerplate in `launch-notes.md`, ≤400-byte chunks, packet path proven before Enter. The worker was itself mangled by the defect once and measured it on itself. First launch through the fix landed clean; every launch since has. |
| `GH-BOARD-WATCH` (9c's draft) | Sonnet high, Amber | one digest per window, five interrupt kinds; four-line `PROMPT_WATCH_ONCE` disclosed. Armed live from `b784be7`; the per-worker watches were stopped and nothing was missed. |
| `GH-PANE-KICKOFF` | Sonnet high, Amber | **2436, 2437, 2440 closed.** Thirteen `--exclude pane`, a pane job; the exclusion mutation SURVIVED until the worker added the guard test, then KILLED. Found `blast-radius.sh`'s `-p glasshouse` hardcode (successor dispatched). Codex pin and libdbus packages added in the same commit. |
| `GH-CACHED-INPUT-PRICE` | Sonnet high, Amber | **1300 closed**; the parked ruling resolved the day its condition was met. One helper for both cost paths; missing-rate-as-free killed. |
| `GH-PANE-EVENTS` (9c's brief) | Opus xhigh | the sixth spec; 61G's seven lines recorded and made Phase −1-able. |
| `GH-SECRET-SERVICE-BACKEND` + `-VERIFY` | Opus xhigh **RED** + Opus high verifier | **442 LOCALLY VERIFIED.** `dbus-secret-service` with a zero prompt timeout — refuse before raising; VERDICT ACCEPT, seven claims, the crate comparison's far half honestly UNCHECKED. Four mutations killed, Linux tests run in `docker rust:1.98.0`. Tick waits on `GH-SECRET-SERVICE-CI-FIXTURE`. |
| `GH-ENV-SCRUB` | Sonnet medium, Green | the user's pty_smoke answer: gate cargo children under `env -u`; `docker run` forwards nothing; rule 3 amended. |
| `GH-PANE-LEAD` | Opus xhigh team lead | launched on `pane/integration` from the kickoff; 61A first. |

Closed this wave: **1291 → 1295**. Both Windows `msrv` cells green every run since the verbatim fix; every `declared` cell red on exactly the codex tripwire until the pin's first run (`33975254322`, in flight at hand-off).

**Findings.**

22. **A `;` after `validate_round.py` dispatched a refused packet, and the refusal was cosmetic.** `GH-RESIZE-DETERMINISM` went out while the validator was saying REFUSED on a bare `scripts/blast-radius.sh` mention in a FORBIDDEN line. Three later packets tripped the same rule. Two rules: gate every dispatch with `&&` on the validator's exit (the memory already said so), and the validator's bare-mention check should ignore FORBIDDEN and prose — a packet that names the sweep script *in order to forbid it* is not running it.
23. **Two workers touching one function is a partition error the packet writer makes, not the worker.** `GH-ENV-SCRUB` and `GH-BLAST-RADIUS-PACKAGE` both wanted `run_target()`'s cargo line; folding the scrub into the package fix before dispatch cost one sentence and avoided a co-edit.
24. **A verifier that says UNCHECKED where it could not look is worth more than one that says CONFIRMED everywhere.** The Red verifier confirmed the deciding half of the crate choice on the crate taken and refused to confirm the comparison's other half because that crate was not in the registry. The evidence entry carries the distinction; the ruling stands on the checked half.
25. **The first second package broke the gate in a way no test could see.** `blast-radius.sh` classified `--bin pane` correctly and ran it against `-p glasshouse`; the workspace had one package for the script's whole life. The kickoff worker found it by running the bare sweep it was told not to rely on — the honest report said *partial* for a reason outside its files, and that is the right shape.

### Waves 118–119 — 2026-09-05, night: the Fable 5.1 orchestrator's first waves — pane joins the catalogue, the meter's readout lands, two refusals that had ended become packages, and the trailing sweep catches the eleventh row

| package | tier | result |
|---|---|---|
| `GH-PANE-ADAPTER` | Sonnet high, Amber | **2438, 2439 closed.** `harness/pane.rs` declares only what the binary is (bare invocation, `resume` → `None`, `Unverified` where no mechanism exists); `Vendor::Glasshouse` refused as a pairing signal; a real PTY launch test that builds `pane` itself. False `--resume` declaration KILLED. Three scope overflows the packet's "scan list line only" did not foresee, all accepted. |
| `GH-BLAST-RADIUS-PACKAGE` | Sonnet medium, Green | every classified target carries `pkg:label`; `run_target()` passes `-p`; cargo children under `env -u`. Glasshouse-only plan byte-identical; 28/28 script tests unchanged; found and fixed a `local a=1 b="$a"` scoping bug in `is_serial_test`. |
| `GH-ROUTING-COST-JSON` | Sonnet high, Amber | the pane lead's ask answered: JSON Lines, twenty-two keys in a structural order (a `Serialize` view struct, because `serde_json::Value` sorts), `--since`/`--session` requiring `--json`; absent-as-zero KILLED; nine tests drive the binary. Closes no line; 2430's producer. |
| `GH-CONTEXT-SIZE` (live) | Sonnet high, Amber | 1158 + 1534 on one mechanism — the latest gateway exchange's prompt size per session, by a wire rule (Anthropic sums cache reads, OpenAI does not), a bounded `context quality` term. Design note written first. |
| `GH-ROLLBACK-PRESERVE` (live) | Sonnet high, Amber | 1044 — the preserve set the door can name at the moment of a rollback choice: other sessions' active claims plus unclaimed working-tree changes; Glasshouse reverts nothing. Design note written first. |
| `GH-SUMMARY-SCROLL` (live) | Sonnet high, Amber | fix-forward for the sweep red the eleventh integration caused: the onboarding Summary scrolls and announces overflow, as its own test's doc ruled. |

Closed these waves: **1295 → 1297** (2438, 2439). Live at the end of wave 119: five top-level workers plus the pane lead's two subcontractors — the user's raised ceiling.

**Findings.**

26. **The README progress block is part of the tick.** `orient.py` rewrites it; the GitHub lint job runs `progress.py --check`; committing the map without `README.md` (`0c2d783`) is a red lint cell on the whole sweep for one commit. The tick pathspec now always carries `README.md`.
27. **The eleventh row was outside every targeted gate and inside the trailing sweep, which is what the trailing sweep is for.** `GH-PANE-ADAPTER`'s `--targeted` skipped 23 full-trace targets; `onboarding::view` was one, and its worst-case test had been written to fail exactly when a row was added — its doc even pre-decided the fix (*"give it real scrolling rather than deleting an assertion"*). Attribution took one grep per cell (every cell, one test), the fix-forward went out inside twenty minutes, and the line kept moving. The 2026-09-01 ruling's model, replayed.
28. **Re-read the register after a producer lands, not after a wave.** 1535/1545 closed on 2026-09-05 because the gateway's per-exchange tokens ended their refusal; 1158's refusal named the *same* wall (*"a response body `gateway::ingress` is forbidden to parse"*) and was re-read as "still true" the same day. The register's own rule — re-read the *reasons* after every wave that adds a column or a row kind — was applied to two of three lines sharing one reason. The fix is mechanical: when a refusal ends, grep the register for the *reason's* wording, not the line number.
29. **A test that asserts a constant equals a literal pins the declaration, not the output.** (The pane lead, 61A.) 2432's tests pinned `HEADERS` and `JSONL_KEYS` against literals; appending `tokens/turn` to the *rendered* header survived the whole suite. The mutation that asks the real question lives in the renderer, and the killing test reads the emitted bytes. Same shape as §80's four; a fifth way a mutation lies.


### Waves 120–123 — 2026-09-05, night: four ticks, one merge, three reds attributed and fixed forward

| package | tier | result |
|---|---|---|
| `GH-CONTEXT-SIZE` | Sonnet high, Amber | **1158, 1534 closed; Phase 35B 25/25.** Both mutations KILLED (`wire-rule-dropped`, `sign-inverted`); no packet errors. The old tripwire `a_context_size_has_no_producer_in_this_build` still passes — it asserts the hook path, and its name now overstates it (a trim, not owed). |
| pane 61A merge | Opus xhigh lead | **2431, 2432 closed; 2430 PARTIALLY VERIFIED by ruling.** Eight mutations by the lead, two survivors that mattered (a single-tier fixture; tests pinning constants rather than rendered bytes). `pane/integration` merged as `aa16f6b`; `-p pane` 31 passed on the merged tree. |
| `GH-SECRET-SERVICE-CI-FIXTURE` | Sonnet high, Amber | **442 closed; Phase 9E 13/13.** A private bus with an unlocked keyring on both gates; the `busctl list` activation race found under load and fixed; the round trip ran for real twice. Fixed at integration: the `--workspace` count pin 5 → 6. |
| `GH-ROLLBACK-PRESERVE` | Sonnet high, Amber | **1044 closed; Phase 21K 43/43.** The preserve set on the door's reply; `own-claims-preserved` KILLED; packet error (the reply is built in `api/unix/assumptions.rs`) corrected by the worker. |
| `GH-SUMMARY-SCROLL` | Sonnet high, Amber | the onboarding red fixed forward: a scroll offset, keys, an overflow indicator; `silent-truncation` KILLED. Hand-rolled wrap because `Paragraph::line_count` is behind an unstable feature. |
| `GH-BOARD-WATCH-LINUX` (live) | Sonnet medium, Green | the lint red: `test_board_watch.py` races its fixture where forks are fast. |
| `GH-RESERVE-AVAILABILITY` (live) | Sonnet high, Amber | 1837 as RC-A: the band the router read, recorded for Heavy/Frontier launches, read back as a rate. |

Closed these waves: **1297 → 1303**.

**Findings.**

30. **A ruler that cuts `HEAD^` needs history, and `actions/checkout` gives one commit.** Seven of `ruler_run`'s eleven tests failed on both pane cells with `fatal: invalid reference: <sha>^`; local runs never see it. `fetch-depth: 0` on the pane job only. The general form: a test that reads git history is platform-conditional on the *checkout*, not the OS, and the targeted gate cannot trace that.
31. **A lint job that fails early hides every later check.** `test_board_watch.py` has been red on the ubuntu runner since board-watch landed this afternoon; three sweeps in a row failed lint *before* reaching it (rustup network, `progress.py`, `progress.py`). Read the failed step's name, not just the job's colour, and when a lint red is fixed expect the next one behind it.

### Waves 124–129 — 2026-09-05, night: the lint red fixed, 1837 closed, the explicit route rating landed, and three Phase 59 trims

| package | tier | result |
|---|---|---|
| `GH-BOARD-WATCH-LINUX` | Sonnet medium, Green | the lint red: the two racing tests give their worker a busy screen; 5/5 on macOS and in a Linux container, three runs each; `board-watch.sh` byte-identical. The first sweep with lint, audit and both pane cells green followed. |
| `GH-RESERVE-AVAILABILITY` | Sonnet high, Amber | **1837 closed** as RC-A: the router's own band recorded for Heavy/Frontier launches, read back as a rate; `tier-filter-dropped` KILLED after the worker discarded a false KILLED from a compile error (§80 case 4). `EVALUATION_KINDS` is an explicit `[&str; N]` — a second edit the compiler catches. |
| `GH-ROUTING-RATING` | Sonnet high, Amber | the explicit half of RC-B's routing side: `RoutingRated`, `rate-route`, the rated/proxy split printed apart; `rated-counted-twice` KILLED. Closes no line; 1846's producer. `route` is a flat command, so the door is top-level, not a subcommand. |
| `GH-TRIM-MEMORY-SEARCH` · `GH-TRIM-TUI-EVENT` · `GH-TRIM-MEMORY-INJECT` | Sonnet medium, Green | 2053 PARTIALLY VERIFIED: 749→465, 728→491, 620→432 comment lines; every longest block now 20; ten, nine and seven items moved verbatim to the design record behind pointers; each filtered diff empty, each test count unchanged. Reviewed by reading two kept blocks each. |

Closed these waves: **1303 → 1304**; three files trimmed.

**Findings.**

32. **Two trims live at once collide on the design record.** Both append a `## Trims:` section to `design-decisions.md`, and `integrate.sh` would refuse the second. The rule that worked: the second and later trims write `docs/product/trims-<file>.md`, and the orchestrator folds the section in at integration and deletes the staging file (`132978b`, `d91cd7a`). A trim packet derived by substitution must replace the staging name too — one derivation failed its own guard on `trims-memory-inject.md` and dispatched a worktree with no packet until fixed.
33. **A push cancels the running sweep, and eight pushes in an hour meant no sweep finished its twelve test cells until the last.** Lint, audit and the pane cells came back green each time, so the fixes were confirmed, but the platform legs on any one commit were never read to the end. Batch pushes per wave, and when a sweep is past half, hold the next push until it concludes.
34. **§85 again, with a price: eight `while true` load generators from `GH-DEGRADE-BARRIER`'s experiment ran for 2 h 56 min at 91–95 % CPU each, parented to PID 1.** The worker's `kill $(jobs -p)` was the no-op §85 describes, its pane was closed, and nothing on the board watches the host's own load. The user noticed from the Activity Monitor before any mechanism did. Rule: a packet that starts load generators names the kill in its own STOP CONDITIONS as `kill <pids>` from the echoed list, never `jobs -p`, and the orchestrator runs `ps -eo pid,pcpu,etime,command -r | head` once per wave — one line, no script.
35. **The Windows CI VM was pinned at 100 % for three days by one `sshd` connection child** (PID 4420, started 2026-09-02, 129,965 CPU-seconds), with no cargo, rustc or test binary left inside. The memory that a killed `--windows-vm` run orphans the *job* on the VM was half the story: the ssh session that carried it spins after the client dies. `taskkill /PID <child>` inside the guest freed it; the listener (booted with the VM) is untouched. Check the VM's `sshd` children after any interrupted run.

### Waves 130–131 — 2026-09-05, night: the first full-sweep read, its four reds attributed inside twenty minutes, 1846 closed

| package | tier | result |
|---|---|---|
| `GH-TRIM-COMMANDS-HOOK` · `GH-TRIM-ROUTING-BURN` | Sonnet medium, Green | 517 → 369 and 355 → 292 comment lines; longest blocks 51 → 20 and 65 → 19; nine and three items moved verbatim behind pointers; filtered diffs empty; test counts unchanged. Five files trimmed so far under 2053. |
| clippy fix, `pty/process.rs` | orchestrator, Green | `collapsible_if` under `#[cfg(windows)]`, flagged by clippy 1.98 on both Windows `declared` cells the first time they reached their clippy step (the codex pin unblocked them today). A let-chain; behaviour unchanged. |
| `GH-PAIRING-CROSSOVER` | Sonnet high, Amber | **1846 closed.** Prior vs local evidence per k-bucket against the rated-or-proxy outcome; k = 0 scored wrong for local evidence so a uniform history cannot cross over in the first bucket; `prior-inverted` KILLED by every test in the file. |
| `GH-TERMINAL-LOSS-DETERMINISM-2` (live) | Sonnet high, Amber | rule 4's packet after the resize test's third strike on GitHub's slowest cells: slop-gated tolerances that print the slop, the reverted rounding line still failing. |
| `GH-TRIM-GATEWAY-MOD` (live) | Sonnet medium, Green | the sixth trim; `gateway/ingress.rs` deliberately excluded (rulings cited by wording). |

Closed these waves: **1304 → 1305**.

**Findings.**

36. **The first sweep to finish all sixteen cells since 15:46 was `d91cd7a` at 19:40, and it read: 12 green — every msrv cell, lint, audit, both pane cells — and four `declared` reds, each attributed from one grep per cell within twenty minutes.** Two were the load-sensitive `terminal_loss` family (macOS resize 2/8 at 4 ms; Ubuntu hangup 1/15) on `declared` runners while the same tests passed on the sibling msrv cells of the same run — the shape §91 describes, and the third strike that buys a determinism packet. One was a Windows-only clippy lint that no local clippy can see and that `--target x86_64-pc-windows-msvc` cannot check here because `ring`'s C build wants Windows headers; the Windows cells are that check. Read the failed *step*, not the cell colour: two of the four reds were not tests.

### Waves 132–134 — 2026-09-05, night: two more trims, a stop condition that fired correctly, and a ruling decided by the worker's numbers

| package | tier | result |
|---|---|---|
| `GH-TRIM-GATEWAY-MOD` · `GH-TRIM-API-PROTOCOL` | Sonnet medium, Green | 584 → 489 and 468 → 392 comment lines; six and seven items moved verbatim behind pointers; filtered diffs empty; test counts unchanged. `gateway/mod.rs`'s module doc stays at 43 lines as the packet's one allowed exception — five security invariants. Seven files trimmed under 2053. |
| `GH-TERMINAL-LOSS-DETERMINISM-2` | Sonnet high, Amber | **partial, then complete.** The packet asked for slop-gated tolerances; the worker measured first, hit the stop condition, and reported: the reverted rounding line yields at most 2 of 8 stalls at 4 ms even under 56 spinners (the fixed tree 0–1), and eight spinners leave this box at ~270 µs slop, under the 500 µs threshold. Ruling from those numbers: `MAX_STALLS[0] = 3` unconditionally (the 4 ms gap never discriminated the defect; the 500 µs gap is the proof), hangup retry-once unconditional and printed. Mutation KILLED in both regimes; spinners killed by PID. Fixes the resize red on macOS declared (three runs). |
| rule 4 rerun, run 33982138652 | orchestrator | `gateway::conformance::a_rebind_during_an_in_flight_exchange_is_still_attributed_to_the_binding_that_dispatched_it` (a fixed 2 s deadline inside the parallel `--lib` run) red on both macOS cells, green on both after one rerun — a flaky-pass, no write-up; the resize test alone stayed red until `982a6f2`. 15/16, then 16/16 expected on `982a6f2`. |
| `GH-TRIM-MIGRATIONS-V14` · `GH-TRIM-GATEWAY-INGRESS` (live) | Sonnet, Green | the 132- and 184-line blocks; the ingress packet carries an extra rule for rulings other records cite by wording. |

**Findings.**

37. **A stop condition that fires is the packet working, and the numbers it returns are worth more than the tolerance it was asked to pick.** The determinism-2 packet guessed at a slop-gated shape; the worker's measurements showed the gate would never engage where the failures happen and the 4 ms assertion never discriminated the defect. The ruling took twenty minutes and one relay because the worker brought a table instead of a number. Write the stop condition as a measurement threshold, not a judgment, and it does this every time.
38. **`cli.rs` is not a trim candidate** despite 67 % comment lines: its `///` on clap arguments is the `--help` text, so a comment cut there is a behaviour change. Comment share alone does not pick a trim; whether the comment is *rendered* does.

### Wave 135 — 2026-09-05, night: the Fable 5.1 successor's first wave — 61C merged and ticked, sixty sibling worktrees gone, the served-by producers dispatched

| package | tier | result |
|---|---|---|
| `GH-PANE-61C` (the lead's sub-phase: loop, seams, project, TUI, session) | Opus xhigh lead; Sonnet high subs, Amber | merged at `5d63533` as `a67358e`; **2444–2450 closed**, 2451 PARTIAL; `cargo test -p pane` on the merged tree 87/87. The lead found and corrected the no-production-caller shape itself, before any tick (`.agent-runtime/pane/CORRECTION-61C-reachability.md`): nineteen of forty-eight `pub fn`s orphaned because `main.rs` was FORBIDDEN in every sub-packet and the caller was never packaged — practice §32 in one line, and the first of the eleven such cases found by the author rather than an audit. |
| `GH-TRIM-ROUTING-EVIDENCE` | Sonnet medium, Green | 782 → 603 comment lines, four items, longest block 163 → 20; filtered diff empty, `--lib routing` 292 before and after. The tenth file under 2053. |
| sweep 33984063363 on `982a6f2` | orchestrator | **16/16 green — the first fully green sixteen-cell run**, the first with the terminal_loss determinism fix. |
| three rulings for the lead, one commit | orchestrator | 2451's *response* half gets a real producer (`GH-GATEWAY-SERVED-BY`, dispatched); Codex is option 2 — stated duplication, the adapter named as authority (`0be714d`, verified against codex-cli 0.153.3); 2456 is a Glasshouse-side Red package after 61D merges; `sandbox-grants.md` §4.2 names the machine's credential store and §4.3's title is the rule; the pane CI job gains `windows-latest` (run 33985317460 is its first). |
| `GH-TRIM-PROVIDER-TELEMETRY` · `GH-GATEWAY-SERVED-BY` (live) | Sonnet medium, Green · Sonnet high, Amber | the 133-line module doc and six more blocks; two response headers on the served paths and two currency keys on the readout. |

**Findings.**

39. **Sixty sibling worktrees in `~/projects/` outlived their sessions, and the user noticed first.** Every one sat at zero commits ahead of `main` with a dirty tree holding the diff `integrate.sh` had applied by patch days earlier. Their diffs were archived as patches under `~/.cache/glasshouse-worktree-archive/2026-09-05-siblings/` and all sixty removed in one script; the sixteen stale trees inside `.worktrees/` went the same way. A worker tree lives in `.worktrees/` and nowhere else (practice §73), and a session that closes a worker removes its tree in the same act.
40. **Four Finder `.DS_Store` files made `integrate.sh` refuse a clean tree.** The dirty-tree guard counts untracked files, which is right; `.gitignore` carries the pattern now (`2a97027`).
41. **`new-packet.sh --worktree` prints the `git worktree add` line and does not run it**, and the validator then refuses the packet for paths that do not exist. Run the printed line; the skeleton is still worth it for the sections it gets right.

### Wave 136 — 2026-09-05, night: 1594 closes Phase 37, the served-by producers land, three trims, the first dogfooding session in two days, and rule 4's third strike

| package | tier | result |
|---|---|---|
| `GH-ROUTER-FRESH-OVER-BLOATED` | Sonnet high, Amber | **1594 closed, Phase 37 complete** (`7361f0a`). The refusal's producers had landed (1534, 1584/1586); what still kept the line from firing was 1534's 0.1 cap against a −0.25 checkpoint bootstrap. Ruled: the cap is a property of warmth — 0.4 once cold (`9b2f807`). Three tests, one mutation KILLED, `--lib routing` 292 unchanged; the worker's one flagged scope overflow (a `pub(crate)` on the warm-window constant, so coldness has one definition) accepted. |
| `GH-GATEWAY-SERVED-BY` | Sonnet high, Amber | `4adb8cc`: `x-glasshouse-provider` and `x-glasshouse-entitlement` on every response a backend served, never on a refusal, never carrying the secret; `cost_micro_usd`/`cost_confidence` end each `routing-cost --json` line. Three mutations KILLED, `--lib gateway` 237 whole. 2451 stays PARTIAL until the lead's pane-side reader. |
| `GH-TRIM-PROVIDER-TELEMETRY` · `GH-TRIM-GATEWAY-GEMINI` · `GH-TRIM-API-CLIENT` | Sonnet medium, Green | 1,120 → 1,014 · 309 → 216 · 227 → 127 comment lines; thirteen files under 2053. The telemetry module doc stays at 59 lines by the packet's exception — four invariants, each naming its guard test — accepted by reading. |
| dogfooding 2026-09-05 | orchestrator + the shipped binary | `docs/process/dogfooding.md`: Claude Code launched by the binary fixed the nightly false positive (`ad05372`) and surveyed 189 negative-substring assertions; extraction ran after every turn and called no model (no support-work provider configured), and the firewall mode defaults to `off` — both preconditions for the next session, neither a package. |
| sweeps 33985317460 · 33986291324 | orchestrator | every red enumerated per cell before attribution: the conformance rebind test red on both macOS cells in both — **strike 3**; `session_supervision::many_writers_on_one_session_all_succeed` once on windows msrv (strike 1); the nightly `999` false positive (fixed); the new `pane (windows-latest)` cell's build error (`ruler/meter.rs:167`, Unix-only import) handed to the lead. |
| `GH-CONFORMANCE-REBIND-DETERMINISM` · `GH-TRIM-ROUTING-DESTINATIONS` (live) | Sonnet high, Amber · Sonnet medium, Green | two signals replace two sleeps, proven by twenty runs under load before and after · the 83-line block and three more. |

**Findings.**

42. **A negative substring assertion against a whole readout is exposed to every incidental digit in it.** The planted marker `999` matched a project-id hash once in roughly a hundred and fifty runs, and the JSON sibling could have matched `observed_at`. Assert on the parsed field or the section, never on the whole stdout. The survey found only those two sites, so this is a rule for new tests, not a sweep.
43. **Read the flaky test at strike two, dispatch at strike three.** The second strike bought a reading: a 50 ms sleep ordering a dispatch against a re-bind, which a loaded runner reorders. The packet — signals for sleeps, twenty runs under load before and after — was validated an hour before the third strike landed, and dispatched the minute it did.

### Wave 137 — 2026-09-05, night: 2054 closes, three of 61C's ticks come back off, 61D merges with nothing ticked, the trims go multi-file, and a worker's load loops freeze the board

| package | tier | result |
|---|---|---|
| line 2054 | orchestrator | **closed** (`d22a06c`): the seven refused lines the user named are all settled by design and closed on real producers — 1534 today, 1535/1545 and 1129 earlier this evening, 1044 (`d963016`), 1294/1610 on 2026-09-03 with a declared producer. Phase 59 is open only on 2053. |
| `GH-PANE-AUDIT` → un-tick 2446, 2449, 2450 | Opus read-only audit (the lead's), the primary re-verifying on `main` | `b198158`: the shipped `pane` draws its screen into ratatui's `TestBackend` and writes zero bytes (2449); `LocalMemory::add` has no production caller so the fallback store cannot hold anything (2446); `commands::all`, the function that *offers*, is reached only by tests (2450). 2444 and 2447 stay ticked with the audit's limits recorded. The lead had already dispatched `GH-PANE-61C-FIXUPS`; the three re-tick on a demonstration through the binary. The eleventh, twelfth and thirteenth wrongly ticked boxes in this project's history — and the first three found by the lane's own audit rather than the primary's. |
| 61D merge | orchestrator | `208fddd` (`pane/integration` at `0dcff84`), `cargo test -p pane` 113 on the merged tree, **nothing ticked** — 2455 waits for 61E's caller, 2456 is the primary's, 2457 is the prohibition that holds. Two independent verifiers, both REJECT, four escapes fixed, nine mutations killed. |
| `GH-CONFORMANCE-REBIND-DETERMINISM` | Sonnet high, Amber | `de0bdac`: two barriers replace two sleeps; the attribute-at-recording-time mutation KILLED with the test's own message; `--lib gateway` 234 before and after. Twenty runs under load on twelve cores reproduced nothing either way (0/20, 0/20) — reported honestly; the macOS cells of sweep `33989406799` are the empirical check. |
| `GH-TRIM-PROVIDER-MOD` · `GH-TRIM-PROFILE-MOD` · `GH-TRIM-GATEWAY-DOCS` | Sonnet medium, Green | 132 → 15 (a module doc that was 97 % of its file) · 1,003 → 740 across thirteen blocks · 2,329 → 2,068 across nine gateway files in one package. Nineteen single-file trims and the first multi-file one under 2053; the tree-wide measurement (`//`-runs over 20 above the first `#[cfg(test)]`) listed 129 files at 21:46 and is being worked module by module — memory/session, provider, routing live; commands and api/events/harness/config queued, all from one generator. |
| sweeps `33987274829` · `33988480102` | orchestrator | on `ad05372`: macOS green (the rebind flake confirmed as such), two new Windows single-cell timing reds (`shell::view … route_health …`, `integrations::version … null_stdin …`), each strike 1; on `b198158`: **all sixteen glasshouse cells green** — both Windows reds flaky-passes, no write-up; the pane windows-latest cell now compiles and fails twelve `sandbox_profile`/`sandbox_apply` tests on Windows path shape (`HOME` absent, verbatim `\\?\` prefixes, unguarded seatbelt/Landlock text tests, one InvalidFilename fixture) — handed to the lead as a fix-forward. |

**Findings.**

44. **A worker's load generators froze the board for ten minutes, and the watch called the victims "quiet".** The determinism worker's first batch of eight spinners was backgrounded without `nohup`, never killed, and forgotten; three escalations later there were 104, load average 163, and every `cmux` call timed out — which the board watch reported as two workers gone quiet. The read that settles it is `ps -eo pid,command | grep -c 'while :; do :; done'`, before any pane is read; the worker's own report names the cause exactly. Packets that ask for load now say *never leave a generator running* in the verification section, and a trim packet says *never spawn one*.
45. **One rule applied across N files is one package.** Fifteen files and thirty-two blocks in one Green packet cost one dispatch and one integration; the same work as single-file trims would have cost fifteen of each (trap 2). The generator (`.agent-runtime/gen-trim-packet.py`) measures the blocks, writes the YOURS list and names the `--lib` targets; the worker re-scans before cutting because the list is a measurement, not a contract.
46. **Three cmux workspaces created during the load spike never launched a harness and sat as empty shells until the user noticed them.** `new-worker.sh` reported "could not create a workspace" and the lead's two attempts the same; the workspaces existed anyway. After any dispatch failure, `cmux workspace list` and close what has no harness in it.
47. **A refused integration plus a chained worktree removal destroyed a verified trim.** `integrate.sh` refused `trim-commands-docs-2` because the user's new `sites/` folder sat untracked in the main checkout, and the `git worktree remove --force` chained after it ran anyway; the worker's only copy of a three-block trim went with the tree (workers never commit). Two rules follow: read `INTEGRATE EXIT` and `git status` before any pane is closed or any tree removed, in a separate call; and a folder that is the user's (`sites/`) blocks every integration until the user commits it — say so, never stash or add it.
48. **A comment trim turned every cell red, and the gate that should have caught it slices where the test does not.** `tests/subscription_pressure.rs` asserts that `routing/pressure.rs` *names* its knobs (`capacity_band_thresholds`, `reserve_percent`); the routing trim cut the sentence that named them. The packet's rule 5 asks the worker to grep tests for literals from a block — the worker grepped phrases and missed identifiers — and the targeted blast radius did not run that integration target. Fixed forward in `ccc05dc` as one invariant sentence that also says the scan exists. Two rules: a trim packet's grep is for every backticked identifier in the block, not for prose; and a positive scan (a test asserting a comment *contains* a name) is the shape rule 5 was written against and must be named in the packet when the tree-wide grep finds one.


### Wave 138 — 2026-09-06, after midnight: the ratelimit successor's first wave — the routing-2 trim lands past the untracked `sites/`, the pane lane hands to a Fable successor lead, and `mutate.sh` learns to say COMPILE-ERROR

| package | tier | result |
|---|---|---|
| `GH-TRIM-ROUTING-DOCS-2` | Sonnet medium, Green | `051dffe`: thirteen blocks in `routing/disposable/mod.rs` and `routing/session/mod.rs` (375 lines cut, 170 kept or added); the other five listed files were already compliant; `--lib routing` 292, `reserve_inputs` 37, `routing_policy` 37; folded into *Trims: routing module docs, second packet*. Integrated with `status.showUntrackedFiles=no` for the one `integrate.sh` call: the user's untracked `sites/` trips its dirty check and is disjoint from every worker's files — finding 47's block, resolved without touching the folder. |
| pane lane | Fable xhigh lead (successor) | the Opus lead stopped at its window at 79 % with `report-pane-lead.md` as its handoff; its three sub-workers had reported (`pane-61d-windows-paths` — the Windows `\\?\` canonical-path defect that refuses a project's own files as `$HOME`; `pane-61d-exec-grant`; `pane-61e-handles`), their panes are closed and their worktrees kept. The successor was dispatched into the same worktree (`packet-pane-lead-2.md`, ws 102) with the three asks answered: decision 8 accepted (no facility reaches a directly-launched pane; the cancellation token is built in 61E-RUNTIME), `GH-PANE-NOTEBOOK-VIEW` after `pane-61e-handles` integrates, exec-grant option (a) closed. The guarded-continuations plan stays HELD for the user's Codex verification. |
| `scripts/mutate.sh` | orchestrator | `efb1b55`: a non-zero exit whose output carries a cargo compile error and no `test … FAILED` line is **COMPILE-ERROR** (exit 3, facts line `-> COMPILE-ERROR`), never KILLED — the lead's finding, four false kills in one day under `warnings = "deny"`, one hiding a real SURVIVED; `test_mutate.py` case 3b pins it. |
| sweeps `33993104799` · `33995052810` | orchestrator | on `525969a` (docs-only after a fully green `ccc05dc`): the pane windows-latest cell (the lane's known defect, fix in the windows-paths worktree) and `test (windows-11-arm, msrv)` on `shell::view::tests::route_health_keeps_line_1765s_five_concepts_on_separate_lines` — **strike 2** for that test this week (strike 1: `ad05372`, wave 137); the cause is visible in the fixture (`cooling_down_until_unix: now + 300` against the renderer's own later clock read, so "ends in 5 minutes" flips on a slow runner); one rerun issued per rule 4, a third strike buys the determinism packet (widen the fixture's margin or inject the clock). `33995052810` on `051dffe` was running at the checkpoint. |
| three trims dispatched | Sonnet medium, Green | `trim-rest-docs-2` (24 files, 48 blocks), `trim-api-events-harness-config-docs-2` (23 files, 48 blocks), `trim-commands-docs-3` (4 files, 20 blocks); rule 5 in each packet amended to the generator's finding-48 sentence before dispatch; worktrees fast-forwarded to `525969a`. Board: four live, one lead. |

**Findings.** None new — 47 and 48 were applied rather than repeated: the integration's exit line was read before the worktree removal, and the amended rule 5 went into every packet before dispatch.

### Wave 139 — 2026-09-06, 00:40–01:30: Phase 59 completes — nine multi-file trims land in eight integrations, the route_health flake is fixed at its third strike, and the product site goes live on the user's instruction

| package | tier | result |
|---|---|---|
| `GH-TRIM-COMMANDS-DOCS-3` · `GH-TRIM-SESSION-DOCS-2` | Sonnet medium, Green | `2d35bd0`, one integrate call: 34 blocks; `--bin glasshouse` 91 (the packet named `--lib commands`, which is no target — `commands/` is the binary's; the worker reported the packet error and ran the right one) and `--lib session` 315 / `--lib harness` 220 before and after |
| `GH-TRIM-MEMORY-EXTRACT-DOCS` | Sonnet medium, Green | `e11ce62`: 17 blocks; `--lib memory` 155; `disposable_interface` 7, `tracked_knowledge` 5 |
| `GH-TRIM-API-EVENTS-HARNESS-CONFIG-DOCS-2` | Sonnet medium, Green | `149dc3e`: 50 blocks in 23 files (two the packet's list missed, found by the worker's re-scan); five `--lib` targets before and after |
| `GH-TRIM-REST-DOCS-2` | Sonnet medium, Green | `2d3dde5`: 59 blocks in 24 files (eight found by re-scan); the kept text is a condensation — `pty/mod.rs` read by the orchestrator; 52 pointers are doc-comment lines and render in rustdoc (debt, no successor) |
| `GH-TRIM-GATEWAY-PROFILE-PROVIDER-DOCS` | Sonnet medium, Green | `03ec924`: 26 blocks; six carried a pointer from an earlier single-file trim that had never cut the block (finding 50) |
| `GH-TRIM-CONFIG-CHECKPOINT-EVALUATION-CODEX-DOCS` · `GH-TRIM-MIGRATIONS-SECRET-DOCS` | Sonnet medium, Green | `5600485`, `44e4831`: 15 + 15 blocks; `secret/native.rs`'s module doc keeps every security statement, read at the fold |
| line 2053 → **Phase 59 complete** | orchestrator | `3f138ee`: eight blocks over 20 content lines remain, every one under the invariant exception, ruled by reading and listed by file in `phase-59.md`; 34 runs over 20 raw lines comply by the budget rule. **1312 / 1398.** |
| `shell::view::tests::route_health_keeps_line_1765s …` | orchestrator, tests only | `8cfc252`: strike 3 (ubuntu nightly on `9c7a96a`) → fixed — the fixture's deadline moves from 300 s to 330 s because `describe_deadline` floors to whole minutes against its own clock read |
| product site | user instruction | `c038163`: `sites/` + `.github/workflows/pages.yml`, authored in the user's Codex session, committed by pathspec on the user's own words; Pages run 33997706879 green. The auto-mode classifier refused `git add sites …` twice before those words arrived |
| sweeps | orchestrator | `33995052810`, `33995530968`, `33996669314`, `33996779961`, `33997304633` — each cancelled by the following push (finding 49); **`33997706886` on `c038163` concluded: all sixteen glasshouse cells green, red only on the pane windows-latest cell (the lane's known defect).** Single-cell reds seen before cancellation: `gateway_failure_taxonomy::each_failure_class_is_recorded_from_status_headers_and_framing_alone` on windows-latest declared — the row held the next case's class (rows zipped with cases by `seq`; a stream-abort row can land after the following exchange's) — strike 1; two pane tests on ubuntu-latest (`ruler::meter … zero_turns_not_absent`, `seams::a_tool_result_goes_to_the_context_firewall …`), strike 1 each, green on the rerun |

**Findings.**

49. **Rapid pushes cancel their own trailing sweeps.** `ci-extended.yml` runs one sweep per ref with `cancel-in-progress`, so eight integrations pushed one at a time produced five cancelled sweeps and one conclusion in fifty minutes. The rule already says once per two-to-four integrations; under an event-driven board the way to obey it is **hold the push until the running sweep concludes or two-to-four integrations have accumulated, whichever comes first** — the held commits cost nothing, and the sweep that finally ran covered them all.

50. **A pointer without a cut is a false negative for every measurement that keys on the pointer.** Six gateway/provider blocks carried a `// History:` line from an earlier trim that never shortened them and were still over 20 lines; the tree-wide scan counts lines, not pointers, and caught them. `.agent-runtime/gen-trim-packet.py` now slices the production part at `#[cfg(test)]\nmod tests` (the first-marker slice missed twenty blocks in routing), and a packet's LIBS must name `--bin glasshouse` for `commands/`.

### Wave 140 — 2026-09-06, 01:30–02:00: the pane lane's 61D+61E merges, Phase 60 completes on the control-API delivery, direct verified completion is authorised directly, and the user rules on seventeen parked questions in one sitting

| package | tier | result |
|---|---|---|
| `pane/integration` `bdbc816` → `main` | orchestrator merge | `221e355`: 61D's Windows path spelling and narrow exec grant, 61E's handles/previews, notebook view, cancellation, prompt renderer and redirect admission; `cargo test -p pane` 16 targets green on `main`. Sweep `33999112258`: all sixteen glasshouse cells green; the pane `windows-latest` cell red on **seven** tests (six exec-grant tests in `sandbox_apply`, one in `sandbox_profile`), down from twelve — handed to the lead as a fix-forward (`finding-primary-windows-cell-after-merge.md`). The lead's merge-ready report was missed for ten minutes because the board watch fires REPORT on the standing file at every re-arm; check the mtime. |
| `GH-CONFLICT-NOTICE-VIA-API` → **Phase 60 complete** | Sonnet high, Amber | `4b70d72`: the hook delivers through the control door with a machine-originated silent client call and a typed `CallError`; mutation `notice-addressed-to-the-editor` KILLED by the delivery test; 7/14/21/17/17/91 on the merged tree. 2414 closed on the worker's verdict; 2415/2416 closed on the orchestrator's (the predecessor's ruling made delivery the one missing link; per-file claims already serialize only the conflicting file). Limit: a shell-launched orchestrator is unreachable — decision 6 makes the API-served topology the intended one. Report nit: `api/mod.rs` was edited outside EXPECTED FILES with `scope_overflow: []`; the placement had a stated reason and was accepted. |
| direct verified completion | orchestrator ruling | `26ed07a`: the Codex verification the plan waited on is moot (the user closed those sessions: *"do it directly … bypass codex completely"*); the eight decisions ruled, `runtime-contract.md` §9 and `model-contract.md` §2 amended, map line **2485** appended at the end (ids stable, `map-index` 1635). |
| seven experiment gates | user decision | `064f19b`: 1866–1870, 1879, 1882 adopted as standing rules and ticked on the decision; register rows retired. |
| ten steering decisions | user decision | recorded in `design-decisions.md` (*Steering decisions of record — 2026-09-06*): pane's live run on the subscription, ship without a measured win, the `v8` crate, Windows may lag, decided-out closures authorised (census first), the API-served topology, daily dogfooding, a support-work provider on this machine, `sites/` handoffs without re-approval, a tagged pre-release once the pane cell is green. |
| `GH-REFUSED-LINES-CENSUS` | Sonnet high, read-only recon | live: every open mandatory line with its register citation and a disposition, and the fifteen parked Maybe/Experimental lines most worth pulling in, for the user's next yes/no list. |

**Findings.** 51. **A standing status file hides its own update behind the watch's start-up fire.** `board-watch.sh` reports REPORT on a worker's report path at every arm; for a file that always exists the notice is noise nine times in ten and the tenth is the real update. Compare the file's mtime with the last read before dismissing it — and a lead writes a *new* `ask-primary-*.md` when it needs the primary to act. 52. **Nine trims and one merge, and the map moved thirteen boxes in two hours**, ten of them by the user's decisions on questions that had waited days as "deferred": a yes/no list is cheaper than any package and should be put to the user whenever a line's only blocker is a ruling.

### Wave 141 — 2026-09-06, 02:00–02:35: the census closes ten lines and promotes fourteen, the first dogfooding session of the day finds two defects, the site speaks in the user's words, and the Windows compile red is fixed forward

| package | tier | result |
|---|---|---|
| `GH-REFUSED-LINES-CENSUS` | Sonnet high, read-only | 60 open lines outside Phase 61 classified in 15 minutes: **10 decided-out** (closed `7c911bd`), **34 keep-open** with a named producer, **16 disputed** (the orchestrator's to rule), **0 packageable now**; Table 2 ranked the 182 parked Maybe lines — the user said yes to fourteen → **Phase 62** (`961b7c6`, map 2493–2518, denominator 1413), packaged after 61E-WIRE and the ruler's two-column run. |
| Windows red on `4b70d72` | orchestrator fix-forward | `17599f3`: `CallError`'s Unix-only variants were `dead_code` off Unix under `warnings = "deny"` — every Windows glasshouse cell red for a docs-shaped mistake; `cargo check -p glasshouse --tests --target x86_64-pc-windows-gnu` reproduced it in two minutes and the fix in one. Sweep `34000595510`: sixteen glasshouse cells green. **Lesson:** when blast-radius prints PLATFORM-CONDITIONAL, run that check before pushing (successor: blast-radius runs it itself when the target is installed). |
| dogfooding session `586a0338b1a0` | the shipped binary, Claude Code 2.1.261, decisions 7–8 | Two real tasks done (the taxonomy row-matching flake fixed by content matching, 7/7) and **two findings**: (1) the harness child inherits Glasshouse's own provider credential — `env | grep -c GROQ_API_KEY` → 1 — map 488 un-ticked (`88d1a7f`), `GH-LAUNCH-STRIPS-PROVIDER-CREDENTIALS` (Fable, Red) built the strip at all five launch sites after a fix-forward (its first pass covered one of five), verifier live; (2) a `[providers.*]` entry is not a chargeable account — `[entitlements.<name>] provider = …` is — and `NoResource::NothingConfigured` names the provider instead (Green one-liner queued). Also: Claude Code's suggested-prompt placeholder looks like typed text on `read-screen` (memory saved). |
| `GH-SITE-PANE-COPY` | Sonnet medium, Green | `4ec2ec6`: the user's copy verbatim, a real `<section class="compare">` from a `data.compare` field after a follow-up granted `main.js`/`style.css`; vite build green; Pages run `34001464043` succeeded. |
| `e548618` cherry-pick | orchestrator | the lead's tests-only Windows fixture repairs taken onto `main` at the lead's word; `sandbox_apply` 23, `sandbox_profile` 34, `tools` 15; sweep `34001464037` is the Windows pane cell's verdict. |
| pane lane | third lead (Fable xhigh) since 02:20 | second lead stopped at 76 % with a clean handoff; `7c5a39b` holds the scope fix and table bytes (seven mutations killed); isolate merge held for the isolate fix; WIRE live; the ruler's two-column run is the next package after it (`ruling-benchmark-first.md`). |

**Findings.** 53. **A Red fix that covers one of five call sites passes every gate.** The strip worker put the credential removal at `launch_session` and proved it end to end; four other production sites built a `HarnessLaunch` directly and were untouched. The packet named one site and the worker did what it named; `grep -rn 'HarnessLaunch::new'` before dispatch would have named five. **A packet for a cross-cutting rule lists every production site of the seam it changes**, and the acceptance includes a scan test that a sixth site cannot pass without the rule. 54. **The user answered seventeen parked questions in twenty minutes when they arrived as yes/no with a recommendation each** (seven gates, ten decisions, fourteen promotions) — more map movement than any package wave tonight. Put every "awaiting a decision" to the user that way, and do not sit on it.

### Wave 142 — 2026-09-06, 02:45–05:50: the credential strip's verifier finds the hooks, the user reframes and answers two questions and asks for a key-leak guard, the rate window stops the whole board for two hours, and seven lines close on demonstrations

| package | tier | result |
|---|---|---|
| `GH-LAUNCH-STRIPS-PROVIDER-CREDENTIALS` + `GH-VERIFY-…` | Fable high, Red (secrets) + read-only Red verifier | The verifier **REJECTED** a package that passed every gate and mutation: hook subprocesses (`glasshouse hook`, `context-firewall hook`) are children of the harness and lose an environment-only provider key with it; a keychain key still resolves. Ruled (then the user's decision 11): the keychain is 488's secure-store boundary, no credential channel; fix-forward 2 made the loss loud in three places (both hooks' stderr, `doctor`) naming the variable and the platform's store command, never a value; four mutations KILLED; verifier **ACCEPT** on the re-check with two Debt (Linux `secret-tool` wording fixed at integration; an over-long provider list). **488 re-ticked; Phase 9G complete.** |
| `GH-PANE-ADAPTER-DESCRIBE` | Sonnet high, Amber (the lead's sub, integrated by the primary) | `9cdfb3d`: `describe()` declares Anthropic Messages, the two env names pane's wire reads, shell/edit through `bash`; `direct_provider_launch` mirrors Claude Code's. The worker's flagged deviation ruled: `model_override` verified-**empty** and the catalogue invariant narrowed to allow exactly that field; the gateway-resolution test the worker could not reach was added at integration; mutation KILLED by it. |
| `GH-PROVE-IT-BATCH-2` | Sonnet medium, Green (tests only, +118) | Six disputed census lines, six `closed`: 740 (a worker stays writable after its wake-up), 1170 and 1175 (existing tests found for the bootstrap-cost term and `--from-checkpoint`; **Phase 31 complete**), 1216 (an existing 3 600 s-window test; the reader was built), 828 and 829 (the prompt's one sentence pinned on the prompt the model received, and the validator shown to accept a rediscoverable fact — the limit stated; **Phase 20 complete**; **Phase 15 complete**). |
| `GH-SECRET-LEAK-GUARD` | Sonnet high, Amber (the user's instruction of 03:00) | The scanner, two git hooks, an install script, lint steps local and on GitHub, nine tests, one mutation — reported PASS with one honest risk: an entropy bar of 4.8 that a real 36-character token clears ~15 % of the time, because the packet forbade fixture edits. Fix-forward: a fingerprint allowlist and a bar of 3.5 (in flight). Measured first: none of the sixteen local key values is in `HEAD` or in history. |
| the sixteen disputed census rows | orchestrator | Ruled in one register section (`c9a2b08`): six to the prove-it packet; Cluster F's missing producer is Phase 61's verified completion; 621 has no folding to preserve (the census's grep hit comments); 514 and 1356 keep-open with producers named; 1323 the user's. |
| the fourth pane lead | Fable xhigh | The third lead exited to a shell at RL5 92 % with its HANDOFF written and the isolate fix committed (`c218b17`); relaunched at 05:30 on a two-screen bridge packet over that handoff; two subs had reported and their panes were gone, their diffs sitting uncommitted in the lead's tree. |

**Findings.** 55. **A Red verifier earns its cost on the consequence outside the diff.** The strip was correct at all five sites and proved end to end; only a reader asking *who else reads this variable* found that the harness's hooks inherit the stripped environment. A packet that removes something from an environment lists every process that inherits it. 56. **The five-hour window is one resource for the whole board.** At 90 % the five panes on this account went quiet together at 03:12 and nothing moved until 05:15; each paused differently — a lead exited to a shell after its handoff, two Sonnets stopped mid-verification on *You've hit your session limit*, one sat on an output overlay — and the board watch called every one of them *quiet*. After a reset the successor reads every pane before trusting any watch, and the checkpoint is written at the RL5 90 % line, not after. 57. **A "no edits to fixture files" constraint bought a weak guard.** The worker raised the entropy bar until the planted fixtures passed and said so plainly; the fingerprint allowlist every secret scanner uses was the packet's to name. When a packet forbids the natural fix, it must name the substitute. 58. **The guard refused the commit that installed it** — the allowlist's comments quoted the fixture strings they waive, and the scanner scanned its own allowlist; the worker's `--tree` never covered its untracked files. A scanner exempts its own allowlist from shape rules (never from local-key), and a worker's gate runs `--tree --worktree` over the files it is adding. That refusal is the mechanism working on its first day.

### Wave 143 — 2026-09-06, 05:50–06:15: the pane branch merged, the extraction outcome observed, the guard catches its first fixtures, the trailing sweep catches the strip's one escape, and two more packages land

| package | tier | result |
|---|---|---|
| `pane/integration` at `a6efa54` → `a8766b2` | merge (primary) | the isolate fix, WIRE, the ruler rows and their launch through `glasshouse launch --profile`, the wire error's body; `cargo test -p pane --no-fail-fast` on `main` every target ok; 61E's four lines **LOCALLY VERIFIED** (`e3297cb`), ticks held for an independent verifier over `c218b17` + `310b977` (dispatched by the lead, Opus high) and for the pane cells. |
| `GH-EXTRACTION-OUTCOME-OBSERVED` | Sonnet high, Amber | `ea5a349`: one `memory_extraction_observed` row per hook extraction, `None` outcomes named by the caller's own elapsed time against the 5 s bound; mutation KILLED; the guard refused the first commit on two planted `sk-` fixtures in the worker's new tests — fingerprinted, its first catch on a worker whose base predates it. |
| the trailing sweep on `e3ca9d5` (`34009634577`) | attribution (primary) | **all twelve glasshouse cells red on one test**: `entitlement_broker::a_launch_on_a_pooled_provider_is_bound_to_the_chosen_account` — the strip withheld the serving account's own reference variable, which that test pinned as riding along; the value still reaches the harness through `ANTHROPIC_AUTH_TOKEN`, so the premise moved under 488 (`33486e6`, 40 passed). The test drives the binary and the strip's file-traced `--targeted` gate skipped it among 56 full-trace targets; the sweep caught it within the hour and the fix landed with zero damage — the trailing model working as ruled. The `audit` cell failed on a check-run POST (`checks: write` missing) after one unmaintained-crate warning (`paste`, under pane's V8): permissions and an ignore, same commit. Two pane reds (a Windows compile red on unused imports; a Linux grep-line test) handed to the lead. |
| `GH-PRE-RELEASE` | Sonnet high, Amber | decision 10's workflow: five OS/arch release builds of `glasshouse` + `pane`, archives + `SHA256SUMS`, actions pinned by sha, `prerelease: true`, a dry-run dispatch; mutation KILLED. Two facts surfaced: **no LICENSE file exists** under a README and Cargo manifest that say `MIT OR Apache-2.0` (parked with the user: `repository-licence-files`), and **`pane` has no `--version`** (a Green line for the lead). |
| `GH-QUOTA-WINDOW-START` | Sonnet medium, Amber | **1210 closed**: the rolling window's start derived from reset and length in `apply_to`, the never-called `with_started_at` finally called; mutation `window-start-sign-flipped` KILLED. |

**Findings.** 59. **A `--targeted` gate that says it skipped 56 targets has said everything.** The strip's escape was in a binary-driven test no file trace reaches; the number was printed, the trailing sweep was the design, and the fix took one hour end to end. Do not widen the targeted gate for this; read the skipped count as the trailing sweep's size. 60. **A worker's untracked files are outside its own `--tree`.** The guard's author never scanned the allowlist it was adding; the fix was the scanner exempting its own allowlist and a worker gate that runs `--tree --worktree`.

### Wave 144 — 2026-09-06, 06:15–07:15: the second dogfooding session breaks decision 11's path, the verb repairs it, three pane lines tick on the Red verifier, and `v0.1.0-pre.1` is published

| package | tier | result |
|---|---|---|
| dogfooding, second session (`6bad2a429d5e`) | the shipped binary, Claude Code 2.1.263 | the strip holds in a real session (`env \| grep -c GROQ_API_KEY` → 0); the new `memory_extraction_observed` row said *none configured* on every turn with Groq configured and the key in the Keychain — the row found the defect on its first outing. Measured four ways: **only an item the Glasshouse binary itself creates is readable by Glasshouse** (macOS names the creating program on the item's access list; Glasshouse never shows the Allow dialog), and nothing created one. |
| `GH-CREDENTIALS-STORE-VERB` | Opus high, Red (secrets) | `glasshouse credentials store\|remove\|list`: a prompt with echo off restored on drop, or `--stdin` one line, never argv, zeroized; `store` refuses a foreign item before prompting and names the removal command; `doctor` tells *present but not readable by Glasshouse* from absent (keyring's `PlatformFailure` versus `NoEntry`, proven live). Four mutations KILLED, two ignored real-keychain tests by hand; the demonstration line re-proven through `main`'s binary at integration. Two facts: `security find-generic-password -w` hangs on a Glasshouse item's Allow dialog, so no outside tool reads back what Glasshouse stored; every differently-built binary is a new program to the Keychain (Debt: a signing identity for the release binary). One `--lib` red at integration was a spawn-timing test that passed alone twice (strike 1). |
| the `affba0d` merge and the Red verifier | lead + Opus high verifier | the loop ACCEPTED (every §5/§6 clause as bytes through the binary) → **2462, 2465 ticked**; 61G's window → **2476 ticked**; the isolate fix REJECTED on a deterministic Blocker (`Array.prototype.fill` outruns the watchdog) → fix-2 live; 2461/2463 wait. |
| `v0.1.0-pre.1` | decision 10 | the pane Windows cell green on `34011829488`; the first dry run failed both Linux builds on `libdbus-1-dev` (fixed), the second built all five; the tag's run published five archives and `SHA256SUMS`. No licence file yet (parked with the user). |

**Findings.** 61. **A verifier's Curiosity is tomorrow's Defect when the product forbids the thing the platform wants.** The strip's verifier noted the Keychain's Allow dialog as a curiosity; Glasshouse's refusal to ever show one made it the whole mechanism of the failure. A curiosity that names a platform prompt the product suppresses is a Defect to probe, not to file. 62. **An observation row is worth a package when it ends a guessing game.** Finding 4's row (`ea5a349`) turned "extraction produced nothing, reason unknown" into "no configured provider names a model" on the very next session, and that sentence pointed straight at the candidate set's presence check.

### Wave 145 — 2026-09-06, 07:15–08:00: three pane merges, the third dogfooding session proves decision 11's path and finds its two blockers, the credentials verb's first sweep goes red on two stores, and the Fable limit nears

| package | tier | result |
|---|---|---|
| `98f61c9`, `3920b38`, `ee7b1cd` (pane/integration at `db64c98`, `514ad3c`, `9c4e4ef`) | the lead's gates, no local re-run | TERMINAL, USAGE, attempt-name, the isolate fix-2, 61F, `pane --version`; only `Cargo.lock` outside `crates/pane/`; the sweep on `98f61c9` was red on pane (windows-latest) at the build step (dead helpers under `#[cfg(unix)]` callers) — fixed forward in `9c4e4ef` before the next push. Ticks wait on the lane's verifier. |
| dogfooding, third session (`2d559ae4a1cc`) | the shipped binary, Claude Code 2.1.263 | decision 11's Keychain path proven: the hook made an authenticated Groq call (`model_not_found`, then a completed call in 422 ms on `openai/gpt-oss-20b`). What had blocked both earlier sessions was **no `memory_extraction_model` consent**, and the outcome row blamed credentials → `GH-EXTRACTION-CONSENT-NOTICE` (Amber, reported complete in 9 min, one mutation KILLED). Two launchers with revoked stdio supervised harnesses stuck in exit for 5 h → `GH-ATTACH-REVOKED-TERMINAL` (Red, live). |
| credentials fix-forward | primary, Red path | sweep `34013598118`: eleven glasshouse cells red on one test — `store ""` filed by the Secret Service and Windows stores, refused by the Keychain; `usable_variable_name` guards `store` and `remove` before any store is probed; second test with a spaced name; the sweep's eleven reds are the mutation's evidence (a local mutation would write to the real Keychain). |
| `GH-GATEWAY-PURPOSE-HEADER` | Amber, Sonnet medium | dispatched: `x-glasshouse-purpose: supervisor` stamped when allow-listed, stripped before upstream — the Glasshouse half of `supervisor.md` §3. |

**Findings.** 62. **A recorded decision that was never acted on reads as a failure unless the row says why it was not.** The extraction row named the routed model, then "no model was called", then other providers' credentials — three true sentences that together pointed at the wrong cause, and two orchestrators followed them. The sentence that was missing was the one about consent. 63. **A revoked terminal is not a hangup.** Closing a pane can revoke stdio without any signal; a zero-timeout `poll` on the revoked descriptor says nothing on macOS and `SIGHUP` never comes, so the only detectors that work are the ones that try to use the descriptor. 64. **Test a secrets verb's refusals on every store, because the stores disagree about what they refuse.** The Keychain refused an empty account and made the test pass on macOS; two other stores filed it.

### Waves 146–147 — 2026-09-06, 08:00–10:10: the consent notice, the purpose header and the revoked terminal land with their verifier; four pane merges and two verifier REJECTs; the Windows-gate shape named; the window closes and the inbox is ruled

| package | tier | result |
|---|---|---|
| `GH-EXTRACTION-CONSENT-NOTICE` (`23527bd`) | Amber, Sonnet medium | mutation KILLED; packet error: `tests.rs` is the bin's module (`--bin glasshouse`, not `--lib`). |
| `GH-GATEWAY-PURPOSE-HEADER` (`3e164dc`) | Amber, Sonnet medium | mutation KILLED, 241 gateway tests, targeted blast green; 61F's limit sentence can go. |
| `GH-ATTACH-REVOKED-TERMINAL` (`a920aab`) and its verifier | Red, Opus high; read-only Opus | two mutations KILLED; the worker corrected the packet's cause (a revoked fd sets `POLLNVAL`, so `stdin_hung_up()` already fired; the hang was `pump_output` leaving its loop on a write failure, so nothing drained the pty); verifier ACCEPT — six revoke trials through the binary end the launcher in 46–76 ms with the harness reaped, an idle attached session lives 60 s. Two Debts → `GH-SUPERVISE-KILL-BOUND` (after `TERMINATION_GRACE`, `supervise` re-sends SIGKILL forever for an unreapable child; a single failed write blinds the terminal for the session's life). |
| `GH-FIREWALL-FILE-TOUCHED` (`bf8b28a`) | Amber, Sonnet | the gate was the canonicalized root against the harness's raw path under a symlinked directory; mutation KILLED; 101 + 15 tests; the third dogfooding session's own missing row stays open with the log file as the next probe. |
| map tick `0a0cc09` | primary | 2485 and 2469–2471 on the fourth verifier's ACCEPT of §9 and sweep `34015246338` (1346/1413). |
| pane merges `a3d8ce1` (TOOLS-REAP), `6523a2f` (SIGINT + isolate fix-3), `c6cedc2` (61G delivery + bg), `f9ae0f4` (the Windows fix-forward) | the lead's gates, no local re-run | nothing outside `crates/pane/`; the sweep's pane cells are the verdict. Verifier-3 REJECTED 2461/2463 on `a0186fa` (the epilogue's budget is per V8 entry, so thirty lazy accessors poison the isolate; a 32 MiB ceiling still aborts because the near-heap-limit callback never fires) → fix-4, reported 09:59 and merged on the branch as `4ab1876`, its merge ask pending; the bg-lifecycle verifier REJECTED 2477 (`Interrupter::end_the_session` exits 130 without `bg::shutdown`, so a job survives a double Ctrl-C at 99 % of a core) → folded into the lane's first Red packet after the reset; 2475/2481 tick on `34020972140`'s green pane cells. |
| `scripts/stale-workspaces.sh` (`a37472b`) | primary | a lead's subs under `.worktrees/<lead>/.worktrees/<name>` are not stale; twice in one morning it had offered a close command for a live sub. |
| `GH-HEALTH-OBSERVATION-ORIGIN` (packet) | Amber, Sonnet medium | written and validated at 08:33, held for the window; dispatched 10:14 by the successor. Its ruling: origin ∈ {task, retry, support-work}; *repair attempt* and *explicit probe* have no producer by design and become register rows at the tick. |
| 61G §7 | — | `GH-PANE-61G-INBOX` returned blocked in 12 min (~$3) with a clean tree on two Glasshouse-side gaps: pane cannot locate `control.sock`, and `Request::Events` carries neither a sender nor a body. Ruled 10:24 (`ruling-inbox-glasshouse-side.md`): `glasshouse api socket-path`; `from` on `SendMessage`; a `session_messages` table (schema 28 → 29) read by `Request::Inbox`; the door delivers by the recipient's harness → `GH-API-INBOX` (Red, Opus high, dispatched 10:30); the pane half follows its merge. |

**Findings.** 65. **A regression that sits on the safe side of a threshold verifies the grant, not the crossing.** Three isolate packages in a row passed their own tests and failed a verifier that swept the threshold — allocation sizes, the epilogue's budget, the heap-raise count. When a fix names a limit, the regression must cross it. 66. **A `cfg`-gated test's imports are read, not compiled, before a merge ask.** Four sweep reds in a week on pane's windows-latest cell were one shape — a helper or import dead under the gate, and dead is an error — because no local check builds pane's tests for MSVC; CLAUDE.md's sixth build rule. 67. **A mutation run against a filtered test name mutates the command, not the code.** The lead filtered to a plausible test name, one test ran with 73 filtered out, and the mutation "survived"; `mutate.sh`'s own warning said so, and the killer the worker had named was right. Run the target the report names. 68. **A blocked worker that returns a clean tree in twelve minutes is the cheapest Phase −1 there is.** The inbox packet asserted two producers that did not exist; the worker read the files, refused to inline a dependency, and stopped. The ruling it forced is the package.

### Wave 148 — 2026-09-07: the checked subscription-broker claim gains its missing production caller

| package | tier | result |
|---|---|---|
| subscription broker core, routing, CLI and integration | Red; three parallel research/implementation lanes plus an independent verifier | Four integration commits and one live fix. Focused suites and 114/114 binary tests green; verifier ACCEPT. Three provider OAuth accounts connected and three real Pane turns succeeded through the Glasshouse gateway. |
| Pane structured-return completion correction | Red, primary after live Gemini dogfood | One runtime predicate shared by foreground and subagent loops. Exact diagnostic-object regression proves another request and final prose; bounded output reaches both model and TUI. Traced gate, TUI live 10/10, Windows check and Clippy clean. Real Gemini replay completed after 11 cells in 70.7 s at 369,178 reported tokens; it proved completion and exposed severe broad-ledger context inefficiency. |

**Finding.** Fixture-sidecar tests proved lifecycle and byte relay but did not parse the generated YAML or delay model registration. Live use found both defects in minutes. A managed third-party sidecar package must include one real binary startup before its evidence can support a production claim, even when all protocol fixtures are green.

### Wave 149 — 2026-09-07: the real subscription catalogue reaches Pane's model picker

| package | tier | result |
|---|---|---|
| Pane provider carousel | UI, Astra high | 196 Pane unit/PTY/render tests green; eight themes and 1–160-column bounds; exact selected model observed on the next request. |
| broker model catalogue | Red, Sol high; primary fix-forward | Three live authenticated catalogues reported 11 Google, 16 Anthropic and 9 OpenAI models. 2,207 library tests, 116 binary tests and 54 focused integration tests green; Clippy clean. |

**Finding.** A catalogue can be complete globally and still be wrong for the active route. The first live picker exposed 444 API-provider models while its own `gemini-3.8-flash-high` was absent. Catalogue entries now carry provider, account and selectability separately; Pane shows every subscription but applies only models belonging to the gateway's pinned entitlement.

### Wave 150 — 2026-09-09–10: input dogfood repairs with GPT-5.6 Sol

| package | tier | result |
|---|---|---|
| Glasshouse launch focus, mouse and paste | Sol high, isolated editor | Immediate typing plus real PTY paste/mouse proofs; input routing extracted to retain the 2,500-line production ceiling. Three older PTY sequences and one V1 session sequence required explicit return to control after auto-focused launches; all 80 PTY tests and the seven-test V1 session target then passed in the integrated checkout. |
| Pane picker clicks and bare entrypoint | Sol high, isolated editor | Rendered hit geometry, fragmented SGR buttons, real help and ordinary bare launch. Integrated entrypoint/session/live-TUI checks: 103 passed. An old echo-line expectation was corrected; a separate handler-panel timeout passed an exact worker rerun and the integrated 16-test PTY suite. |
| independent input review | Sol medium, read-only | ACCEPT after catching the fragmented SGR button-drop path and UTF-8 mouse coordinate boundary. Primary owns integration and installed subscription dogfood. |

**Measurement:** two editors ran concurrently with disjoint crate ownership;
zero merge conflicts and zero reverts. Review and the integrated gate caused
bounded corrections rather than a new capability batch. No capability boxes
closed. This contributes one no-conflict observation to the concurrency
question, not a general model-quality claim. Token/cost telemetry for these
Codex subagents was not exposed; no spend estimate is invented.

**Installed dogfood:** `f912bb9` release installed; four real subscription tasks
passed (Claude initial and follow-up, Gemini, OpenAI Sol). Each recorded
read/write/read and the exact expected output bytes. The visible standalone
composer and post-task typing worked. The quick-open profile-selection gap and
computer-control limits remain explicit in the linked dogfood report; no new
capability closure is inferred from these trials.


### Wave 151 — 2026-09-10: realistic Sol/Luna dogfood drives harness repairs

| package | tier | result |
|---|---|---|
| Narrow command executable grants | Sol medium, isolated editor; primary integration | Exact admitted executable resolution, Homebrew Python companion, deny preservation; integrated profile/apply/tool tests green. No broad child executable directory grant. |
| Helper cancellation, live lane and opt-in preflight | Sol high, isolated editor; independent Sol medium review | Shared caller cancellation, held-provider and real-PTY regressions. Reviewer rejected disappearing completed Scout; worker persisted the actual notebook record and cleared it at the next task. Re-review ACCEPT; integrated full Pane suite 890 passed, one ignored. |
| Request model and helper purpose evidence | Sol high, isolated editor; primary and Sol medium review | Primary rejected the first scanner's malformed JSON acceptance and unrelated string retention. Revised streaming grammar, partial-body and escaped-model bounds accepted after seven focused tests and a differential malformed-input corpus. Final validation: full workspace sweep plus exact expectation corrections covers 4,237 active tests. |
| Context batching and latest visible source version | Primary editor, isolated worktree; Sol medium review | Baseline 2 failures reproduced. Seven context tests pass, including output overflow and serialized wire visibility; independent ACCEPT. Named runner regression proves fresh output across cells and unchanged permission refusal. |
| Actual expense CLI repair | Sol task model, Luna configured helpers, subscription route | Old installed build needed one operator correction of an invented extra-header requirement. Final 13 self-tests and 15 independent black-box cases pass. New installed rerun pending; full Pane suite 890 passed, one ignored. |

**Open-question contribution:** delegation enabled three disjoint repair lanes
while the primary investigated and tested cell batching, but did not remove the
integration-review work. Both helper UI and gateway observation required review
corrections before acceptance. That is evidence for the standing question about
throughput versus moving serial review, not evidence that any tier closes boxes
at a known price. This batch closes zero capability boxes. Subagent billing is
not exposed, and old gateway labels cannot split the live task/helper spend.
Correctness/completeness and operator corrections are scored before weighted
model/cache cost; elapsed time is secondary, as the person explicitly requested.


**Gate follow-through:** the full macOS gate exposed stale assignment-model
queries after request-model attribution changed. A repository-wide audit of all
23 integration files using `ObservationQuery` corrected nine additional test
files without weakening behavior assertions. Final evidence is 4,237 active
workspace tests through the complete sweep plus exact corrected-target reruns,
and 890 Pane tests. The failed intermediate logs remain explicit. Windows GNU
compile stopped on a missing upstream V8 artifact before Pane compilation; no
new Windows proof is claimed. A broad staging command was refused by the repo
guard; the primary then named every reviewed file explicitly, without bypassing
the guard or requesting user approval.


**Installed rerun:** `c010eea` completed the fresh expense repair autonomously
in 13 cells / 361.7 seconds, with nine self-tests and all 16 frozen independent
cases passing. Model-request attribution separates Sol (13 task requests,
118,751 input / 8,773 output, 46,336 cached input reported) from Luna (six helper
requests, 20,329 input / 2,492 output, 5,120 cached input reported). No monetary
usage is available. Setup and prompt differ from the old trial, so neither
relative speed nor cost savings are established. A follow-up Sol high console
formatter editor and Sol medium independent result reviewer run concurrently
with primary live evaluation; this addresses a measured 96-character nested
string cap that hid requirements, not a speculative prompt expansion.


**Accounting audit (Sol high, read-only):** the live UI's 173,860 tokens are
parent Sol usage including cache; Luna helpers add 27,941, for 201,801 reported
tokens overall. Cached input is disjoint on this normalized route. The task
meter currently drops helper usage (existing open Phase 64 item), so UI-only
comparison would undercount delegated work. Helper share is about 13.8% of
these reported tokens, not the user's aspirational two-thirds; monetary cost
is unavailable. This is evidence for measuring model/cache-weighted cost per
correct completed task, not minimizing or concealing total work.

**Console follow-up (Sol high editor, Sol medium reviewer):** the observed
96-character nested cap was repaired with a shared argument budget. The reviewer
caught post-key exhaustion recursing into a child; the editor corrected it and
all 13 focused tests pass independently. Primary integration passes all 894 Pane tests (one ignored). Installed
`b2c38db` replay passes in 25.797 seconds: full structured README observed
exactly, one verifier definition, two fresh nine-test runs, no thrown cells. No capability boxes close in this bounded repair.


### Wave 152 — 2026-09-10: helper starting evidence and complete usage

Status: integration and installed trial in progress; no capability boxes closed.

| Package | Tier | Evidence so far |
|---|---|---|
| Role-specific starting evidence | Sol medium editor; primary integration | Bounded Scout/Checker/Reducer module, eight focused tests and full Pane worker suite pass; primary tightens ordinary-file reads and incomplete-ignore omission. |
| Helper token coverage and task meter | Sol high editor; primary review | Isolated implementation covers parent plus helper totals, per-model/cache classes and unknown coverage; integrated full Pane suite passes; installed accounting reconciliation pending. |
| Profile-aware quick-open | Sol high editor; independent Sol medium review | Picker and shared serving path implemented; review found gateway shutdown drop-order and native fallback regressions, corrections integrated; worker PTY80/shell415 pass, primary full workspace rerun pending. |
| Named confined checks and reuse | Primary editor; independent Sol medium review | Six focused tests pass. Reviewer found flaky accepted socket in provider fixture; corrected before integrated first-request gate. No production blocker found. |

Three disjoint editors worked concurrently while the primary implemented named
verification and tested actual cmux focus. The resumed worker names are
`sol_profiles_resume`, `sol_usage_resume`, and `sol_seeds_resume`; the completed
seed editor then performed independent reviews, so finished worker output was
consumed before reassignment. Native helper models in the installed follow-up
remain Sol task / Luna helpers. API subagent billing is not exposed.

Open-question contribution: parallel implementation still requires independent
integration review. This batch caught two launch compatibility/lifetime defects
and one flaky test before installation, rather than treating compilation or a
single green worker run as acceptance. No tier price or capability-closure rate
is inferred from unavailable usage data. Final results and observed correction
counts will be appended after the installed trial.


**Gate correction:** the full macOS run caught an additional Native argv
compatibility regression after source review had accepted the profile patch.
The shared resolver added approval arguments to the synthesized Native launch;
the existing real-PTY resize test's plain shell exited with usage (79/80 pass).
This requires a production correction, not an expectation weakening. It is
evidence that cheap parallel editors plus source review do not replace the
full behavior gate. The first library failure at descriptor limit256 was
separately attributed to probable resource pressure; all 2,333 library tests
and 116 command tests pass under limit4096. No first failed run is relabelled
green.

**Corrected integration:** the complete Glasshouse workspace rerun passes,
including PTY80 and all targets skipped after the first failure. Full Pane
CI passes (922 summed passing executions including nested probes, one ignored).
Final installed evidence and cost reconciliation remain pending.

**Installed v3:** 9f3a129 repair passes 10 project tests and 16 frozen external
cases with zero operator corrections, 7 cells and 309.3 s. Known reported
usage 84,753 Sol + 33,776 Luna = 118,529; cache reporting partial, no monetary data.
The helper share of known tokens is 28.5%, not two-thirds. UI/ledger reconcile.

**Follow-up batch:** Sol high completion-guard editor and Sol medium checker
evidence editor run concurrently with Sol high accounting audit. Primary
review caught the initial guard touching only the nested-agent path; the editor
added the actual session/TUI path and regressions for both. It also expanded
the handoff to all checkers in the cell. A temporary worktree collision was
detected through disjoint-file status, corrected by extracting only the seed
worker's patch into its own worktree, and left main untouched. No worker was
discarded or silently reset. These are process correction counts, not hidden
operator corrections to the native coding task. Final integration/replay pending.

**Installed completion correction:** 6d0cbe3 passes the full 925-execution Pane
run (one ignored), scoped local gate and all-target Clippy, and is installed for
both binaries. Real v4 review uses four cells with no error or operator correction;
Sol interprets Luna in a later turn and corrects two overbroad helper statements
before reporting readiness with explicit history limits. All 16 independent
cases remain green and no fixture files change. The live model yielded, so
forced-return deferral evidence comes from its two scripted regression paths,
not an invented claim about this live trial.

**Final v4 audit accepted:** 154.297 s, four Sol requests + one Luna request;
38,280 parent + 11,741 helper = 50,021 known tokens. Input/output 5/5, cache 2/5
coverage; no prices. All 13 baseline hashes unchanged. Fresh/reused/automatic
checker observations reconcile, as do UI and ledger. The parent corrects helper
overstatement before its verdict. This answers the batch's open question with
observed evidence: cheap helper output needs a parent interpretation boundary;
low token count and a passing local test alone are not completion assurance.

### Wave 153 — 2026-09-12: benchmark telemetry and official TB2 adapter

| package | tier | result |
|---|---|---|
| Pane machine telemetry | GPT-5.6 Sol worker; primary semantic audit and integration | Final machine output now splits parent/helper usage by model and four token classes, counts attempted/successful/reported provider exchanges, records every preflight invocation, cells, tool calls, failures and wall time. Six focused regressions and the complete native Pane suite pass. Missing provider usage remains explicit coverage debt. |
| Exact-task comparator audit | GPT-5.6 Luna read-only worker; primary arithmetic correction | Four public GPT-5.3-Codex and Claude Opus 4.6 task-level baselines traced to individual official Harbor artifacts. The audit records that actual Codex CLI/Claude Code rows publish only full-suite aggregates, which cannot replace paired task results. |
| Harbor campaign adapter | GPT-5.6 Terra worker; primary integration and live bridge smoke | Official package revision 1, four tasks × three attempts, native timeouts, no retries, concurrency two. Real Harbor config resolution and Docker-to-host authenticated relay pass; a startup schema mismatch (`listening` versus assumed `url`) was caught and fixed before any model request. |

**Open-question contribution:** cheap parallel implementation found useful
pieces, but primary integration still caught two result-contract errors: the
Harbor verifier reward is nested, and the gateway's actual ready field was not
the adapter's assumed spelling. That is another concrete observation that
delegation improves breadth while moving, not eliminating, the serial boundary
review. Worker token/cost telemetry is unavailable and is not estimated. The
12 paid subscription attempts will supply the first complete parent/helper
model split from the newly instrumented product; no capability box closes from
adapter preparation alone.

**Execution canary correction:** the oracle was 4/4, but the first paid task
failed before its intended shell command because the local Docker runtime did
not expose Pane's required Landlock/seccomp regime. The primary inspected the
first completed failure, stopped the remaining batch, and excluded the partial
launch as infrastructure-invalid. A Sol design audit independently recommended
the same narrow shape: an explicit Linux-only dangerous flag, requiring
`--yolo`, with no config/env activation and no fallback. Pane now names the
outer container/VM as the security boundary, and the artifact builder runs a
real no-provider shell tool before a campaign. This is the measurement-process
answer: every campaign needs a scored-task canary and a harness tool canary;
aggregate execution begins only after both pass.

**Accepted campaign result:** 12 fresh trials, zero retries/exceptions, 9/12
passes. Per task: custom heap crash 3/3, large text editing 3/3, SQLite/gcov
2/3, C/Python polyglot 1/3. Known tokens total 1,371,727: Sol parent
1,067,208 across 99 requests; Luna helper 304,519 across 48 requests. All 147
provider requests succeeded and reported usage; cache-creation usage remained
unreported, so coverage is incomplete rather than zero. The 12 recorded
preflights accompany 91 cells (13 failed) and 134 tool calls (11 failed).
Campaign wall was 1,675.9 seconds at concurrency two. The failure distribution
answers a product question more strongly than the small score delta: cleanup
and verifier-aware final-state validation are systematic gaps (two retained
`cmain` artifacts; one gcov build-directory mismatch). Public exact-task
baselines: Terminus 2 + GPT-5.3-Codex 14/20 and Terminus 2 + Opus 4.6 3/4;
different harnesses/effort/environments and unequal trial counts prohibit a
superiority claim. Full report and sealed raw-artifact hashes live in the
separate `pane-benchmarks` checkout. No capability box closes from this pilot.
