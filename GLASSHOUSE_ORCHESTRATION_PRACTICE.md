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

- Start it as **`agy-gh`**, the dev shim in §19, and pass `--mode accept-edits`.
  Without `accept-edits` it cannot write its own report. Antigravity declares
  **no automatic-review mode**, and its "always allow" matches on the exact
  command prefix, so it re-prompts for every new command; a leaf doing more
  than reading needs the blanket bypass too. **Do not type that flag into a
  pane.** Claude Code's own auto-mode classifier refuses it, and asking the
  user every time is a round trip per worker. The shim carries the flag so no
  orchestrator has to — that is the whole reason it exists.
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

**How exposed is the rest of the codebase? Checked, not guessed.** Eight files
use `include_str!` to scan their own source. After this fix, **none** of them
searches a multi-line literal, none uses `split('\n')`, and every other scan
already goes through `str::lines`. So the exposure was exactly one site — the
one that went red — and the rest were already safe by habit.

That is worth stating precisely rather than alarmingly, because the first
version of this section claimed six tests were exposed. That number came from a
worker's count of a narrower idiom, was repeated without checking, and was
wrong. *Check a declaration against the use* applies to the practice file too.

The rule still stands for the next scan someone writes: `lines`, and a CRLF
copy in the test.


## 15. Reproduce a Windows line-ending failure locally, in four commands

Two CI round-trips went into one CRLF bug on 2026-08-26 — the second because
the *guard* written for the first depended on the checkout it was guarding
against. There is no need for a third, ever: the failure reproduces on macOS.

```sh
cp crates/…/file.rs /tmp/file.orig                       # back up ONE file
python3 -c "import pathlib; p=pathlib.Path('crates/…/file.rs'); \
  p.write_bytes(p.read_bytes().replace(b'\r\n',b'\n').replace(b'\n',b'\r\n'))"
cargo test --lib --all-features <the test> < /dev/null    # must still pass
cp /tmp/file.orig crates/…/file.rs                        # restore, then `diff -q`
```

Used exactly this way it did two jobs: it proved the fix holds under real CRLF,
and — with the pre-fix guard restored under the same CRLF — it reproduced CI's
failure with the **identical assertion message**, which is what makes the
reproduction trustworthy rather than merely green.

**The deeper rule, which is not about line endings.** The first guard built its
CRLF copy from `SOURCE` directly, so its input varied with how the file happened
to be checked out. An assertion whose input depends on the environment is a
flake generator, and it will find the environment you did not test on. Build
both sides from a normalised base. A subcontractor taught this project the same
lesson one batch earlier with a test that scanned a randomly generated token and
failed 45 times in 100.


## 16. A mutation runner must force a rebuild, and workers must not share `target/`

The worst failure mode in this whole process is a mutation result that is not
about the code. On 2026-08-26 a subcontractor pointed `CARGO_TARGET_DIR` at the
repository's shared `target/` and cargo served it a **cached test binary built
from mutated source** — the fingerprint matched, so a mutation no longer present
in the source was still in the binary under test. It caught this itself and
moved to a private target dir.

The trap has a second mouth, which the team lead then reproduced deliberately:
**restoring a mutated file with `mv` puts back the original mtime**, so the next
build is judged fresh against a mutant binary. `cp` from a backup has the same
shape unless the timestamp moves.

So, for any mutation runner:

- **`touch` every source file before each build.** Do not rely on the restore
  having moved a timestamp.
- **Never share a `target/` between two workers**, and never point one at the
  repository's. Each worktree builds into its own.
- When in doubt, **delete `target/` and re-run the gates from scratch** before
  reporting. The batch that found this did exactly that and re-derived every
  number in its report from the clean build.
- Subagents inherit the lead's scratchpad directory, so two of them can collide
  on a path like `scratchpad/mutant/` without either doing anything wrong. Give
  each one its own.

A mutation verdict from a stale binary is worse than no mutation testing at all,
because it is indistinguishable from a real one in the report.


## 17. An absence assertion is only as strong as the viewport it renders into

A settings test planted a real credential in the environment, drove nine
screens, and asserted with `!contains` that the value never appeared. It looked
thorough and it was **passing for a reason that had nothing to do with the
code**: at its 100-column render the providers row was truncated, so a leaked
46-character value was clipped off-screen.

