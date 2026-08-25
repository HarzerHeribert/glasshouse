# Glasshouse orchestration practice

How to *run* the process the SDLC describes. `GLASSHOUSE_AGENT_SDLC.md` says
what the steps are; this says how to execute them without losing time to the
same mistakes twice.

Everything here was paid for. Each rule names the failure that bought it.

---

## 1. Parallelism, and sizing tasks so it actually helps

**The problem to avoid: workers that come back in five minutes.** On
2026-08-25 several packets were sized so small that workers finished almost
immediately. That looks efficient and is not: every return costs the
orchestrator a full review cycle, and three short workers finishing together
means three reviews colliding while the orchestrator is mid-thought on
something else. Short tasks do not parallelise — they *interrupt*.

**Size a packet for 20-40 minutes of worker time.** That is roughly:

- one new module with its tests, plus the wiring that gives it a production
  caller; or
- one coherent vertical slice across 4-7 files; or
- 400-900 lines including tests.

If a packet looks like 150 lines, it is either leaf work (§6) or it should be
merged with the next packet.

**Run two or three workers, started apart.** Stagger their starts by ten
minutes or so, so their returns stagger too. The orchestrator needs breathing
room *between* reviews, not a queue of three.

**Never review two workers at once.** Finish one — diff, gates, mutations,
integrate, commit — before opening the next. A half-reviewed batch is worse
than an unstarted one.

**Keep the orchestrator's own hands free.** If you are implementing while
three workers run, you will do all four things badly. The orchestrator's job
during a worker's run is: probe real binaries, settle design, write the next
packet. Not code.

---

## 2. Never lose a worker

**The failure:** a worker was started with no watch and finished unnoticed.
The user noticed before the orchestrator did. Separately, several idle
notifications arrived mid-thought, were read, and were not acted on.

**A single notification is not enough.** It competes with whatever you were
doing and loses.

So every worker gets a nagging watch, armed **in the same turn you start it**:

```
Monitor(command: "scripts/worker-watch.sh <name> <surface-ref> <abs-report-path>",
        persistent: true)
```

It reminds every three minutes, forever, until you physically tick it off:

```
scripts/worker-ack.sh <name>      # after you have actually dealt with it
scripts/worker-ack.sh --list      # what is still waiting
```

**Acknowledge only after the work is integrated or explicitly parked.**
Ticking it off to stop the reminder is precisely the failure the reminder
exists to prevent.

**Before starting any new work, run `scripts/worker-ack.sh --list`.** If
something is waiting, deal with it first.

---

## 3. Read before you act

Three rules, each of which cost a full cycle.

**Read the failure log before forming the fix.** A Windows CI failure was
diagnosed twice from reasoning and fixed twice wrongly — three CI round-trips
— before anyone read the assertion message, which named the cause in one line.
A simulation that confirms your hypothesis is self-consistency, not evidence.

