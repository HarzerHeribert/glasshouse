# pane — the ruler

Unblocks **61A**, and 61A is first for a reason: a harness with no ruler is a
preference. This document decides the task set, the score, and the command.

## 1. The tiers are the router's own, and there are three of them

`crate::routing::classify::WorkloadTier`
(`crates/glasshouse/src/routing/classify.rs:88`) has five variants. Two —
`Deterministic` (Tier 0) and `Frontier` (Tier 4) — have **no producer**;
the type's own doc comment says so at lines 80–86, and this project adds a
variant when its producer lands. A ruler that scored a tier nothing can
classify into would be scoring an empty set.

So the ruler measures **Leaf, Standard and Heavy**, in the router's own
vocabulary, and gains a tier the day a producer for one of the other two
exists. Reusing the enum rather than inventing a scale is deliberate: a win
the ruler reports on `Heavy` is a win on the tier the router already routes
by, and the two can be read against each other.

## 2. A task is a commit of this repository

**Rule.** A task is a commit that closed a capability-map line or fixed a
defect, and it consists of four things:

1. the commit's **parent**, which is the tree the harness starts from;
2. a **statement**, derived from the commit's subject line and the map line it
   names — never from its diff. A harness handed the diff is being graded on
   transcription;
3. the commit's own **test command**, which the ruler runs;
4. its tier.

Using this repository's own history is what makes the tasks real work with a
real bar, and it is what makes the bar unarguable: the commit passed the gate
on the day it landed.

### Tier 1 — Leaf

| id | commit | statement source | test |
|---|---|---|---|
| `L1` | `fa66efc` | a `std::fs` import used only by the unix permission tests breaks the Windows build after a split | `cargo build -p glasshouse --tests` |
| `L2` | `ca18723` | a test pins that a referenced file cannot be stored; line 1139's producer has landed | `--test memory_file_observer` |
| `L3` | `9c1b0a5` | re-read Codex's hook catalogue from the installed 0.153.3 and make every declaration match | `--lib harness::codex` |
| `L4` | `ad2e8f5` | the `1836` line must print after `served:`; the view tests read each account's block by position | `--test entitlement_broker` |

### Tier 2 — Standard