The mutation that exposed it — render the credential's value instead of
`set`/`not set` — survived. Re-rendered at 400 columns, the same mutation fails.

**Truncation makes absence trivially true.** So whenever a test asserts that
something is *not* in rendered output:

- capture at a **wide** size as well as a realistic one, and assert over both;
- and prove it, in both directions: hardened test + mutation must FAIL, and
  hardened test + clean code must pass. One of those alone proves nothing.

The same shape applies beyond a TUI — any assertion over truncated, paginated
or elided output. This is the third distinct way this project has produced a
test that passed for the wrong reason, after the vacuous poll loop and the
mutation run against a cached binary.


## 18. Compile the *other* platform's path locally, by flipping the cfg

Phase 9E went red on Linux, Windows **and** lint while macOS stayed green, for
one reason: a constant used only inside a `#[cfg(target_os = "macos")]` module
was declared outside it, so on every other target it was dead code — and
`-D warnings` makes dead code a hard error.

macOS CI can never catch this. Neither can `cargo check`, run normally, on a
Mac. But you do not need a Linux box:

```sh
cp crates/…/file.rs /tmp/file.orig
python3 -c "import pathlib; p=pathlib.Path('crates/…/file.rs'); \
  p.write_text(p.read_text().replace('target_os = \"macos\"','target_os = \"linux\"'))"
cargo check --lib --all-features        # the NON-macOS path now compiles here
cp /tmp/file.orig crates/…/file.rs      # restore, then `diff -q`
```

Flipping the gate excludes the macOS arms and includes the fallback ones, which
is exactly the compilation the other platforms perform. It found the fix was
complete in one run, with no second CI round-trip.

`rustup target add x86_64-unknown-linux-gnu` was tried first and did **not**
work here — the target installs but its `core`/`std` did not resolve, so every
dependency failed with `can't find crate for core`. The cfg flip needs no
toolchain at all.

**The rule this earns:** anything used only by a platform-gated module needs the
same gate as that module.

**And the same rule applies to documentation, which is easy to miss.** On
2026-08-26 the newly-enforced rustdoc gate went red on Linux with
`unresolved link to keyring::Error::NoEntry`. `NativeSecretStore::detect` is
public on every platform, but `keyring` is declared only under
`cfg(target_os = "macos")` — so the link resolved on the machine it was written
on and could not resolve anywhere else. A doc link is a compile-time reference
like any other. **Naming a platform-gated dependency in the docs of an ungated
item is the same defect as calling it from one**, and it fails a different gate,
so it survives the check you would think of first.

When you fix one, scan for the rest instead of letting CI find them one at a
time:

    grep -rnE '//[/!].*\[`?(keyring|windows_sys|libc|security_framework)' \
      crates/glasshouse/src --include='*.rs'

That took seconds and proved there was exactly one, which is a round trip to CI
saved. When a batch adds a `cfg`-gated backend, compile the
other side before pushing. This is the third local-reproduction recipe in this
file, after CRLF (§15) and stale mutation binaries (§16) — all three exist
because a green local run proved nothing about the platform that broke.

## 19. Dev shims: stop paying the same toll every session

Two frictions recurred in every session until they were fixed properly, and
both were being paid as a per-invocation tax rather than solved once.

**`glasshouse` is not installed.** It is a `target/` artifact of this
repository. Every `glasshouse …` in the map, the ledger and the worker packets
means "the binary this repo builds", so the orchestrator typed
`cargo run --manifest-path … --` instead, and the user — reasonably — typed
`glasshouse setup` and got `command not found`.

**`agy` cannot be launched unattended by typing its flag.** Antigravity
declares one unattended mode and no automatic review, and Claude Code's
auto-mode classifier refuses to type that flag, launch the flagged binary, or
write the config key. A previous session recorded this as a hard block and
asked the user to intervene each time. That was the wrong conclusion: it is a
tooling gap, and the fix is tooling.

**The rule: when a harness needs the same intervention every single time,
write a dev shim.** It is checked in, it is readable, it says on its face what
it does, and one `rm` removes it. That is strictly better than an orchestrator
either round-tripping to the user forever or improvising something invisible.

