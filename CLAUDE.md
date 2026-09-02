# Claude Code project instructions

Glasshouse uses a spec-to-evidence, multi-harness development process.

## Decompression — user ruling 2026-09-03, and the process changes it makes

**The ruling** (design-decisions, *Decompression*; map **Phase 59**, lines 2043–2054):
*Glasshouse is not sloppy; it is extraordinarily conscientious — in places too
conscientious. The biggest risk is now complexity through over-assurance: files too
large, too much historical documentation, a process whose evidence system itself
needs maintaining. Before a broader release: no further large features; a hardening
and simplification phase — split modules, cut redundant explanation, run real long
sessions, close the open items by risk rather than by checkbox count. Keep what was
good; changing the process is explicitly allowed.* **Phase 59 outranks every feature
package until its lines are closed.** What stays: Phase −1, the targeted gate and the
trailing sweep, a mutation on every decision an Amber or Red package makes, the
register, visible workers in worktrees, the co-edit protocol, an independent verifier
for Red. What changes:

1. **A size ratchet runs in every gate.** `scripts/check-file-sizes.py` ends
   `blast-radius.sh` and sits in `ci-local.sh`'s lint lane: a file over 2,500
   production lines may only shrink, per `scripts/file-size-baseline.txt`. A
   decomposition package ends with `--update`; the reviewer diffs the baseline.
2. **A pure move is Green and owes no mutation.** It is verified by the full targeted
   gate (for `config` or `main.rs` that is most of the crate), a moved-lines
   accounting (`git diff --color-moved=zebra --stat`; the worker reports how many
   lines are not moves and what they are), every existing import path kept valid by
   `pub use` re-exports, and the ratchet. **Trimming comments is a separate package**,
   reviewed by reading, never mixed into a move.
3. **A doc comment states the invariant and why it holds now.** How a decision was
   reached goes to `design-decisions.md` or the measurements behind a one-line
   pointer. No new comment block over 20 lines in production code unless its first
   sentence is the invariant.
4. **A flake costs one rerun, not a ceremony.** A red target in a known load-sensitive
   family (`terminal_loss`, `session_supervision`, the pty fixtures) is re-run alone
   once by the gate and reported `flaky-pass`, which is not red and gets no attribution
   write-up; three flaky-passes in a week buy a determinism packet for that test.
   Until `GH-GATE-RERUN-ALONE` lands, do the one rerun by hand and stop there.
5. **Evidence entries and checkpoints are bounded.** An evidence entry is the
   contract, the tests by name, the mutation table (Amber/Red) and the limits — the
   worker's report is the record, linked by path. A checkpoint is under 150 lines. A
   new practice section is a rule under 20 lines; the file is closed to stories.
6. **Dogfooding is a lane.** One real session per working day — the shipped binary
   driving a real harness on a real project for at least an hour, the orchestrator
   watching memory extraction, routing, the firewall and the shell — with findings in
   `docs/process/dogfooding.md` and packets by risk.
7. **Open lines are worked by risk, not count.** The user named 1534, 1535, 1545,
   1129, 1044, 1294 and 1610 as product-relevant; their refusals are superseded by
   *design it* (map line 2054). Everything else open stays refused unless a producer
   lands.

Order of the splits: `config`, `routing/evidence`, `shell` first (no live worker
touches them), then `routing/session`, then `main.rs → commands/` once the package
holding `main.rs` integrates; a trim package follows each split; dogfooding runs
beside all of it.

## The orchestrator's reading, and why it is no longer eleven documents

**Start here, in this order:**

1. `.agent-runtime/CONTINUATION.md` — the previous session's exact checkpoint
2. `docs/process/ORIENT.md` — **generated** by `scripts/orient.py`: where the
   map stands, every phase ranked by open lines, the nearly-finished phases
   quoted in full, the practice index to read **by number**, and the recent
   checkpoints. Regenerate it with `scripts/orient.py` after any map or handoff
   change; `--check` fails if it is stale.
3. `docs/process/agent-sdlc.md` and `docs/process/worker-capabilities.md` —
   the proof process and the model-tier boundaries. Short, and both load-bearing.