| id | commit | statement source | test |
|---|---|---|---|
| `S1` | `e9178c0` | a verbatim (`\\?\`) project root refuses every path inside it on Windows | `--test project_isolation`, `--lib commands::context_firewall` |
| `S2` | `045c71d` | the relay's gzip limit differs per harness — Codex always populated, Claude Code conditional | `--test relay_usage` |
| `S3` | `09e6ae9` | map lines 2409–2410: predict a conflict, and name the distinction rather than implying it | `--test orchestrator_conflict` |
| `S4` | `2bdbbc5` | the reranking tripwire fired as designed; invert it into a four-caller census and close 1625 | `--test memory_reranker` |

### Tier 3 — Heavy

| id | commit | statement source | test |
|---|---|---|---|
| `H1` | `a61ba99` | the database bootstrap straggler waits on a timer and fails every module-level run under load | `--test project_isolation`, `--lib database` |
| `H2` | `26fb65b` | create the project database privately and publish it with one hard link; the race's successor | `--lib database`, `--test memory_store` |
| `H3` | `ee7799b` | `main.rs` is over the size ratchet; split it into `commands/` with every import path kept valid | `scripts/blast-radius.sh` |
| `H4` | `f2883ca` | map lines 2402–2405: edit intent, and 2392 finally gets a producer | `--test edit_intent`, `--test file_claims` |

Twelve tasks, three tiers, every one of them a commit in this repository's
history with a test that passed on the day it landed.

## 3. The score

Three numbers per (task, harness) pair, and one of them is the headline.

**Tokens per completed task.** The sum of provider-reported input, output and
cache-read tokens across every request of every **passing** attempt, divided
by the number of passing attempts. Tokens spent on attempts that failed are
reported in their own column and **never folded into the headline** — a
harness that gives up early must not be able to buy a good score with it.

The counts come from **one meter**: both harnesses run against the same
Glasshouse gateway, and the figures are the gateway's own recorded exchange
tokens, not either harness's self-report. The gateway records them for a
relayed exchange as well as a translated one — the user's ruling of
2026-09-03, implemented at `crates/glasshouse/src/gateway/ingress.rs:127–133`
— which is exactly what makes a same-meter comparison possible without asking
Claude Code to tell the truth about itself.

**Wall-clock.** From the first request leaving the gateway to the task's own
test command exiting. Includes the harness's thinking, its tool calls and its
test runs, because all three are time the user waits.

**Outcome.** The task's own test command's exit status, run **by the ruler**
on the harness's tree after the harness stops. A harness that reports success
and fails the test scores `fail`. There is no partial credit and no rubric.

### What the ruler will not print

There is **no tokens-per-turn column, and no flag that produces one** (61A's
third line). Tokens per turn falls as a harness takes more turns, so it
rewards exactly the behaviour that makes a task expensive. The number is not
computed and then hidden; it is not computed.

### Per tier, always

Every report carries a row per tier before the aggregate, and the aggregate
never replaces them (61A's second line). A harness that wins Heavy and loses
Leaf has a real result, and an aggregate that averages it away has destroyed
the only interesting thing the run found.

## 4. The command

    pane ruler run \
      --task L1 --task S1 --task H1 \
      --harness claude-code --harness pane \
      --repeat 3 \
      --gateway http://127.0.0.1:8731 \
      --out ruler/2026-09-05/

`--task all` runs the twelve. `--tier heavy` runs one tier. Each attempt gets
a fresh `git worktree` at the task's parent commit, removed afterwards; a
harness never sees another attempt's tree.

It prints two rows per task per harness, and one block per tier:

    task  harness      outcome  tokens/completed  wall     turns  tokens(failed)
    ----------------------------------------------------------------------------
    L1    claude-code  3/3 pass           18,204   4m12s      9        —
    L1    pane         3/3 pass            6,881   2m48s      4        —
    S1    claude-code  2/3 pass           94,110  22m40s     31   41,802
    S1    pane         3/3 pass           38,455  14m02s     12        —
    ----------------------------------------------------------------------------
    tier leaf      claude-code 19,340 · pane  7,102   (pane 2.7× cheaper)
    tier standard  claude-code 88,206 · pane 36,918   (pane 2.4× cheaper)

`--out` writes one JSON line per attempt — task, harness, commit, attempt,
outcome, the gateway's token figures, wall-clock, turn count, and the test
command's exit status — so a later run can be diffed against an earlier one
without re-reading a table.

**`--repeat 3` is the default and the minimum.** A single attempt of an agent
task measures the sample, not the harness.

## 5. Two ways this measurement can lie, and what the ruler does about each

**A task whose statement leaks its answer.** The statements above are derived
from commit subjects, and a subject like *"split `main.rs` into `commands/`"*
names the destination. That is the correct amount of leakage — it is what a
person would say — but it must be *equal* across harnesses, so the statement
is a fixed string in the task file and neither harness gets a word the other
does not.

**A test that the harness can satisfy without doing the work.** `H3`'s test is
`scripts/blast-radius.sh`, which a harness could pass by deleting code. Every
task therefore carries the commit's own `--shortstat` as a **sanity bound**,
and an attempt whose diff is under a tenth of it is reported `pass (suspect)`
with the figure, not silently counted. The ruler does not judge the diff; it
shows the number that makes a human look.

## 6. What this does not decide

- **Which baseline harness beyond Claude Code.** Codex, Cursor and OpenCode
  are all reachable through the same gateway and the command already takes
  `--harness` more than twice; which ones a report includes is the report's
  call.
- **A pass/fail bar for pane.** 61E's line asks for a measured win on at least
  one tier *or a recorded reason why not*. This document decides how to
  measure, not what result is acceptable.
- **Anthropic's programmatic tool calling as a third arm.** It is a useful
  baseline and it is provider-specific; adding it is a task-file entry, not a
  change here.

CONTRACT
behaviour:  `pane ruler run` executes a fixed twelve-task set — four Leaf, four Standard, four Heavy, each a commit of this repository — through two or more harnesses against one gateway, and prints tokens per completed task, wall-clock and outcome per tier and in aggregate.
invariant:  Token figures come from the gateway's own recorded exchanges rather than any harness's self-report, failed attempts' tokens are never folded into the headline, and no tokens-per-turn figure is computed or printed.
path:       `crates/pane/src/ruler/`: the task file, a worktree-per-attempt runner, a reader of the gateway's exchange rows, and one table printer.
test:       `crates/pane/tests/ruler.rs::the_score_excludes_failed_attempts_and_names_the_tier` — a fixture run with one passing and one failing attempt asserts the headline counts only the passing tokens, the failed column carries the rest, and the per-tier rows survive into the aggregate output.
