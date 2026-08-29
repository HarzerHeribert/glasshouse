# Claude Code project instructions

Glasshouse uses a spec-to-evidence, multi-harness development process.

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

**Why this changed.** The old list said "read these eleven completely" and cost
about **228,000 tokens**, paid before any work happened. Every orchestrator so
far quietly improvised around it — reading the checkpoint, then grepping — which
meant this file enforced its own scoping rule on workers and exempted the role
that spends the most context. Measured 2026-08-29: `ORIENT.md` is **4,900
tokens**, a 46× cut, and it is derived from the same documents. Nothing was
deleted; the difference is that you now open a document because you need it
rather than to discover whether you do.

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
interactions between patches are the part no worker can see, and they only
appear once the diffs share a tree — so serial integration hides exactly what
integration is for. Batch 47 measured both halves of this: adding a non-`Default`
field to `SessionRecord` broke five struct literals inside *another* worker's
files, and a schema bump broke eight migration-ladder tests in files no packet
named. Neither worker could have found either alone; one combined
`integrate.sh` run surfaced both. Attribution does not suffer, because the tool
already refuses any file two worktrees both touched, so the patches are
file-disjoint and the blast radius names the failing target.

**It deliberately stops there.** It never commits, ticks a box, writes evidence,
or runs a mutation. The mechanics caught nothing on their own in batch 45 —
every real catch came from reading a diff or choosing a mutation, and the
classify-caller refusal was noticed *while applying the patch*. Automating the
mechanics is a win; delegating them to an agent would remove the exposure that
produces the rulings. **Reading every diff, every mutation target, every box
decision and every commit stays with the orchestrator.**

**Before the gate, run `scripts/blast-radius.sh`.** It maps the files you changed
to the cargo test targets that could break, and runs them. Practice §79 exists
because a worker ran §69's grep, the grep correctly named the affected file, and
the worker then *read* that file and judged it unaffected — costing a full gate
cycle for something one eight-second test run catches. Once a grep names a file,
run its tests; do not read them and decide.

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

Current phase and next action belong in `docs/process/handoff.md`; do not encode
phase-specific assumptions in this file.