**That is roughly 15,000 tokens and it is enough to start.**

**Then read on demand, never end to end:**

- `docs/product/capability-map.md` — **authoritative**, and 178 KB. `ORIENT.md`
  carries the open lines for nearly-finished phases; for any other phase use
  `scripts/discover.py --phase <id>`. Open the map itself to quote a specific
  line, not to find out what is open.
- `docs/process/orchestration-practice.md` — 176 KB. **Read sections by number.**
  `ORIENT.md` has the index with one-line summaries.
- `docs/product/evidence/phase-<id>.md` — the entry for the phase in hand.
- `docs/process/assurance-economics.md` — before writing a packet; **Phase −1 is
  a hard gate** (see below).
- `docs/process/orchestration-measurements.md`, `docs/product/design-decisions.md`,
  `docs/process/harness-hook-protocol.md`, `docs/process/orchestrator-prompt.md`
  — when the task actually reaches them.

**Why this changed.** The old "read these eleven completely" list cost about
**228,000 tokens** before any work happened; `ORIENT.md` is **4,900** and derives
from the same documents. Nothing was deleted — you now open a document because
you need it rather than to discover whether you do.

## If you are a worker, this list is not yours

The documents above are the **orchestrator's** reading. A worker that works
through them spends more context orienting than working — measured: a four-box
package used 288k tokens, over half of it on documents it did not need. **A
worker should not read `ORIENT.md` either**: it is a map of work the worker was
not asked to choose between.

**A worker reads only this**, and its packet names anything extra:

1. this file
2. its own packet
3. `docs/process/worker-capabilities.md` — what its tier may and may not decide
4. the practice sections its packet names, by number
5. `docs/product/evidence/phase-<id>.md` for the phases in its package
6. its own box lines, quoted in the packet

That is roughly 5,000 tokens instead of 175,000. `scripts/discover.py --phase
<id>` prints items 5 and 6 together.

**The orchestrator writing the packet owes the worker this scoping.** A packet
that says "read CLAUDE.md and the files it names" has handed a Sonnet the
orchestrator's job and will be paid for in context that produced nothing.

The capability map is authoritative. Work in its stated order. Do not check a
box until its evidence-ledger entry is `COMPLETE`. Only the primary Opus
orchestrator integrates, commits, and updates project-status records.

`docs/process/orchestration-practice.md` is not optional reading. It records
how to run this process without repeating mistakes that have already cost
whole cycles — task sizing for real parallelism, never losing a finished
worker, reading a failure before fixing it, and the shell traps that have bitten.
Its later sections cover running several workers at once, team leads that
subcontract, and the cheap leaf tier.

**Run workers in parallel.** Partition batches by the files they touch, order
those batches by the map, and name the other live workers' files in each
packet's `FORBIDDEN FILES`. Map order is a priority, not a mutex — one worker
at a time has already cost this project a session.

**Since the 2026-08-29 move to a 20x plan, quota is no longer the reason to stop
at three.** Dispatch four or five when the partitions are genuinely disjoint. But
the ceiling did not disappear, it changed shape: practice §9 measured the real
limit as **review collision** — reviews are serial and worker wall-clock is not —
and the orchestrator's own context is still the scarcest thing here. Past three
concurrent editing workers, use a **team lead** (§10) so review is paid out of
the lead's context rather than yours. Measured 2026-08-29: the main checkout
produced more output tokens than the next seven worker directories combined.

`docs/process/orchestration-measurements.md` is a standing inherited experiment
measuring which model tier closes capability boxes at what cost. Add every
batch to its ledger and answer one of its open questions when you can.

`docs/process/assurance-economics.md` is how verification compute is spent, and
its **Phase −1 is a hard gate you owe before every dispatch**: a packet must
demonstrate, from current production code, that each claimed input has a
producer, a caller that carries it, a propagation path, and a consumer that can
observe it. **If one link cannot be shown, do not dispatch — return the packet as
premise-invalid.** Two packets on 2026-08-28 failed this and cost ~$30 of worker
compute that no downstream optimization could recover. `scripts/validate_round.py`
enforces it, so the check is free.

