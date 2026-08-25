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