`scripts/dev/` holds them; both are symlinked into `~/.local/bin`, which is
already on `PATH`:

| shim | what it does |
|---|---|
| `glasshouse` | execs `target/<profile>/glasshouse` **from the checkout you are standing in**, and **warns when sources are newer than the binary** — a stale dev binary prints plausible output from last week and costs an afternoon. `GLASSHOUSE_DEV_BUILD=1` builds first; it is off by default because it would take the `target/` lock out from under a running worker. |
| `agy-gh` | execs `agy` with its blanket bypass, and **refuses to run outside a git work tree** (`AGY_GH_ANYWHERE=1` overrides). An auto-approving agent started in the wrong directory is the one real hazard, and the guard costs nothing. |

**Do not confuse a dev shim with the product's shim.** `glasshouse shim` is
Phase 9B's own mechanism, it generates a different file for *users*, and
nothing in `scripts/dev/` pre-authorizes anything inside Glasshouse. A
generated user shim still resolves a launch profile and is still refused if
that profile asks for an unacknowledged bypass; `bypass_acknowledged` remains a
statement a person makes about their own machine, consulted from the user layer
and never the project layer. `agy-gh` sidesteps that question entirely by not
going through Glasshouse at all — we are *building* Glasshouse here, not
running through it. Conflating the two cost this orchestrator a wrong answer to
the user's face.

Both were verified in both directions before being committed: the stale-source
warning fires when a source file is touched and stays silent when it is not,
and the work-tree guard refuses in `/tmp` and passes in the repository.

**One correction, found by a worker on 2026-08-26 and worth knowing.** The first
version resolved the repository from *the script's own location*, which is the
main checkout. A worker in a git worktree running `glasshouse` through `PATH`
therefore got the **main** checkout's binary — silently exercising code with
none of its own changes — and the stale-source warning could not fire, because
the worktree's sources are not newer than *that* binary. It now prefers the
checkout the caller is standing in and says so when the two differ.

**The general shape is worth more than the fix:** a tool that resolves context
from where *it* lives rather than where it was *called* will be wrong for every
worktree, and wrong silently. When a packet asks a worker to run the binary,
that worker is in a worktree by definition.

## 20. A gate that cannot fail is not a gate — the MSRV case

The MSRV check this project ran before every commit was
`rustup run 1.85.0 cargo check --locked --workspace --all-targets`. It passed
every time, all session. It was incapable of failing, for two independent
reasons, and it took a CI job to find out.

**Reason one: old cargo does not enforce `rust-version`.** Cargo 1.85.0
compiles whatever compiles and never consults the field. Cargo 1.96 refuses the
workspace outright. So the command proved "this compiles on an old rustc" while
appearing to prove "the declared floor is real". Those are different claims and
only the second one matters.

**Reason two: `rustup run <v> cargo` does not pin rustc.** rustup execs the
toolchain's cargo, and then *cargo* resolves `rustc` from `PATH`. With a
Homebrew rust ahead of `~/.cargo/bin`, the "1.85 check" compiled with rustc
1.96.1 and reported success. Pin both halves:

    CARGO="$(rustup which --toolchain "$V" cargo)"
    RUSTC="$(rustup which --toolchain "$V" rustc)" "$CARGO" check --locked ...

That is now `scripts/msrv-check.sh`, which also reads the version out of
`Cargo.toml` so the gate and the manifest cannot drift, and uses its own target
directory — sharing `target/` with the stable build turns the check into a
no-op that prints `Finished in 0.39s` and proves nothing (§16 again, in a new
costume).

**The transferable rule: apply mutation discipline to gates, not just tests.**
This project already refuses to count a test until a mutation proves it would
fail. A gate deserves exactly the same treatment, and it is cheap — raise
`rust-version` to `1.99`, confirm the gate now refuses, put it back. That
two-minute check would have caught this on the day the gate was written.

Ask it of any gate you inherit: **what change would make this fail?** If the
answer is "nothing I can think of", it is decoration.

## 21. A Unix pty is a byte pipe; ConPTY is a screen buffer