**Start every packet with `scripts/new-packet.sh <name> [--recon]
[--lines N,M] [--worktree]`** rather than hand-writing one. It emits a
skeleton that already passes `validate_round.py` — the correct
`READ ONLY THIS` scoping, a `FEASIBILITY` block in the one-line form that
does not shadow itself, and box lines quoted verbatim and unwrapped from
`--lines` — so the only edit-and-revalidate cycle left is the one for the
task's actual substance.

**Every worker gets a nagging watch, armed in the same turn it is started:**
`Monitor(command: "scripts/worker-watch.sh <name> <surface> <report>", persistent: true)`.
It reminds until you run `scripts/worker-ack.sh <name>`. Before starting new
work, run `scripts/worker-ack.sh --list` and clear anything waiting.

**That watch is yours, not the worker's, and it is not a continuity watch.** It
reads the worker's *pane* from your session and tells *you* the pane went
quiet — which is exactly what a worker that died of context looks like, and it
cannot tell the worker anything. So every long-running session, this one
included, also arms its own:

    Monitor(command: "<repo>/scripts/continuity-watch.sh --role orchestrator",
            persistent: true)

`--role worker` is the other half, and `scripts/dev/new-worker.sh` now puts
that in the launch prompt itself, so a worker arms it before it reads anything.
The script finds its own session by branch and refuses out loud rather than
watching the wrong one. **Arm yours in your first turn** — and pass an absolute
path: `.agent-runtime/` exists only in the main checkout, so the relative form
that used to be documented here failed with exit 127 in 63 of 64 worktrees
while the pane looked armed.

Measured 2026-08-29: three Opus workers, two hours in, no watch between them,
and the user noticed before any mechanism did.
`scripts/tests/test_launch_prompts.py` fails the gate if a launch prompt loses
the instruction again — the rule is enforced now rather than written down.

**Keep the pipeline fed, and let `scripts/pipeline.sh` remember it for you.**
Every other watch in this project fires on a worker *event*. An empty board
produces no events, so it is quiet in exactly the way that looks like nothing is
wrong — and on 2026-08-29 an orchestrator sat at one worker with ~90% of the
tree unclaimed until the user asked why. Arm this in your first turn alongside
the continuity watch:

    Monitor(command: "scripts/pipeline.sh --watch 600", persistent: true)

It stays silent while two or more workers are live and names the undispatched
packets when they are not. **The floor is two, not one**: by the time the board
is empty the refill has already cost wall-clock that parallel work would have
absorbed. The ceiling is still §74's — past three concurrent editing workers use
a team lead, because review is what catches a mutation killed by the wrong
assertion, and review is yours.

**Before choosing what to dispatch, run `scripts/cluster-b.py` and then read
`docs/process/refusal-register.md` — in that order, and read the register
before you commit to anything.** The script finds the shape that closed four of
batch 51's eight lines: production code whose every call site falls after its
file's `#[cfg(test)]`. The register is what stops you packaging a phase that
looks open and is not — six of Phase 32A's nine open lines are Cluster E, *"the
provider signal genuinely does not arrive, do not package"*, and an orchestrator
recommended that phase anyway by counting open lines instead of reading the
register first.

**Run practice §16's mutation ritual with `scripts/mutate.sh`, not by hand.**
It backs up, mutates, touches, runs the given test, and always restores from
the backup — failing loudly if the restore does not come back byte-identical.
A SURVIVED result is the valuable one: it names behaviour no test in the
command actually watches.

**Integrate with `scripts/integrate.sh <name>...`, and read what it prints.**
It takes bare worker **names** (`api-routing`), not paths — it builds
`.worktrees/<name>` itself, and a path argument fails with `MISSING`. It applies
each worker's diff, copies the untracked deliverables `git diff` cannot see (a
tests-only worker has *no* tracked changes — three of batch 45's six were
invisible), runs fmt, and runs the blast radius. It refuses a dirty tree, a
non-ancestor base, and any file two worktrees both touched.