**Read the worker's surface before sending to it.** `cmux read-screen
--surface <ref>` first, every time. A harness may be on a trust prompt, a
login menu, or not started at all. One worker received its prompt into a
trust dialog and did nothing.

**Capture the surface ref from `cmux send`'s own output.** An empty
`--surface ""` silently resolves to *your own pane* — which once nearly sent a
stray Enter into the orchestrator's own session.

---

## 4. Shell traps that have actually bitten

- **`cd` persists within a single Bash call.** A patch meant for `main` was
  applied inside a worker's worktree because the same call had `cd`-ed there
  earlier. Apply patches from a call that never `cd`s.
- **`grep -c` exits 1 when the count is 0.** Chaining gates with `&&` made a
  clean clippy run look like a failure and silently skipped the rest.
  **Do not chain verification commands with `&&`.** Run them as separate
  statements and print each result.
- **Run the test suite with `< /dev/null`.** `glasshouse hook` drains stdin to
  EOF by design; from a shell whose stdin is an open pipe, two hook tests hang
  forever in `wait4`.
- **Foreground `sleep` in a compound command may be blocked.** Use a Monitor
  with an until-loop, or a background command.

---

## 5. Evidence discipline the SDLC assumes but does not spell out

**A mechanism with no production caller does not get its box.** Applied to
`SessionRuntime`, to Phase 1 line 90, to Phase 9A's environment injection, and
to Phase 9's identifier reader. If the slice does not reach a caller, either
extend the slice or leave the box.

**Check a declaration against the *use*, not the claim.** Claude Code's
`auto-mode` was a true statement about the product and useless for launching
it — it is a subcommand, and the session flag is `--permission-mode auto`.
Four separate declarations in this project have been derived from artifacts
that did not serve the purpose they were cited for. Before a declaration is
consumed, check that its evidence supports the consumption.

**Do not check a box your own packet claimed, if the code does not support
it.** A packet asserted NVIDIA and LiteLLM templates and header overrides were
in scope; none existed. The packet was wrong; the boxes stayed unchecked.

**A `SURVIVED` mutation is more often a weak mutation than a weak test.** One
"leak" mutation read a credential into an unused local without printing it —
nothing leaked, and the test was right to pass. Rewrite the mutation before
doubting the test.

**Read the named test's own result line, in the target that runs it.** A bin
target's kill is invisible in the lib target's result line, which will happily
report `0 failed`.

**Run the binary.** Two rendering defects compiled, passed clippy, and passed
a full suite: descriptions containing backticks rendered doubled inside the
backticks the report adds. Only running `glasshouse doctor` showed it.

---

## 6. Model tiers, including the fast one

`GLASSHOUSE_WORKER_CAPABILITIES.md` defines the tiers. Two practical notes:

**Red-risk work goes to an Opus specialist, not to Sonnet.** Secret
boundaries, PTY lifecycle, migrations, resume identity. The secret-storage
batch went to a specialist and its *refusals* — declining a
`SecretRef::Literal { value }` variant, declining a memoising cache, declining
`assert_eq!` on `expose()` because it prints both sides on failure — were the
most valuable part of the output.

**The leaf tier can be a fast cheap model, and it works.** Gemini (via `agy`)
was given a bounded inventory: scan the map, quote every matching unchecked
line, group by phase. It returned 171 quotes, **all 171 verbatim, none
already-checked**. Verify leaf output mechanically — diff its quotes against
the source — and it is excellent value for searches, inventories, call-site
lists and checklist reviews.

Antigravity declares **no automatic-review mode**, and its "always allow"
matches on the exact command prefix, so it re-prompts for every new script.
Running a leaf worker there needs `--dangerously-skip-permissions`, which the
user has explicitly accepted for this use. That is the same situation Phase
9A's acknowledged-bypass line exists to govern.

---

## 7. Workers are right against their packets more often than you expect

Four workers in one session each corrected their packet on at least one point,
and **every one of them was correct**:

- an acceptance test that would have asserted `None != None`;
- a claim that a capability's evidence string did not actually support;
- a note that no shipped profile can populate the thing being tested;
- a refusal to implement a `NativeSessionSource` that would have opened every
  one of the user's private conversation databases on every session end.

That last one is why packets must carry an explicit stop condition inviting
the worker to report rather than choose. **Read those flags carefully and
check them; do not skim past them because the gates are green.**

---

## 8. Handoff

Running low on context is a handoff, not a stop. Finish or checkpoint the
batch in hand, rewrite Part 2 of `.agent-runtime/CONTINUATION.md`, commit,
then run `.agent-runtime/self-continue.sh context`.

Before handing off:

- `scripts/worker-ack.sh --list` must be empty, or the checkpoint must say
  exactly what is waiting and where its worktree is;
- every live worker's worktree and branch must be named in the checkpoint,
  because **workers never commit** — the worktree *is* the deliverable;
- `python3 scripts/progress.py` must have been run if the map changed, or CI's
  lint job fails.

---

## 9. Parallelism at scale — partition by file, order by map

**The failure this rule buys back:** on 2026-08-25 the orchestrator ran one
worker at a time for most of a session, believing the work could not be
partitioned. The map disproves it — 1,266 unchecked lines across 99 phases,
with whole blocks in modules nothing else touches.

The conflicts were real, but only *inside* the Phase 9 family, because work was
being taken in strict map order within one family. **Map order is a priority,
not a mutex.**

So schedule like this:

1. Group open lines by **the source files they would touch**, not by phase.
2. Within a group, take them in map order.
3. Run one worker per group, concurrently.
4. **A packet's `FORBIDDEN FILES` section is the scheduling primitive.** Name
   the other live workers' files in it explicitly — "another worker is editing
   this right now" — and add a stop condition telling the worker to report
   rather than edit. That is what makes concurrency safe.

Three editing workers is the point where reviews start to collide, because
reviews are serial and worker wall-clock is not. Beyond that, use a team lead.

Measured numbers live in `GLASSHOUSE_ORCHESTRATION_MEASUREMENTS.md`. Add yours.

## 10. Team leads — push the review cycle down a level

An Opus worker may run its own subcontractors. This is how concurrency grows
past the orchestrator's own attention: **a lead's review cost is paid out of
the lead's context, not yours.**

Give a lead a packet that decomposes, and say in the packet:

- **what it must keep** — every red-risk part, the design, and the mutations;
- **what is good to hand out** — test batches once the API is settled,
  mechanical wiring, dependency plumbing, scans and inventories;
- and the three rules that are not negotiable:
  1. **verify every subcontractor's gates yourself** — a worker on this project
     once reported gates green while its tests did not compile;
  2. **never let two subcontractors edit the same file at once** — give each an
     explicit file list;
  3. **the lead owns the mutations** — a subcontractor may write a test, but
     only the lead decides it is non-vacuous and runs the mutation that proves
     it, reading the named test's own result line in the target that runs it.

Ask the lead to report what it delegated and what it kept, so the value of the
arrangement can be measured rather than assumed.

## 11. The cheap tier: Gemini Flash via `agy`, and how to run it

The leaf tier is measured, not assumed — it scored 171/171 verbatim quotes on a
bounded map inventory. Use it for inventories, call-site searches, focused
reruns, checklist reviews, settled documentation, and **record audits**, which
it is unusually good at because they are pure counting.

Running it, with the traps in order:

- Start it as `agy --mode accept-edits`. Without `accept-edits` it cannot write
  its own report. Antigravity declares **no automatic-review mode**, and its
  "always allow" matches on the exact command prefix, so it re-prompts for
  every new command; a leaf doing more than reading may additionally need
  `--dangerously-skip-permissions`, which the user has accepted for this use.
  **Claude Code's own auto-mode classifier may refuse to type that flag into a
  pane** — if so, ask the user to approve it rather than working around it.
- **Give it its own worktree, never another worker's and never `main`.** It
  runs in accept-edits mode; a folder it is trusted in is a folder it can
  write. `git worktree add --detach <path> main` costs nothing and contains it.
- It asks **"Yes, I trust this folder"** on first start in a new directory.
  Confirm only after checking which directory the pane is actually in — the
  pane inherits the workspace's cwd, which is usually the *previous* worker's
  worktree.
- **Verify its output mechanically.** Diff its quotes against the source; do
  not read its summary and nod. Its value is that it is checkable.
- Watch it like any other worker, with a shorter nag: leaf tasks finish fast,
  and `scripts/worker-watch.sh <name> <surface> <report> 120` is right.

## 12. Keep the experiment running

`GLASSHOUSE_ORCHESTRATION_MEASUREMENTS.md` is a standing, inherited experiment,
not a one-off note. Add every batch to its ledger with its verdict, answer one
of its open questions when you can, and record what changed your mind. The
project is a control plane for routing work to models; the data this process
generates about *which tier produced what verified result* is the same question
the product exists to answer.


## 13. Two traps this project hit while running several workers at once

**`git add -A` in the main worktree sweeps up whatever a tool left there.**
On 2026-08-26 a stray `AGENTS.md` — a retitled copy of `CLAUDE.md` that no
worker admitted to creating — was committed that way, in a commit that was
supposed to touch one documentation file. `git status` had been checked *before*
the edits and not again before the commit. **Print `git status --short` in the
same call that commits**, or stage explicit paths. The SDLC already says to
reject generated noise; this is how it gets in.

**Mutation proofs are not delegable while the team lead is also mutating.**
A lead's subpacket invited a subcontractor to run mutations on `src/` files the
lead was mutating at that moment; both sets of results would have been garbage.
The lead caught it and cancelled that section before anything started. Put it in
the packet: *the lead owns every mutation*, and a subcontractor works on files
the lead is not touching — or from a git ref, never the live working tree.

**A branch cut before a sibling batch landed will not apply cleanly.** Use
`git apply -3` rather than forcing, and expect to merge by hand on any file two
batches share. Naming the other live workers' files in `FORBIDDEN FILES` reduces
this but does not eliminate it, because a batch that landed *between* the branch
point and the merge is not a live worker any more.


## 14. A source-scanning test is a line-ending trap

`include_str!` reads a file exactly as it was checked out. On a runner where Git
converts line endings, the source your test scans contains `\r\n`, so any search
for a literal `"\n}\n"` — or any other multi-line literal — silently finds
nothing. On 2026-08-26 that took Windows CI red on a guard that had nothing to do
with platforms: it proved a code path never opens the user's conversation
databases, and it failed by *panicking* rather than by asserting.

Two rules, and the second is the one that matters:

1. **Scan by `str::lines`**, which strips the carriage return for you. A
   column-zero `}` is `line.trim_end() == "}"`. CRLF-agnostic by construction
   rather than by remembering.
2. **Test the scan against a CRLF copy of its own source.** An LF checkout never
   exercises the broken path, so without this the fix is untested precisely where
   it was needed. `SOURCE.replace('\n', "\r\n")` and assert both scans agree —
   restoring the old literal search must fail *locally*.

This project now has **six** source-scanning tests. Every one of them is
exposed to this.
