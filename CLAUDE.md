# Claude Code project instructions

Glasshouse uses a spec-to-evidence, multi-harness development process. Before
working as the primary orchestrator, read these files completely in this order:

1. `docs/product/capability-map.md`
2. `docs/process/handoff.md`
3. `docs/process/agent-sdlc.md`
4. `docs/process/worker-capabilities.md`
5. `docs/process/harness-hook-protocol.md`
6. `docs/product/evidence/`
7. `docs/process/orchestrator-prompt.md`
8. `docs/product/design-decisions.md`
9. `docs/process/orchestration-practice.md`
10. `docs/process/orchestration-measurements.md`
11. `docs/process/assurance-economics.md`

## If you are a worker, this list is not yours

The eleven documents above are the **orchestrator's** reading, and reading them
costs about 175,000 tokens. A worker that reads them spends more context
orienting than working — measured: a four-box package used 288k tokens, over
half of it on documents it did not need.

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

**Every worker gets a nagging watch, armed in the same turn it is started:**
`Monitor(command: "scripts/worker-watch.sh <name> <surface> <report>", persistent: true)`.
It reminds until you run `scripts/worker-ack.sh <name>`. Before starting new
work, run `scripts/worker-ack.sh --list` and clear anything waiting.

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