**Pass every finished worktree in one call. Do not integrate serially.** The
interactions between patches only appear once the diffs share a tree, so serial
integration hides exactly what integration is for — batch 47's non-`Default`
field on `SessionRecord` broke five struct literals inside *another* worker's
files. Attribution does not suffer: the tool refuses any file two worktrees both
touched, so the patches stay file-disjoint and the blast radius names the target.

**It deliberately stops there.** It never commits, ticks a box, writes evidence,
or runs a mutation. The mechanics caught nothing on their own in batch 45 —
every real catch came from reading a diff or choosing a mutation, and the
classify-caller refusal was noticed *while applying the patch*. Automating the
mechanics is a win; delegating them to an agent would remove the exposure that
produces the rulings. **The ruling, every packet's Phase −1, and the diff of
anything that decides something stay with you.** Not *every* diff: ten wrongly
ticked boxes have been corrected so far and **all ten were found by audit
workers, after the orchestrator had read the diff and ticked it** — every one of
them the "no production caller" shape `cluster-b.py` finds mechanically. Run the
script instead, and spend the reading on decisions (§86).

**Before the gate, run `scripts/blast-radius.sh`.** It maps the files you changed
to the cargo test targets that could break, and runs them. Practice §79 exists
because a worker ran §69's grep, the grep correctly named the affected file, and
the worker then *read* that file and judged it unaffected — costing a full gate
cycle for something one eight-second test run catches. Once a grep names a file,
run its tests; do not read them and decide.