The gap §18 does not reach. That rule says to compile the other platform's path
locally before pushing, and it works because compilation is a property of source
you can flip a `cfg` on. **Runtime pty semantics are not, and they differ.**

On Unix, a pty is a byte pipe. Whatever the child writes, the master reads —
byte for byte. Line wrapping is a property of the *terminal displaying* the
bytes, not of the bytes. A ten-thousand-character line arrives as one line.

**ConPTY is not a byte pipe.** It renders into a console screen buffer of fixed
width and emits the reflowed result, so a line wider than the buffer comes back
**split across lines**. Any test that writes something long through a pty and
parses the result line by line therefore passes on macOS and Linux and fails on
Windows — and no local run will tell you, because the local pty is the one that
behaves.

This cost a CI round trip on 2026-08-26. A test planted the runner's own `PATH`
(≈3000 characters on GitHub's Windows image), dumped the child's environment
through a pty, and parsed it per line. It read back the first ~74 characters.
The truncation point is the tell: if a value comes back cut at roughly a
terminal width, suspect the pty, not the code under test.

**Two habits that avoid it:**

- **Never send CI's own environment through a pty and assert on it.** Plant a
  short value the test chooses. It removes the width problem *and* makes the
  assertion independent of whatever environment the runner happens to have,
  which is worth having on its own.
- **Ask for a terminal size when a test's output width matters**
  (`HarnessLaunch::size`). The default is whatever the fixture picked, and a
  test that silently depends on it is a test that will break on a machine you
  cannot see.

**And the honest caveat, because it is the point of this section:** an attempt
to reproduce this locally *falsified the diagnosis* — the pre-fix test passes on
macOS under a deliberately 3599-character `PATH`. The mechanism is real and the
fix follows from it, but the proof is a green Windows job, not a local run. When
that is the situation, say so in the commit rather than implying a verification
you do not have.

## 22. In a worker's worktree, `git checkout` is a delete

**The orchestrator did this on 2026-08-26 and destroyed 161 lines of a finished
worker's work**, with the rule already written down in its own memory. Writing
it here too, because the memory note was not where the mistake happened.

Workers never commit. **Their deliverable lives only as uncommitted changes**,
so to git there is no difference between the worker's edits and yours. A
path-wide `git checkout -- <file>` or `git restore <file>` reverts *the file*,
not *your change to it*.

What made it slip through is worth more than the rule. Every mutation run on
`main` that morning had used a `cp` backup and a `cp` restore — correctly, a
dozen times. The failure came from a probe: appending a small test to a worker's
file to check whether a type boundary held, then reaching for `git checkout` to
undo the append because it was "just a probe, not a mutation". **Those are the
same operation.** The size of your edit says nothing about what the undo removes.

    # in any worktree with uncommitted work — yours or a worker's
    cp file /tmp/file.bak      # before
    …probe…
    cp /tmp/file.bak file      # after
    touch file                 # so cargo rebuilds — see §16

**Never `git checkout`, `git restore`, `git stash` or `git clean` in a worktree
holding uncommitted work you did not personally create.** If you have already
done it: the worker's session still has the context. Ask it to re-create what
was lost, tell it the loss was yours, and say exactly which file — it wrote the
code and will rebuild it faster and more accurately than you can reconstruct it
from a diff you no longer have.

**And a second-order lesson.** The probe itself was unnecessary: it was checking
that a private field cannot be set from outside its module, which is a language
guarantee, not a property of this code. Before modifying a worker's tree to test
something, ask whether the compiler already promises it.

### The guard covers both harnesses, and here is what it took to prove it

The `PreToolUse` guard is registered twice: `.claude/settings.json` for Claude
Code, `.codex/hooks.json` for Codex. Codex reads that path and nowhere else.

**The first Codex copy was decoration.** It was the Claude Code document with
the path left alone, so it interpolated `$CLAUDE_PROJECT_DIR` — Claude Code's
variable, which Codex does not set — and resolved to a path that does not
exist. Being untracked did not make it harmless: Codex reads the file off disk,
so it was installed and inert at the same time, which is the worst of both. A
gate you have not watched refuse something is not a gate.

**Three things had to be established before the guard could be believed, and
only one was already recorded.** They were settled on 2026-08-26 against Codex
0.149.1 with `gpt-5.6-luna`, using a throwaway probe hook that dumped its
payload and exited 2:

1. **The document schema.** `{"hooks": {"<PascalCaseEvent>": [{"hooks": [{"type":
   "command", "command": "<shell string>", "timeout": N}]}]}}`. There is **no
   `matcher` field** — that is Claude Code's. The trust keys in
   `$CODEX_HOME/config.toml` are `snake_case`, which is a different spelling for
   a different purpose and must not be copied into the document.
2. **The command string is shell-evaluated.** So
   `"$(git rev-parse --show-toplevel 2>/dev/null || pwd)/scripts/hooks/..."`
   works, and one document is correct in the main checkout *and* in every
   worktree with no absolute path anywhere in it. This idiom was read off a
   working third-party document already trusted on the machine, not invented.
3. **A non-zero exit actually blocks, and stderr reaches the model.** This was
   the real unknown: Glasshouse only ever used Codex hooks to *observe*, so
   nothing here had established that Codex honours a refusal. It does. The probe
   exited 2, the pane printed `PreToolUse hook (blocked)` with the stderr as
   `feedback`, and the marker file the command would have written was absent.
4. **The payload is the same shape as Claude Code's** — `tool_name: "Bash"` and
   `tool_input.command`, alongside `session_id`, `turn_id`, `cwd`,
   `hook_event_name`, `model`, `permission_mode`, `tool_use_id`. So the guard
   script needed **no changes at all**. The path was the only defect.

**Then the guard itself was watched working, both directions, in a live Codex
session** — which is the standard §22 sets and the reason the first version was
not committed:

- `git checkout -- README.md` → `PreToolUse hook (blocked)`, the guard's own
  refusal shown as feedback. README was clean first, so a failure of the guard
  would have destroyed nothing; design the acceptance test so the bad outcome is
  harmless.
- `git stash list && git status --short && echo GUARD-ALLOWED-OK` → ran, printed
  `GUARD-ALLOWED-OK`. That command is there deliberately: `git stash list` had
  been a false positive in the guard hours earlier, so the allow direction
  re-proves the fix in the *other* harness.

**Two cautions.**

`.codex/hooks.json` is also the path **Glasshouse itself writes** when a Codex
launch installs project-local hooks. It is committed anyway, because a worker's
worktree needs the file and being tracked means an overwrite by the product
under test shows up as a diff instead of silently removing the guard.

Codex normally asks before running a project's hooks — "Hooks need review". On
this machine cmux's `cmux-codex-wrapper` passes
`--dangerously-bypass-hook-trust`, so **hooks in any repo opened through cmux
run unreviewed**. That is convenient here and is worth knowing generally. Note
also that trusting via the prompt is a blanket action over whatever else is
pending: an earlier probe answered "Trust all" and trusted five unrelated
plugin hooks. Prefer "Review hooks", or a bypass whose scope you know.

## 23. Re-run a worker's decisive external observations yourself

A worker that probes the outside world is reporting something you cannot
reconstruct from its diff. Source you can read; a live `401` you cannot. So
**re-run the observations a box depends on**, and do it before the box goes in.

On 2026-08-26 the 9D batch promoted six provider `model_list_endpoint`
declarations from `Unverified` to `Verified`, each citing a live probe. Six
`curl`s, under a minute, re-ran all six. **Five reproduced exactly. One did
not**, and it was about to ship as a product claim.

The one that failed is instructive because the worker had done everything
right except one step. It promoted z.ai on a `401` rather than a `200`, and
justified it with the correct control — *"a host that served nothing there
would have answered `404`"* — which it had **cited from a probe it ran against
a different service**. Against z.ai the control collapses: every path under
`/api/paas/v4/` answers `401`, invented ones included, and a version prefix
that does not exist answers `200`. The `404` behaviour is real but lives
outside the API prefix, where the probe cannot use it.

**The rule: a control has to be run against the host it is being used to
justify.** A control borrowed from another service is a statement about that
service. This is the fifth declaration in this project derived from an artifact
that did not support the use it was cited for, after Antigravity's executable
name, Codex's snake_case hook events, Claude Code's `auto-mode` subcommand and
Cursor's sandbox usage strings — so it is now a standing review step, not a
lesson.

Two practical notes:

- **Cost it honestly.** Re-probing six endpoints took less time than reading
  the diff hunk that changed them. Where an external observation is cheap to
  repeat, repeating it is not duplication of the worker's effort; it is the
  only independent evidence available.
- **Counts in citations are snapshots.** UnoRouter answered `374` entries at
  09:00 and `369` at 10:00 the same morning. That is not drift to correct, it
  is why every citation names a date — and why nothing downstream may treat a
  catalogue count as stable.

**And the finding does not diminish the batch.** The same worker killed
thirteen mutations, verified 339 leaf quotes mechanically, found two real
defects by running the binary, and volunteered five things its own packet got
wrong. The flaw was visible *in its own doc comment* precisely because it
explained itself. A less careful worker leaves nothing to catch.

## §24 — checking a box is three edits, not one

`lint` went red on `dc78129` for `Check README progress block`, and the failure
was correct. Checking a map box changes three files, and the third is easy to
forget because a script writes it:

1. the map (`☐` → `☑`),
2. the evidence ledger entry (`State:` → `COMPLETE`, with the CI run cited),
3. **`README.md`'s progress block — regenerate it with `scripts/progress.py`
   and stage it in the same commit.**

`progress.py` rewrites the block as a side effect of being run for a count. So
the trap is specific: running it *after* committing gives you the right number
on screen and a stale number in the commit. Run it **before** staging, and let
`git status --short` in the same call show you the README is dirty.

The CI job exists because the README is what a reader sees first, and a
progress claim that disagrees with the map is exactly the kind of quiet
inaccuracy this project's ledger discipline is meant to prevent. Cheap failure,
correctly placed.

## §25 — "nothing is listening" has two honest answers, and they are platform-specific

`test (windows-latest)` went red on `5cf2fc4` for
`a_capability_probe_composes_with_a_real_connectivity_check`, with
`did not answer within 509ms` where the test demanded `never answered`.

Neither the product nor the platform was wrong. A probe against a closed
loopback port gets:

- **Unix** — an immediate refusal, so the probe reports
  `ProbeOutcome::Unreachable` → "never answered: …";
- **Windows** — a dropped SYN, so the probe waits out its own bound and
  reports `ProbeOutcome::TimedOut` → "did not answer within Nms".

The distinction between those two outcomes is worth keeping; the test simply
asserted one platform's spelling of a property that both satisfy. Fixed by
asserting what the test actually cares about — not-answered, and distinct from
both reached and rejected.

**The general trap, which is not about sockets.** An assertion that passes
locally can encode a *runtime* platform difference just as easily as a `cfg`
one, and it is harder to see: there is no `#[cfg]` to flip and no compile error
to catch it. Practice §18 says compile the other platform's path; this is its
runtime sibling — **when an outcome type has several variants that all satisfy
the property under test, assert the property and enumerate the variants, rather
than asserting whichever variant this machine happens to produce.** The repair
here loops over both outcomes on every platform, so the Windows spelling is now
exercised on macOS too.

Third time a Windows job has caught something every local gate hid. The pattern
has not changed: local green says nothing about the platform that broke.

## §26 — a single `try_lock` is a coin flip, not "best effort"

`test (macos-latest)` went red on `3ec4973` for
`interrupting_a_headless_launch_does_not_leave_the_harness_behind`, then
**passed on rerun against the identical commit.** Intermittent, so a race.

The mechanism, established rather than assumed:

- `SessionRuntime::close` sends `ProcessSignal::Kill`, so if it runs the child
  dies — reading that eliminated the competing theory that the fake harness's
  `trap '' HUP` was letting it survive.
- The child survived, therefore `close` never ran.
- The forced-exit callback got **one** `try_lock` on the runtime, and the
  headless poll loop takes that same lock every 20ms. One attempt, no retry.

**Measured: 1 orphan in 100 runs under 3x CPU load.** `shutdown`'s rule —
never wait indefinitely, because failing to exit is worse than failing to
clean up — was honoured to the letter and still produced the wrong answer.
The fix is a *bounded* retry: it keeps the guarantee that matters (it always
returns, quickly) and removes the coin flip. Poisoning is now treated as
ownership rather than as a reason to give up, since a poisoned mutex would
otherwise make every retry fail for as long as we were willing to try.

### The transferable parts

1. **"Best effort" must still make an effort.** If a cleanup path gets one
   attempt at a contended resource and silently does nothing on failure, its
   failure rate is the contention rate — and nothing above it retries.
2. **Reproduce before fixing, and put the machine under load to do it.** The
   first attempt was 0/25 on an idle machine and proved nothing. 3x CPU count
   in spinners turned it into 1/100. An unreproducible race is a theory.
3. **Then make it deterministic.** A probabilistic regression that fires once
   in a hundred runs is not a regression test. Holding the lock on purpose
   turns it into one that fails every time — and the one-shot mutation kills
   it with the exact message, which a 1-in-100 test never could.
4. **A rerun is a cheap experiment.** Re-running just the failed job answered
   "intermittent or environmental?" for almost nothing, and that answer
   decided the entire investigation.

## §27 — CI is unavailable until September; the local mirror is the gate

From 2026-08-26 this repository's GitHub Actions quota is spent. It is a
**private** repo, so every run bills minutes, and there are none left.

**Know what the failure looks like**, because it is not what you expect: a push
still creates a run, and all seven jobs report `failure` within seconds, with
**no steps and no logs**. `ce9b5c0` looked like a total build collapse and was
in fact fine — it had been green on every local gate minutes earlier. Seven
simultaneous failures with no logs is a billing block, not a defect. Check
`gh run view <id> --json jobs` for empty `steps` before you debug anything.

**The gate is now `scripts/ci-local.sh`.** Run it before every commit. It
mirrors `ci.yml` deliberately — `--locked`, `RUSTFLAGS=-D warnings` on build
and test, clippy *without* `--all-features`, and the README progress check.

Worth noticing: **that is not the gate list in the worker packets**, which use
`--all-features`, no `--locked`, and no progress check. The packets have been
running a different and in places weaker set than CI all along. Prefer the
script.

### It covers five of seven jobs, and the two it misses are the ones that matter

macOS runs natively; ubuntu runs in a container; **Windows is not covered at
all**. `--windows` cross-compiles and proves the Windows path still builds — it
runs no test there. This project has already shipped one Windows-only defect
that only `test (windows-latest)` caught, and Phase 4's interrupt box can be
closed by nothing else. Do not let a green local run be written into the ledger
as platform evidence.

### A naive container is not ubuntu-latest, and it will lie to you

The first version bind-mounted the repo and ran as root. Two tests failed that
real `ubuntu-latest` had passed:

- `version_probe_child_starts_in_the_active_project_root` — `ETXTBSY`,
  "Text file busy". The test writes an executable and immediately spawns it;
  across a macOS→Linux bind mount that races. **Copy the tree in, do not mount
  it.**
- `the_shared_index_path_opens_one_file_by_name_and_never_lists_a_directory` —
  failed with *"this test proves nothing unless the directory really cannot be
  listed"*. `chmod 000` does not stop **root**, and the container ran as root.
  **Drop to a non-root user.** The test caught its own vacuity, which is the
  whole argument for writing assertions that check their own premise.

Both are fixed in the script and both were false reds. If your mirror disagrees
with the last known CI result, suspect the mirror first.

### Two traps inside the container, in order

- `rustup` resolves its own binary from `CARGO_HOME`. Moving `CARGO_HOME` to a
  writable volume breaks `rustup which`, which `msrv-check.sh` needs to pin
  cargo *and* rustc. Redirect `CARGO_TARGET_DIR` instead and mount a volume
  over the registry.
- Do not write `install >/dev/null 2>&1 && next-command`. The install failed on
  a permissions problem, the `&&` short-circuited, and the step reported a
  failure with **no output at all** — which cost more time than the bug. Silence
  a command only when you are willing to debug it blind.

### Restoring real CI

Two options, neither taken yet, both the user's call: making the repository
public restores all seven jobs free and unlimited on standard runners, and is
the only way to get Windows back; a self-hosted runner bills zero minutes even
while private but still cannot provide Windows without a Windows machine.
