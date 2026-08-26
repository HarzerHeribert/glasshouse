# Glasshouse orchestration practice

> This describes how Glasshouse is built, not what Glasshouse does. Nothing
> here is a product requirement. Capability requirements live only in
> `docs/product/capability-map.md`.

How to *run* the process the SDLC describes. `docs/process/agent-sdlc.md` says
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

`docs/process/worker-capabilities.md` defines the tiers. Two practical notes:

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

Measured numbers live in `docs/process/orchestration-measurements.md`. Add yours.

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

`docs/process/orchestration-measurements.md` is a standing, inherited experiment,
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

## §28 — a team lead idles while its subcontractors work, and the watch calls that "finished"

`worker-watch.sh` decides a pane is idle when its screen shows no spinner for
two consecutive reads. That is correct for a worker doing its own work. It is
**wrong for a team lead**, which spends much of its batch waiting on
subcontractors and is therefore legitimately idle for long stretches.

On 2026-08-26 `lead-extract` was reported "idle with NO report" about forty
minutes into a batch. It had five new files in `memory/extract/`, two test
files, and had just relayed a subcontractor's findings. Nothing was wrong.

**Inspect the pane before acting on a lead's idle notice.** The tells that it
is mid-batch, not finished:

- uncommitted work in its worktree (`git status --porcelain -uall`);
- subcontractor packets or reports in `.agent-runtime/` that it wrote;
- the status line showing an armed monitor of its own;
- pane text that reads as a *subcontractor* speaking to it ("your frozen
  code", "per the packet's instructions").

**Acking ends the watch**, so a false positive costs coverage: `worker-watch.sh`
exits once the marker is cleared. The recovery is to ack and immediately arm a
fresh watch, which resets it to waiting-for-idle. For leads, give the fresh one
a longer nag and a delayed start:

```
Monitor(command: "cd <repo> && sleep 240; scripts/worker-watch.sh <lead> <surface> <report> 300",
        persistent: true)
```

A better fix, unimplemented: gate a lead's idle on the report file existing, or
teach the script that a pane whose worktree has grown since the last read is
working even when its screen is quiet.

## §29 — a rate-limit stall can leave a worktree mid-mutation

The mutation protocol edits production code, runs one named test, then restores
and verifies the restore. If a worker is stopped between the edit and the
restore — a spent five-hour window will do it — **its worktree holds mutated
code that looks like ordinary work.**

So before integrating a batch that ran across a usage-limit boundary, read the
diff for anything the report does not claim. A mutation is usually obvious once
looked for: a deleted scope check, an inverted condition, a constant changed to
a wrong value. The worker's own mutation ledger names every one it ran, which
makes the check cheap — compare the diff against that list.

This has not bitten yet. It is written down now because the conditions for it
existed on 2026-08-26 (two Opus leads and four subcontractors on one account,
the window at 87% with 43 minutes to reset) and the cost of finding out the
hard way is a silently wrong integration.

## §30 — the self-handoff lock was spent by a different session, and nobody would have noticed

`self-continue.sh` is the mechanism that keeps an orchestrator from dying
mid-batch: two Monitors watch context and the five-hour window, and on trigger
it snapshots the tree and opens a fresh pane. A lock file makes it fire once.

On 2026-08-26 the five-hour watch fired correctly at 92% and the script did
**nothing**, reporting `already relaunched; doing nothing`. The lock had been
written at 07:55 that morning by a **different session**, on a **different
trigger** (`context`, not `ratelimit`). One shared `.relaunch.lock` meant the
second session of the day inherited a safety net that had already been spent,
and the watch then exited — so there was no net at all, at 92%, with two Opus
leads and four subcontractors on the same account.

**A fire-once lock must be scoped to the thing it fires for.** It is now
`.relaunch-<session>-<mode>.lock`.

A second bug was hiding behind the first. `ratelimit` mode reads
`${TMPDIR}/ccsl-data-${CCSL_SESSID}` to learn when the window resets, and
nothing exported `CCSL_SESSID` into the Monitor's environment. The path
resolved to a file that does not exist, the reset time read as `0`, and the
sleep was skipped — so had the lock not stopped it, it would have relaunched
**straight back into the same spent window**. It now refuses to relaunch blind
and says why, and the Monitor commands export the session id.

Both bugs are the same shape as the MSRV gate in §20 and the Codex hook in §22:
**a mechanism that had never once been observed doing its job.** The context
trigger had fired successfully that morning, which is precisely why the lock
existed to break the rate-limit trigger. Ask of any safety net: *what has it
actually been seen to do, and under which trigger?*

## §31 — the local gate had two defects, and a worker found the one that mattered

`scripts/ci-local.sh` was written on 2026-08-26 to replace CI. Within hours it
had told two separate lies, and it is worth recording that **the tool built to
enforce the evidence standard was itself the least-evidenced thing in the
repository.**

### The step that could not fail

`run_linux` nested its step inside `su ci -c "…$1…"`. The step string contains
`RUSTFLAGS="-D warnings"`, whose quote closed the `su -c` string early: the
command was mangled and its exit status meaningless. `test (ubuntu)` reported
PASS unconditionally.

`lead-record` found it — not by reading the script, but by **distrusting a PASS
on a tree it had personally just watched fail by hand in the same container**,
and then applying §20 to the gate. Measured both directions: a step running
`RUSTFLAGS="-D warnings" false` exited `0` before and `1` after.

`lint (ubuntu)` and `msrv (ubuntu)` carry no embedded quotes and were always
genuine; macOS runs natively and was always genuine. Only Linux *test* coverage
was fake, for `dc80bc2` and `a53877a`.

**Fix:** pass the step through the environment (`-e STEP="$1"`) and run
`runuser -u ci -- bash -c 'eval "$STEP"'`. Never interpolate a command into a
quoted string you also control the quoting of.

### The container tree was a union of every worktree that ever ran the script

One shared `glasshouse-ci-home` volume, and `tar -x` writes over a tree without
removing what is no longer in it. `lead-record` ran the gate from its own
worktree; the next run extracted `main` on top; the build then compiled
`main` **plus** `tests/checkpoint_portability.rs` from another branch, and
failed on a file `main` does not contain. Two leads running the gate
concurrently also raced on that one volume.

The failure mode is loud in this direction and silent in the other: a file
deleted in the source survives in the container and keeps compiling.

**Fix:** volumes keyed to the worktree (`glasshouse-ci-home-<hash of repo>`),
and `rm -rf /home/ci/repo` before every extract. The build cache at
`/home/ci/target` is deliberately kept.

### And underneath both, a real Linux-only flake

With the gate honest, `version_probe_child_starts_in_the_active_project_root`
failed — **only on Linux, only under the full suite, never alone** (three of
three in isolation, beside 865 siblings it failed). It writes an executable and
immediately runs it while other tests fork; a child inheriting the still-open
write descriptor makes Linux refuse the exec with ETXTBSY. macOS does not
enforce that, so it had never been seen locally, and CI had been lucky.

Both call sites now retry on errno 26, bounded, with every other spawn error
raised immediately. Production never writes a program and then runs it, so the
retry asserts nothing false about Glasshouse.

**The transferable part:** a gate is a product, and it deserves the same
question as any other — *what change would make this fail?* Two of the three
findings here were invisible to reading and needed the gate to be run against a
tree already known to be broken.

---

## §32 — put the caller's file in the partition, or the batch ends in a patch

Observed three times now, and the third was a controlled comparison.

`lead-record` and `lead-extract` ran the same round: same model, same effort,
same process, packages of 28 and 25 boxes. `lead-record` had a **wide**
partition — `main.rs`, `shell/**`, `database.rs` — and closed **25 of 28 from
its own worktree**. `lead-extract` had a partition covering only
`memory/**`, produced the strongest evidence in the batch — 23 mutations, 22
killed, +81 tests — and closed **zero**. Not one box. Everything it built was
reachable from nothing, because *nothing in the shipped binary produced a
memory*, and all three lines that would give the extractor a caller live in
`main.rs`.

Two patches it wrote but was forbidden to apply — 2 lines and ~40 lines — took
it from 0 to 17.

**The rule.** Before sizing a package, find where each capability's production
caller will live. If that file is not in the partition, the batch cannot close
the box no matter how good the work is (§5: a mechanism with no production
caller does not get its box). Either widen the partition to include it, or
schedule a thin wiring batch straight after, and say which in the packet.

**The corollary, which corrects a question this project kept asking.** "Is 40
boxes too large for one lead?" was the wrong question — neither lead ran out of
context or time. What bounded the result was partition width. **Size a package
by the files a capability's production path touches, not by how many lines the
map lists.**

**Ask the lead for the patch.** `lead-extract`'s report is the model: exact
text, exact insertion point, verified against the live file rather than
recalled, plus the compile fact that made it non-obvious (`main.rs`'s command
match has no `_` arm, so adding a `cli.rs` variant alone does not build). That
turned a lost batch into a twenty-minute integration.

---

## §33 — a lead may hand you a judgement; take it, and say which way you went

`lead-extract` built `glasshouse memory extract --reply-from <file>`: the whole
extraction pipeline with the model's reply supplied by hand, because Phase 39
does not exist and there is nothing to call. It then declined to decide whether
that satisfies the map's *"Allow memory extraction to run manually for
debugging and evaluation"*, wrote down the argument both ways, and left the box
open with the reasoning attached.

That is correct behaviour and worth protecting. A worker that assumes the
generous reading gets a box ticked on a caller that does not do what the line
says; a worker that assumes the strict one strands finished work.

**What the orchestrator owes back is a decision with its reasoning in the
ledger**, not a silent tick. Here: the manual-extraction line is **closed** —
a person really can run extraction for debugging and evaluation, and the
command says `no model was called` on every run so the evaluation can never be
mistaken later for evidence a model did it. The adjacent line the lead offered
to close with it, *"Keep memory-extraction failure non-fatal to the coding
session"*, stays **open**: a CLI invocation is not a coding session, and
nothing is at risk when extraction fails inside one. Same caller, two different
answers, because the two sentences describe different callers.

**Sharpened 2026-08-26, by the next lead pushing back on this very paragraph.**
`lead-mem6` closed the task-completion trigger citing this section as
precedent, stated the counter-argument, and left the call here — noting that if
the strict reading won, *this* decision should be re-examined by the same
standard, because manual extraction calls no model either. It was right to
press, and the criterion needed to be better than "a CLI is not a coding
session":

**The test is whether the capability completes and produces its result in the
shipped binary — not whether a model is called.**

Manual extraction completes: `--reply-from` supplies the model half at the
user's direction, the pipeline runs, and memories are stored. The
task-completion trigger cannot complete: it fires on every completed task and
dead-ends, because nothing can supply the model half at a turn boundary. Both
were verified by running the binary. So the manual line stays closed, the
trigger stays open, and the two are not inconsistent.

The useful general form: **ask the capability as a question a user would ask,
and see whether the honest answer is yes.** *Can a person run extraction for
debugging?* — yes, here is the command and here are the memories. *Can
Glasshouse extract memory after a task completes?* — it tries, every time, and
reports it has no model. The second is not a yes.

---

## §34 — the Linux gate is flaky under load, and it was flaky before your batch

Integrating `lead-extract`, `scripts/ci-local.sh` reported one FAIL:
`events_lifecycle::a_crashed_worker_leaves_its_output_and_its_event_history_behind`,
Linux only, `the crashed worker's terminal output must survive it; got ""`. The
batch had touched nothing within reach of it.

**Do not fix it, and do not wave it through. Attribute it.** Run the same
container, same step, on a tree at `HEAD` without your changes:

- the test alone on `HEAD` → **passed**;
- the **full workspace** on `HEAD` → `events_lifecycle` passed and
  `pty_smoke::a_recorded_antigravity_conversation_is_resumed_through_its_own_flag`
  **failed** instead, with `no resumable session in the listing`.

Two trees, two different tests, one class of failure: a child's output not
observed in time under Linux container load. It is nondeterministic, it lives
on `HEAD`, and it is not the batch's.

**Why the full-suite run is the load-bearing step.** Every one of these passes
alone and fails beside ~900 siblings — the same shape as the ETXTBSY fork/exec
race in `integrations/version.rs`. Running the single failing test to "check"
it will always tell you it is fine, which is the most misleading answer
available.

**What this costs, and the standing debt.** A gate that fails at random teaches
people to ignore it, which is §20's problem wearing the opposite mask: a gate
that cannot fail proves nothing, and a gate that fails for no reason gets
overridden. Recorded here rather than papered over: **the Linux pty tests need
the same treatment the ETXTBSY race got — a bounded wait on the observation
rather than an assumption that the child's output has already been drained.**
Until then, a Linux FAIL on a pty-shaped test is attributed, not assumed, and
the attribution run is part of integrating any batch it appears in.

---

## §35 — a caller every test bypasses is not a caller

§5 says a mechanism with no production caller does not get its box. `lead-route`
found the sharper form, and it is the finding of batch 22–23.

M18 deleted `apply_gateway`'s call to `Gateway::routing().bind` — the single
line on the production launch path that records a session's provider assignment
— and **nothing failed**. All ten gateway conformance tests bound the assignment
themselves in their own helper, so the entire suite passed against a build where
the shipped binary recorded no assignment at all. M24 was the same shape one
layer down: `to_launch_profile` dropping the stored pin broke nothing, because
the profile-side test constructed its `LaunchProfile` by hand.

**A caller you can delete without a test noticing is, to the test suite, not a
caller.** The box would have been ticked with the production path dead and a
helper keeping the tests honest.

**How to catch it, cheaply.** For every box you are about to close on "it has a
caller now", mutate *the call itself* — not the callee. If the suite survives,
your tests are all entering below the production entry point. The fix is one
test that goes in through the function the binary actually calls:
`resolving_a_gateway_backed_profile_assigns_the_session_a_provider_and_a_model`
goes through `resolve_with_gateway`, which is what `main.rs::launch_session`
calls, and it killed M18 immediately.

This is why fixtures that "set up the world" are dangerous in exactly the phase
where a capability is being wired: the helper that makes a test convenient is
the helper that reproduces the production step you are trying to prove exists.

---

## §36 — ask whether a caller *exercises* the policy, not whether its file is in the partition

§32 said: put the caller's file in the partition. Batch 22–23 shows that is
necessary and not sufficient, and `lead-route` caught the packet — mine — being
wrong about its own rule.

The packet told `lead-route` that Phase 9I's consumer was `memory::extract`'s
`ExtractionModel` seam, whose caller `lead-mem6` was building in the same round.
That is true and irrelevant: the caller being built is a caller for
**extraction**, not for **model selection**. `ReplyFromFile` calls no model, so
nothing in the binary asks a router which resource a disposable job should use.
Four of 9I's fourteen boxes were unclosable from that partition before the batch
started, whatever was built in it.

**The tell was available in advance and took one command:** `grep` for a call
site of `ExtractionModel::complete` outside a test. It finds exactly one, in
`main.rs`, behind `--reply-from`.

**The refined rule.** Before sizing a package, for each capability name the
function that will *ask* the policy or *use* the mechanism, and check that this
function exists or is being built **for that purpose**. A seam being built by
another lead is not a consumer of your policy unless someone is calling it with
your policy's question. Write the grep in the packet, not the assumption.

---

## §37 — `cargo fmt --all` cannot be handed to a worker with a one-file scope

Known since batch 18–19 as "`cargo fmt --all` crosses file partitions"; batch
22–23 shows the sharper version, and it cost a subcontractor real time.

`sub-route-tests` had a `FORBIDDEN FILES` list and a single writable test file,
and its packet said to run `cargo fmt --all` as step one. It did, and the
command reformatted two files in `src/routing/` that its own forbidden list
named. It caught this itself, restored them, fell back to `rustfmt` on its one
file, and reported it upward — which is the only reason anyone knows.

**`cargo fmt -p <pkg> -- <file>` does not narrow to that file.** A packet with a
narrow file list must say `rustfmt <the one file>` and nothing else, and the
lead must run `cargo fmt --all` itself before its own gate.

---

## §38 — "run the binary" is not an instruction unless the packet says how

Every packet in this project says the real defects come from running the shipped
binary. `lead-route` pointed out that this is unactionable as written:
`glasshouse run` refuses a session when stdin or stdout is not a terminal —
correctly — and neither the `Bash` tool nor `script -q /dev/null` supplies one.

**The answer is a cmux pane**, which is already the project's mandated idiom for
workers. Say so for *probes* too, in the packet, with the command. A worker that
cannot start the binary will substitute a fixture and report it as a run.

Related, and worth one line in any packet promising live provider evidence:
**an account's state decides whether a free request is servable, not the model's
price.** OpenRouter answers `402 Insufficient credits` for `:free` models on an
account that never purchased credits. The honest fallback is to report the `402`
as the finding — which is what happened, and it exposed a real classification
defect that no fixture would have produced.

---

## §39 — a relay to a live worker costs it a turn, and must be verified as if it were your own change

Two mistakes in one relay, both mine, during batch 22–23.

`lead-route` finished first and reported that its Phase 9H line 515 wanted a new
`LifecycleEvent::GatewayBackendChanged`, which needs a new value in
`lifecycle_events.kind`'s `CHECK`. `lead-mem6` was live and writing migration 6,
so I relayed it as time-critical: *fold the new kind into the migration you are
already writing, it costs you one string.*

**It declined, and it was right on the merits.**

- Migration 6 ALTERs `memories` and rebuilds `memories_fts`. **It does not touch
  `lifecycle_events` at all**, so there was no rebuild to piggyback on.
- That `CHECK` belongs to **migration 5**, which shipped and is append-only.
  SQLite cannot add or drop a `CHECK`, so admitting a new kind means renaming,
  recreating, copying, dropping and rebuilding that table and its three
  triggers — which costs exactly the same in migration 7 as in 6.
- And the risk was specific to that batch: `lifecycle_events.seq` is
  `INTEGER PRIMARY KEY AUTOINCREMENT`, and migration 6 had just made
  `memories.source_event_first`/`_last` reference it. **Rebuilding the referent
  inside the migration that introduces the reference**, untested, at the end of
  a batch, silently re-points every extracted memory's provenance if `seq`
  renumbers.
- The halves cannot be split either: `every_lifecycle_event_kind_is_one_the_schema_accepts`
  asserts the variant set and `LIFECYCLE_EVENT_KINDS` are equal **in both
  directions**, so the enum variant and the `CHECK` value must land together or
  the suite fails immediately — and if it did not, the variant would fail as a
  constraint violation on the event-writer thread, where nobody is looking.

**The transferable mistake: I verified the fact and not the recommendation.** I
checked that the `CHECK` existed and that SQLite cannot `ALTER` one — both true
— and then relayed a *cost claim* ("one string", "cheaper now than later") that
I had not checked at all. A relay carries your authority into a worker's batch.
Verify the recommendation to the same standard you would verify your own edit,
or send the fact and explicitly leave the judgement to the worker.

**The second mistake: the relay ended the worker's turn.** `lead-mem6` was busy;
the message arrived, it read it, and it went idle mid-batch with no report and
three shells still open. Only a watch caught it. A `SendMessage` to a live
worker is not free and is not asynchronous from the worker's point of view — it
consumes a turn and can end one.

**How to relay, then.** Prefer to hold non-urgent findings until the worker's
report lands; a stranded box costs one follow-up batch, and a derailed lead
costs the batch it was in. When it genuinely cannot wait, say in the first line
that the message is optional, say what to do if the work is already settled, and
say explicitly *do not stop for this*. And when a worker declines with reasoning
this specific, the reasoning is the deliverable — it belongs in the ledger
whether or not the change happens.

**What it hands to the next batch.** Migration 7 is now a small, well-specified
piece of work: own `database.rs`, `events/mod.rs` and `events/log.rs` together,
rebuild `lifecycle_events` with `gateway_backend_changed`, and prove `seq`
survives the rebuild with a test that stores a memory's event range across it.

---

## §40 — never run the local gate beside anything else

`lead-mem6` launched `ci-local.sh` while its own `cargo test -p glasshouse` was
running natively in the same worktree. That run failed the ubuntu leg **and**
produced a macOS `pty_smoke` failure that did not reproduce. Both had one
cause: **the lead was the load.**

The gate competes with itself for the machine and then reports the result as if
it were about the code. Every pty test in this project is timing-sensitive
under parallel load — that is §34's standing debt — and the local gate runs the
whole workspace suite in a container *and* natively.

**Run it alone.** Close idle worker panes first; a finished agent still holds
memory, and a lead with three shells open is not a quiet machine. If a Linux
pty test fails, check what else was running before reaching for §34's
attribution procedure — the cheapest explanation is that you were competing
with yourself.

**And §34's procedure gained a second half from the same batch.** Attributing
a Linux FAIL by running `main` once and seeing it pass is *one sample of a
nondeterministic event*, not evidence the failure is yours. `lead-mem6` ran the
`main` tree (clean) and then re-ran **its own unchanged tree**, which passed
too. Same tree, two runs, two answers — which is the actual proof of
nondeterminism, and stronger than the `main` comparison. Do both, and prefer
the second: a re-run of your own tree needs no assumption that `main` and your
branch are otherwise comparable.

---

## §41 — a mutation can be weak in the same way its test is

`lead-mem6`'s only surviving mutation was not a gap in coverage. The test
`no_event_describes_itself_as_nothing` asserted every rendered event line was
non-blank; `describe` writes `"[{seq}] {what}"`, and the prefix is
unconditional — so an arm returning `String::new()` still produced `"[8]"` and
passed.

**The test asserted a property that could not fail, and the mutation attacked
the same unreachable property.** A survivor normally means the test is too
weak; here the mutation was weak in exactly the same direction, so the pair
agreed and neither was testing anything.

The lead checked whether the mutant broke anything else — it did not, because
`"[8] "` is non-blank and the chunk builder keeps it — and then rewrote the
test around what can actually vary: the text *after* the prefix, and whether
two variants describe themselves identically. That kills the original mutation
and a new one (one arm's text copied into another) that the original test could
never have caught.

**The transferable move:** when a mutation survives, do not only ask whether
the test is strong enough. Ask **what the test and the mutation both assumed**.
A prefix, a wrapper, a default, or any always-present scaffolding can make a
whole class of mutations unreachable, and the coverage looks fine from either
side.

---

## §42 — a find-and-slice edit that misses its end marker silently eats the file

The orchestrator truncated `docs/process/handoff.md` from 1715 lines to 54 in a
single commit, and pushed it. The edit was meant to replace one section.

```python
start = s.find("## Next action")
end   = s.find("**Before sizing that packet, read practice §32.**")
end   = s.find("\n\n", s.find("Size a package by the files a capability's production caller touches.", end))
s = s[:start] + new_next + s[end:]
```

**That sentence is wrapped across two lines in the file.** The inner `find`
returned `-1`; `s.find("\n\n", -1)` then searched from the last character and
also returned `-1`; and `s[-1:]` is the final newline. Everything between the
section start and the end of the file was replaced by the new text. No
exception, no warning, and the commit's own `--stat` was the only witness:
`1 file changed, 43 insertions(+), 1704 deletions(-)`.

**Three rules, each of which alone would have caught it.**

1. **Assert every offset before slicing.** `assert start != -1` and
   `assert end > start` cost one line. A `find` that returns `-1` is a *value*,
   not an error, and `-1` is a legal index everywhere it is then used.
2. **Never search for prose that the file may have wrapped.** Markdown in this
   repository is hard-wrapped at ~76 columns, so any search string longer than
   a few words is likely to contain a newline in the file and never match.
   Anchor on short, unwrapped things — a heading, a line you have just read
   back — or address the file by line number after reading it.
3. **Read `git show --stat` before pushing a documentation commit.** A commit
   whose deletions vastly exceed its insertions is either a deliberate deletion
   or a mistake, and you always know which. This one was pushed because the
   `--stat` was not looked at until afterwards.

**Recovery, for whoever needs it.** The content was one commit old, so
`git show HEAD~1:<file>` reproduced it exactly; the fix was to rebuild from that
text with the correct boundary and then `diff` the result against it to prove
the only change was the intended one. **Do not reach for `git checkout` or
`git restore`** — the repository's guard blocks them for good reason, and
`git show` to a scratch file needs no such permission and destroys nothing.

---

## §43 — check sibling packets against each other mechanically, and answer a worker that asks

Two orchestrator failures in one round, both cheap to prevent.

**1. The same file was given to two live workers.** `shell/state.rs` appeared
in the `YOURS` list of both `wire-disposable` and `migration-7`. "Never let two
workers edit the same file at once" is one of the three non-negotiables handed
to every team lead, and the orchestrator broke it while writing the packets that
hand it out. It did not bite — `migration-7` added ten lines and
`wire-disposable` never opened the file — but that was luck, not design.

**The fix is mechanical, so do it mechanically.** After writing a round's
packets, extract every `YOURS` list and intersect them pairwise. If two lists
share a path, one of them is wrong. Do not rely on reading three packets and
noticing; the whole reason `FORBIDDEN FILES` is a list is that humans and models
both fail at set arithmetic done by eye.

**2. A worker asked a question and waited.** `migration-7` finished, hit a
genuine decision — whether to apply three patches to files outside its
partition — presented the options, and idled. The watch fired correctly. The
orchestrator was mid-way through an unrelated investigation, read the
notification, and did not act on it; the **user** noticed the worker was waiting
and said so.

That is precisely the failure the nagging watch was built to prevent, arriving
in a form the watch cannot fix: the reminder was delivered and not acted on.
A worker blocked on a question is worse than a worker that has finished — it is
burning nothing and producing nothing, and only the orchestrator can unblock it.

**Rule: a watch event that says a worker is idle gets looked at before the next
piece of work, not after.** Reading the pane costs one command. And when a
worker offers a numbered choice, answer it in the pane — `cmux send` then
`cmux send-key Enter` — because a report file cannot answer a menu.

**What the worker did right, and it is the standard:** it needed those three
files to prove its migration end to end, so it patched them locally, ran the
full suite green, **reverted them to their exact committed byte content**
(verified with an empty `git diff`), and reported the patches for the
orchestrator to land. Verification without ownership, and no residue.

---

## §44 — a packet's hypothesis is an anchor; label it as killable and reward the kill

The `pty-flake` packet named a cause: *on Linux, when the last slave descriptor
closes, a read on the master can return `EIO`, and buffered output that was
never read is gone.* It read plausibly, it fitted both symptoms, and **it was
wrong.**

The worker killed it with 600 trials — 200 at each of three delays, in the same
container, with the child reaped before the first read — and lost **zero bytes**
every time. Linux hands the reader everything that was written and *then*
reports `EIO`.

Then it found the real cause by instrumenting the failing assertion to keep
looking after it failed: **a pty child's exit becomes observable before its
output does**, because the exit comes from `waitpid` while the output must be
copied by a different thread that, beside ~900 siblings, does not always get a
slice in time. The window is 1.1ms–2.2ms wide and it was captured in situ.

**Why this is worth a section.** A hypothesis in a packet carries the
orchestrator's authority into work the orchestrator is not doing. A worker that
takes it as a starting point will spend its batch confirming it, and a
confirmation obtained by looking only where you were told to look is worth
nothing. This packet survived that because it said *"Confirm it or kill it
before fixing anything… if it is something else, say so, with the evidence"*
and listed five alternatives. That sentence is why the report opens with a dead
hypothesis instead of a fake confirmation.

**So: state hypotheses, and state them as killable.** Give the reasoning and
the precedent — they save real time — and then say plainly that the first job
is to test the hypothesis, not to act on it. A packet that asserts a cause
without that instruction is a packet that will get its cause back.

**The corollary for reading reports:** when a worker says the packet's premise
was wrong and shows the trials, that is the single most valuable paragraph in
the report. It also means the *defect was different from what was budgeted for*
— here, smaller: Glasshouse never lost a crashed harness's output, it reported
it as absent when asked inside a two-millisecond window. Re-read the box against
the real cause before deciding what the fix proved.

---

## §45 — the orchestrator's pane is never closed, and every session is remote-controllable

Two operating facts that are not preferences, recorded because closing the
wrong pane silently stops the whole fleet.

**A `caffeinate` is holding the machine awake against the orchestrator's own
session.** `caffeinate -i -m -w <pid>` waits on that process: while it lives the
MacBook will not idle-sleep and the fleet keeps running unattended; when it
exits, the machine sleeps and everything stops. So the orchestrator's cmux
workspace is **never** closed — not to tidy up, not at the end of a round, not
when handing off. A handoff opens a *new* workspace and the old one is left for
the user to close.

`scripts/close-worker.sh` refuses its own workspace for this reason, comparing
against `cmux identify`. If that refusal ever fires, it is right.

**Start every session with `--remote-control <name>`, not just the
orchestrator.** This one runs as `glasshouse-orchestrator`, which is what makes
it steerable from a phone. The point of a fleet you can leave alone is that when
it *does* need you, you can reach it from wherever you are — and a worker
started without a name can only be reached by walking back to the machine. It
costs one flag at dispatch and there is no reason to omit it.

---

## §46 — closing a worker pane discards two things, so do it with the helper

`scripts/close-worker.sh <workspace> <name>` exists because closing a pane by
hand loses two things and both were paid for.

1. **The conversation.** A finished worker holds an hour of reading. cmux
   already stores its restart command — `cmux surface resume get` prints it
   verbatim, session id included — and that command dies with the workspace
   unless it is written down first. The helper captures every surface's line
   into `.agent-runtime/resume/<name>.txt` **before** closing. A question that
   surfaces two weeks later then costs one command instead of a re-derivation.
2. **What the pane was running.** Four `glasshouse` processes were found
   spinning at ~99% CPU, three of them nineteen hours old, orphaned by panes
   that had been closed. The helper scans afterwards and says what it found.

**It reports and never reaps**, deliberately matching the fixed requirement
written into Phase 10A: a process that is alive and unaccounted for is exactly
where the least is understood, and killing what you do not understand is worse
than saying so. It prints the `kill -TERM` line and leaves the decision alone.

This is the interim hook. When Phase 10A ships adoption, identity verification
and quarantine, the product does this properly and the script retires.

---

## §47 — the orchestrator needs a dead-man's switch, because every other watch is event-driven

The user asked what stops the orchestrator going idle, having watched one sit
idle *not* waiting on agents — just waiting for input. The honest answer was
**nothing**.

Every watch in this project fires on an event: a worker changes state, a
threshold is crossed, a background command exits. That covers idle-while-work-
runs. It does not cover **idle with nothing running**, because with no event
there is nothing to fire. The loop ends quietly, the machine stays awake, and
the fleet is stopped until a person notices.

`scripts/orchestrator-heartbeat.sh` watches the orchestrator's own surface the
way `worker-watch.sh` watches a worker's, and nudges only when three things hold
together: the pane has been genuinely idle for several checks, no worker is
running or owed a review, and the map still has open boxes. One nudge, then a
long back-off — the goal is restarting a stopped loop, not hectoring a working
one. `touch .agent-runtime/stopped` silences it, because stopping is a decision
and not a fault.

**It found its own defect within minutes of being armed, which is the part worth
recording.** Its first version asked only whether a worker was *waiting to be
acknowledged* — a marker file that appears when a worker goes idle. A worker
still **working** leaves no marker, so the heartbeat fired while a batch was
mid-flight. Two different questions:

- *waiting for review* → a marker in `.agent-runtime/idle/`
- *still running* → a live `worker-watch.sh` process

Both must be false before the orchestrator is genuinely idle. Match the `bash`
process rather than the shell wrapper, or every watch counts twice.

---

## §48 — park a question instead of stopping for it

An orchestrator idling because it needs a decision is the fleet stopped, and
most decisions do not deserve that: they block one package, not the project, and
the answer is as good in twenty minutes as now.

`scripts/ask-user.sh <slug> "<question>" "<option>" "<option>"` opens a **Haiku**
pane whose only job is to put the question in front of the user, wait as long as
it takes, and write the answer to `.agent-runtime/answers/<slug>.txt`. The
orchestrator dispatches something else and collects the answer when it lands.
`--list` shows what is outstanding; `--check <slug>` reads one.

**The test for whether to stop is whether the PROJECT is blocked, not the
package.** If another package can be dispatched, dispatch it and park the
question.

Two details that are not incidental. The asking session is told to ask and
nothing else — not to explore the repository, not to offer an opinion unless
invited — because a cheap model given a repository will start having views about
it. And the options are passed through a file rather than interpolated into the
prompt string: a question containing a quote would otherwise rewrite the command
that asks it.

---

## §49 — run the validator before every dispatch, and quote box lines unwrapped

`scripts/validate_round.py` is now the gate on a round, and it earns its place
immediately: run against the two packets this project actually dispatched on
2026-08-26 it refuses them and names **two** collisions, not the one the
orchestrator knew about.

    [partitions-disjoint] packet-wire-disposable.md:77 claims
      crates/glasshouse/src/shell/state.rs and packet-migration-7.md:102
      also claims crates/glasshouse/src/shell/state.rs
    [partitions-disjoint] packet-wire-disposable.md:78 claims
      crates/glasshouse/tests/memory_*.rs and packet-migration-7.md:105
      also claims crates/glasshouse/tests/memory_provenance.rs

The second was invisible to the eye and is the same class as the first, one
glob away from literal: one packet claimed a whole `memory_*.rs` glob while the
other claimed a specific file inside it. **Two collisions in one round, and the
orchestrator caught neither.** This is the check a human is worst at and a
script is perfect at, which is the whole argument for it.

Usage, before any round is dispatched:

    scripts/validate_round.py .agent-runtime/packet-*.md

**And a format rule that came out of building it.** Packets quote box lines
wrapped across two lines for readability; the map stores each box as one long
unwrapped line. The validator has join-then-normalize logic purely because of
that mismatch, and the worker only discovered it by opening both files side by
side. This is §42's lesson again in a second costume — **prose that a file has
wrapped will not match a search for it.** Either quote box lines unwrapped in
packets, or accept that every tool reading them needs to reconstruct.

`scripts/discover.py --seam <symbol>` is the other half: it reports non-test
call sites and says plainly when there are none, because that is §5 and it is
the finding that costs whole rounds. Treat a method-call match as a lead rather
than proof — it cannot resolve the receiver's type, and it says so.

---

## §50 — the two worlds, and the lint that keeps them apart

This repository holds two kinds of document, and an agent must be able to tell
which it is holding **from the path alone**:

    docs/product/   what Glasshouse IS    capability map, design decisions, evidence
    docs/process/   how we BUILD it       this file, measurements, worker tiers,
                                          the orchestrator prompt, the handoff,
                                          the worker-to-worker hook protocol

The confusion is not hypothetical and it is not cheap. `harness-hook-protocol.md`
reads like a product specification and is a contract between our own worker
sessions; the orchestrator mis-filed it once while writing a design page. And
`scripts/check-doc-boundary.sh`, on its first run, found **four citations of this
very file inside shipped Rust source**.

**Only one direction is forbidden.** Product source may cite `docs/product/**`
and must never cite `docs/process/**`. A process document cites the product
freely — that is what it is for. The asymmetry is the point: shipped code that
justifies itself by referring to notes about how we ran our agents is unreadable
to anyone who does not have our transcripts, and unactionable even to those who
do.

**The fix for a violation is never to delete the thought.** Restate it in
`docs/product/design-decisions.md` as a decision about the product, and cite
that. Both of the real ones converted cleanly and are better for it: source
guards read by `str::lines` so they cannot be blinded by line endings, and a
pseudo-terminal child's exit is observable before its output is. Those are facts
about Glasshouse. Where we learned them is not.

The lint is in the gate. It was kept out until the four existing violations were
converted, because a gate that starts red teaches everyone to override it.

---

## §51 — a local gate can afford questions a metered one could not

GitHub Actions was billed by the minute, so `ci.yml` asks only what it must.
`ci-local.sh` costs a laptop's evening. Three checks now exist because of that,
and none of them could have justified a paid runner.

**Doc boundary** — §50. Cheap, and it caught four real violations immediately.

**Evidence coverage** — `scripts/check-evidence-coverage.py`. `CLAUDE.md` says
*do not check a box until its evidence-ledger entry is COMPLETE*, and until now
**nothing enforced that at all**. Its first run:

    evidence coverage: 33/34 phases with ticked boxes have evidence
      1 phase(s), 8 ticked box(es), with no evidence entry
      Phase 0

**Its first version said 52 boxes across six phases, and that was its own bug,
reported to the user twice before it was checked.** Evidence files are named for
the phases they cover and some cover several — `phase-12-18-and-19.md` — and the
first parser read that stem as one opaque key matching none of 12, 18 or 19. A
second version read headings too and still missed them, because the phases were
named in the filename rather than in a heading.

The real gap is Phase 0, eight boxes, ticked before the ledger existed.

**It ships warn-only**, with `--strict` to fail, for the reason §20 gives from
the other side: a gate that starts red is a gate people learn to override.

**The lesson is not about parsing.** A check whose first run produces an alarming
number is at its least trustworthy exactly when it is most persuasive. Verify
the number before anyone acts on it — a worker dispatched on the 52 would have
written evidence that already existed for five of the six phases.

**A flake rate** — `ci-local.sh --flake`, `FLAKE_RUNS=10`. The residual SIGABRT
in `pty_smoke` fails about once in thirty-seven full-suite runs, and a single
green pass says exactly nothing about it. This runs the pty-sensitive suites N
times and reports failures over attempts. **It is a measurement and never fails
the gate** — a rate is not a verdict, and treating it as one would either hide
the number or block on noise.

**And the gap that stays a gap.** Windows containers share the host's Windows
kernel, so a `linux/aarch64` Docker daemon cannot run `mcr.microsoft.com/windows`
images at all — verified, not assumed: the host reports `linux / aarch64` and the
manifest reports `windows / amd64`. No amount of local rigour buys Windows
evidence. The only local route is a Windows 11 ARM virtual machine running the
suite natively, and it is the one thing that could close Phase 4's interrupt box.

---

## §52 — the reading tax was never in the documents, it was in the packet template

The evidence ledger was split into forty per-phase files to cut the orientation
cost. Measured afterwards, the cost had **gone up**: 131,281 words, ~175,000
tokens. Splitting a file changes what a reader *can* read. It changes nothing
about what a reader is *told* to read.

**And `CLAUDE.md` never told a worker to read all of it.** Its list opens
*"Before working as the primary orchestrator…"*. The scoping was correct in the
one file everybody blames.

**The packet template was the leak.** Nine packets in this project's history
open with *"Read `CLAUDE.md` and the files it names"* — which hands a Sonnet
running a four-box package the orchestrator's entire 175k reading list. That is
where `wire-disposable` spent more context orienting than working.

A worker needs six things, and five of them are small:

1. `CLAUDE.md`
2. its packet
3. `docs/process/worker-capabilities.md` — what its tier may and may not decide
4. the practice sections its packet names, **by number**
5. `docs/product/evidence/phase-<id>.md` for its phases
6. its box lines, quoted in the packet

**Measured: ~4,500 tokens against 175,000.** A factor of thirty-eight, and it
cost one paragraph in `CLAUDE.md` and a corrected sentence in the template.
`scripts/discover.py --phase <id>` prints items 5 and 6 together, which is what
the split was for.

**The transferable lesson is about where a cost lives.** The expensive thing
looked like a document-layout problem and was an instruction problem, and the
layout work — worth doing on its own merits — bought none of the saving by
itself. **Before restructuring something to make it cheaper, find the sentence
that makes it expensive.** If that sentence survives the restructure, so does
the cost.

---

## §53 — worktrees are cheap to make and expensive to keep

Eleven worktrees in one day took a 926GB disk from comfortable to **99% full,
13GB free**, and nobody noticed until the machine complained. The user asked
whether the runaway workers had written logs. They had not: `.agent-runtime` is
1.7MB and the logs are nothing. The space was build product.

    worker worktrees' target/     44 GB across 28 worktrees
    per-worktree ci volumes      ~42 GB, one Linux build cache each
    (plus 49GB of dead Docker Desktop data, unrelated and the user's to clear)

**Both are the orchestrator's doing and neither is visible.** A worktree is one
command and looks free; its `target/` is invisible until you go looking, and
`ci-local.sh` creates a Linux build volume per worktree that nothing ever
removed.

**The per-worktree volume stays.** One shared volume was tried and produced a
build of `main` that compiled a test file from another branch — a shared
*source* tree is a wrong-green waiting to happen, and sharing a `target/`
between two trees is the same hazard one layer down. The answer is not to share
the cache; it is to **delete it when the worker is done**.

So `scripts/close-worker.sh` now reclaims a worker's `target/` and its ci volume
as part of closing the pane, and `scripts/reap-worktrees.sh` reports or reclaims
across all of them at once. Both refuse to touch three things: any tracked file,
any uncommitted change, and **the worktree itself** — which holds the diff, and
until the work is integrated and pushed that diff is the only copy.

**Reclaiming build output is not a destructive act and should not be treated as
one.** It is gitignored, it is regenerable, and the only cost is a rebuild.
Deleting the *worktree* is a different question and stays the user's.

**The measurement to keep:** 44GB reclaimed, zero source touched, and the disk
went from 99% to 77%. Run `scripts/reap-worktrees.sh` at the end of any round
that made more than two worktrees.

---

## §54 — a watchdog that goes blind must not report success, and must not quit

The orchestrator heartbeat announced:

    ORCHESTRATOR IDLE and the capability map has no open boxes left. Nothing to do.

There were **1,091**. Then it exited, so nothing was watching any more.

The cause was one fallback. Its open-box count was
`grep -c '^☐' "$MAP" 2>/dev/null || echo 0` — and when the capability map moved
to `docs/product/`, a monitor still holding the old path got a failed grep,
turned it into **zero**, and read zero as *finished*.

**Not-readable and none-left are different answers, and a `|| echo 0` collapses
them into the more flattering one.** This is the same family as the two other
defects this project has found in its own checks — a `chmod 000` test that passed
because it ran as root, and a `grep -v '^./…'` exclusion that matched nothing on
BSD grep. In every case a check reported success while measuring nothing.

Two rules, and the second is the one that made this worse than an error:

1. **A sentinel for "I cannot see" must be distinct from every real value.**
   The count is now `blind` or a number, and `blind` says so loudly.
2. **A watchdog does not exit on a read failure.** The map may be mid-move, a
   filesystem may be briefly unavailable — quitting converts a transient blind
   spot into a permanent one, silently. It logs, waits, and keeps watching.

**And the operational note that produced it:** a long-running monitor holds the
script it started with. Changing a script under a live watch does not update the
watch. After a change that moves anything a monitor reads, **stop and re-arm
every monitor** — the file on disk being correct is not the same as the running
watch being correct.