**The full sweep is TRAILING, not blocking — user ruling 2026-09-01** (*"most
of our SDLC has become waiting … do this the smarter way"*). The blocking gate
before an integration commit is the TARGETED one: the changed files' own
targets plus the worker's quoted tests, re-run on the merged tree
(`blast-radius.sh --targeted` once GH-GATE-ECONOMICS lands; until then, run
the equivalent target list by hand). Commit and push on targeted green. The
FULL two-lane sweep runs in the background per WAVE — once per two-to-four
integrations, not once each — and a trailing red spawns a fix-forward worker
(§84) while the line keeps moving; 2026-09-01's own history is the model: a
missed regression shipped, the next tree's sweep caught it, the fix landed
two commits later with zero damage. What does NOT weaken: per-box targeted
tests and mutations stay blocking for every tick, `--targeted` prints how
many full-trace targets it skipped so nobody mistakes it for the sweep, and
the trailing sweep's failures are enumerated PER TARGET before attribution
(the truncated-grep rule in the batch-70 measurements entry). Batch waves
into one integrate call where finishes align; co-editors stay serial but now
pay only the targeted price.

**Dispatch with `scripts/dev/new-worker.sh <name> <cwd> <packet>`.** It creates
the pane, launches the harness, types the prompt in, and **proves the prompt
landed** before returning. Passing a prompt as a command-line argument silently
does not work here, and `cmux identify --workspace X` reports the *app's*
focused surface rather than that workspace's — both cost real time on
2026-08-29, one of them by typing a launch command into the user's own pane.

**Turn a worker's report into a ledger draft with
`scripts/evidence_from_report.py`.** Workers emit a ```glasshouse-facts``` block;
the script renders the mechanical part of the evidence entry. It decides nothing
— it emits `⟨RULING REQUIRED⟩` and lists what you still owe. **No script may put
a box in a state that would authorise ticking it.**

`scripts/dev/` holds the dev shims, symlinked onto `PATH`: `glasshouse` runs
the binary this repo builds, and `agy-gh` starts an Antigravity leaf worker
unattended. Use them instead of re-deriving the workaround or asking the user
to intervene — practice §19 explains why they are not the product's shims.

Keep Claude Code, OpenCode/Ox, and other native harness workers visible in cmux.
**Every editing worker gets an isolated worktree, and it goes in `.worktrees/<name>`
inside this repository** — gitignored, excluded from the gate's container copy, and
removed by `scripts/close-worker.sh`. Do not create sibling directories next to the
checkout; sixty-one of those accumulated before anyone noticed. Practice §73 has the
reasoning and the one trap. Start Ox with the normal `ox` TUI—never
`ox run` or a headless loop. Follow the worker do/don't rules and the safe hook
protocol rather than personal global routing configuration.

## Scale the ceremony to the task, and prove the board is closing boxes

Output tokens per net closed box, by day: **57k → 109k → 126k → 811k**
(2026-08-27..30, §86). The last step is 6.4× in a day, and box difficulty does
not step 6.4× overnight. Two rules follow, and both are meant to be applied from
memory in seconds.

**1. Every packet is Green, Amber or Red.** Decide it from the packet you just
wrote, not from the codebase — it takes under a minute. This is the scale already
in `worker-capabilities.md`; do not invent a second one.

| tier | model | effort | entry criterion | it owes | it **skips** |
|---|---|---|---|---|---|
| **Green** | Sonnet | low–medium | adds no new decision — wires an existing value to an existing consumer, a `Display`/serde impl, a flag forwarding to a settled function, tests only, docs or config literals, **a pure move (Phase 59 decomposition)** | the box's own targeted test, plus one assertion that the production caller runs it | mutation, independent reviewer, orchestrator diff read, and any blast radius beyond the named target |
| **Amber** | Sonnet | medium–high | adds or changes a decision — a branch, threshold, ordering, ranking, persisted field, or public API shape | targeted tests, blast radius, **one** mutation on the decision the box names | the independent reviewer, unless the worker flags a decisive claim; and you read the diff of the decision, not of the whole package |
| **Red** | Opus specialist | high–xhigh | PTY/process lifecycle, signals, shutdown, migrations, session identity or resume, project isolation, secrets, `#[cfg(...)]` platform code — or a disputed architecture | full relevant regression, platform legs, the semantic mutation suite, an independent verifier | nothing |

**The tier picks the model and the effort with it — one decision, not three.**
`worker-capabilities.md`'s risk routing and `dev/new-worker.sh --model` are this
same scale; never choose them separately. **One harness, one flag: Green and
Amber are both Sonnet and differ only in effort.** **The default is not Opus.**
This session dispatched 6 Sonnet and 8 Opus — but the **last five in a row were
Opus**, and at least two were over-tiered: writing prose from facts the
orchestrator already gathered is Amber at most. **xhigh on a mechanical task is
waste** — effort buys deliberation over a decision, and a Green packet contains
none by definition.

**Phase −1 is never skipped, at any tier.** It is the cheapest check here and
guards the only spend the ledger calls pure waste. *Uncertain tier escalates* —
but "this feels small" is not uncertainty, it is a Green.

**2. Three rules that keep the board closing boxes.** Each fires without
interpretation:

- **A validated, undispatched implementation packet outranks any new
  investigation.** `validate_round.py` passing is the whole trigger. Declining
  it for a contended file is not free: **a co-edit is the normal case** —
  `coedit.sh claim` — and declining one needs a specific reason written into the
  checkpoint, not a judgement made silently.
- **An investigation package names, in its own packet, the implementation
  package it unblocks.** No named successor, no dispatch. A refusal-register row
  is not a deliverable; the package it routes to is (§83).
- **Above 250k output tokens per net closed box over the last two days, the
  next dispatch must be implementation** — and no investigation goes out until
  it is back under. Two commands: `python3 scripts/usage-snapshot.py
  --glasshouse`, and `git show <rev>:docs/product/capability-map.md | grep -c
  '^☑'` at each end of the window. 250k is 2× the worst healthy day measured.

## Size the package by the mechanism, and the six token traps

Long form and every number: **practice §87**. Batch 55 managed **0.77 boxes per
package** across thirteen packages; its outlier closed **five boxes in 66 lines**
because those five map lines were facets of one `store.context(&id)` call.

**Size a package by the mechanism, not by the line — target 3–6 boxes.** One
producer, one call site or one reader serving several map lines is *one*
package. A 1-box package is right only when the phase has one **reachable** line
(Phase 31: one of seven, the rest Cluster Q). Three shapes find the fat
mechanisms: a phase whose first line is the mechanism and whose rest are its
filters (34C — 1431 selects, 1432–1443 are its rules); several lines that are
fields of one returned value (Phase 30's 1161–1165, one `SessionContext`); a
recon grouped **by root cause** — Phase 51's 34 lines were 4 causes, and the
causes are the package boundaries.

**The traps, each one structural and invisible at the moment it is chosen.**

| # | trap | rule |
|---|---|---|
| 1 | investigation ending in a document instead of a dispatch — register 280→969 on the day the map moved +9 | §86: name the successor package in the packet |
| 2 | small packages multiply a fixed integration cost — the blast radius ran **41–56 test targets** per run, priced by files touched, not boxes closed | batch disjoint partitions into ONE `integrate.sh` call; this is what makes 3–6 above worth anything |
| 3 | a co-edit buys dispatch parallelism and sells integration parallelism — `integrate.sh` refuses a shared file, so co-editors integrate **one at a time**, each paying trap 2 in full | partition by mechanism to stay file-disjoint; reserve a co-edit for a large package that must share `main.rs` |
| 4 | reading for a property a script decides — all ten un-ticked boxes were `cluster-b.py`'s shape, every one found *after* the orchestrator read the diff | run the script; spend the reading on **decisions**, and on Phase −1 |
| 5 | polling a running job — `sleep`-and-check pays a tool round to learn "still running" | arm a `Monitor` whose filter names the **failure** signatures too, or background it and wait; a bounded one-shot check is the only exception |
| 6 | skipping the batch's ledger row — batches 46–55 are unlogged, exactly the span where out-per-box went 57k→811k | one row per batch; it is the only instrument that sees a regime change while it happens |

Trap 3 does not contradict *"do not integrate serially"*: that rule is about
**disjoint** partitions, where serial integration hides the cross-patch
interaction it exists to find. Co-editors of one file are the case
`integrate.sh` refuses to batch, and one at a time is correct there.

## Trust the report's artifacts, and verify where the act is irreversible

Long form: **practice §88**. **Not one worker has misreported.** All ten wrongly
ticked boxes were *accurate* reports about correct code nobody had asked the right
question about (`bd81e04`), and all ten were found *after* the orchestrator read
the diff — while `integrate.sh` re-runs every quoted test on the merged tree.

**Act on a report carrying all five artifacts**, reading it for the decision it
hands you rather than to re-derive its facts: `validate_round.py` passed before
dispatch (so Phase −1 is established) · a well-formed ```glasshouse-facts``` block
· mutations **KILLED, killing test named and failure text quoted** · gates quoting
real `test result:` lines with counts, not "tests pass" (§68) · `blast-radius.sh`
exit 0. A missing artifact is a question for the worker, not a re-derivation.

**Verify anyway in these five cases and no others**, each tied to irreversibility
or to a signal the report itself raised — never to suspicion:

- **an authority-carrying act**: ticking a box, un-ticking one, or ruling;
- **Phase −1, before every dispatch, at every tier**;
- **the report names its own thin spot** — spend it there, not on the whole report;
- **two sources disagree**, or `packet_errors` contradicts the packet;
- **a red result** — two runs to attribute it before naming a cause (§34).

**Never re-derive** test results the blast radius re-runs, §81 line numbers, a
mutation reported with its killing test and output, or a script's verdict (trap 4).

**A worker caught misreporting loses trust for that class of claim for the rest of
the batch, and the checkpoint says so. That has never fired** — untested, not
proven. No scoring and no per-worker ledger: bookkeeping is dropped first.

## An orchestrator does not idle, and does not hand off cold

**`scripts/pipeline.sh --watch 600` is not advisory. When it fires, dispatch —
do not reply to it with a reason.** On 2026-08-29 it fired twice and the
orchestrator answered both times with a well-argued explanation of why waiting
was reasonable. Both explanations were wrong, and the user had to say so twice.
There is always work: `cluster-b.py` finds candidates in seconds, the refusal
register says which are packageable, and `new-packet.sh` emits a valid packet.

**Reviewing, integrating, ruling and committing are not "being busy".** They are
what you do *between* dispatches, not instead of them. Two or three workers
should be running while you do them.

**Hand off HOT.** When the continuity watch fires, the instinct is to finish
cleanly and leave a tidy empty board. That is backwards: the successor is a
fresh context that can review anything, and an idle board wastes the whole
window it takes them to spin up. **Fill the board first, then write the
checkpoint, then relaunch.** The successor inherits running workers and reviews
their reports as its first act — which is the cheapest possible start.

**Do not stop for a gate, an integration, or a barrier.** A red gate gets a fix
worker and the line keeps moving (§84). A co-edit barrier blocks one *file*, not
the board (§77). An integration blocks nothing.

**The only reasons to leave the board empty** are the user asking you to stop, or
a defect so central that every candidate package would build on it. Neither has
happened yet.

## Every script, because naming only some of them cost a session

**CLAUDE.md named fifteen of these and the repo has twenty-seven**, which cost a
round queued behind `main.rs` with `scripts/coedit.sh` sitting unread and §77 in
an index already loaded. The fix is a list, not more prose.

**Round mechanics:** `new-packet.sh` · `validate_round.py` · `dev/new-worker.sh`
· `worker-watch.sh` · `worker-ack.sh` · `worker-done.sh` · `close-worker.sh` ·
`integrate.sh` · `evidence_from_report.py`

**Deciding what to work on:** `discover.py` · `orient.py` · `cluster-b.py`
(finds production code with no production caller — the shape behind four of
batch 51's eight closures) · `pipeline.sh` (nags when the board runs dry) ·
`map-index.py` · `progress.py`

**Verification:** `ci-local.sh` (`--macos --linux --windows-vm`; **any flag
suppresses the macOS+Linux default**) · `blast-radius.sh` · `mutate.sh` ·
`msrv-check.sh` · `check-doc-boundary.sh` · `check-evidence-coverage.py`

**GitHub CI is manual-only — the LOCAL gate is the gate.** User instruction of
record, 2026-08-31/09-01: *"this projects CI is way too demanding to be ran on
github fully … only run in the github CI in the future when absolutely
necessary"*, then *"if ci is good on this machine then just skip it on github."*
`.github/workflows/ci.yml` therefore triggers on **`workflow_dispatch` only** —
a push fires **nothing** and costs nothing, on any branch. The jobs are intact
for the rare cross-platform contract that genuinely cannot be settled locally:
run them from the Actions tab, and say in the triggering commit why the run was
needed. The cost that motivated this stands if anyone re-adds a push trigger:
seven jobs (`test`+`msrv` × 3 OS, plus `lint`), macOS billed **10×**, a fresh
monthly allowance gone in about ten pushes. `ci-local.sh` covers macOS and
Linux and drives a real **Windows VM**, and this machine *is* the macOS
coverage. Pushing to main is now cost-free and needs no CI justification.

**Sharing a contended file — read §77 before queueing on `main.rs`:**
`coedit.sh claim|peers|diff|done|status|ready|list|release`. Contention on
`main.rs` is **structural**, not bad luck: §32 says put the caller's file in the
partition and that is where every production caller lives. Batch 45 deferred six
of seven packets on it.

**Continuity and housekeeping:** `continuity-watch.sh` (`--role
worker|orchestrator`) · `orchestrator-heartbeat.sh` · `usage-snapshot.py` ·
`reap-worktrees.sh` · `ask-user.sh` · `stale-workspaces.sh` (`--watch 900` —
arm it in the first turn; it names every provably redundant cmux pane and
nags until each is closed, because two sessions left fifteen behind)

**Hooks (`scripts/hooks/`), which enforce rather than remind:**
`guard-worktree-boundary.sh` · `guard-destructive-git.sh` ·
`coedit-peer-notice.sh` · `coedit-unreleased-guard.sh` · `worker-turn-ended.sh`

Current phase and next action belong in `docs/process/handoff.md`; do not encode
phase-specific assumptions in this file.
