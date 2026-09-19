# pane — watching a session

What a program reads to see what a session is doing **now**, and the
vocabulary a hook mechanism will later be handed. Every sentence is a
decision.

## 1. Why this is not the rollout

`rollout.jsonl` is complete and it is late. Its `cell` line is written when
the cell has finished and its `view` line when the evidence is settled, so a
reader can say what a session *did* and can never say what it is *doing* — it
cannot tell a cell that is working from one that has hung, which is the one
question an observer has.

The stream answers that and nothing else. One JSON object per line, appended
and flushed as each transition happens.

**Payloads stay out.** A diff, a file's bytes, a command's stdout are in the
rollout, and `session.begin` carries its path so a reader that wants them
knows where to look. The one exception is `cell.submit`'s `program`, and it
earns the exception in §4.

## 2. Where it is, and whether it is on

Beside the rollout, sharing its stem: `rollout.jsonl` is watched through
`rollout.events.jsonl`. One file per session, because the rollout is one file
per session and a reader that has found either has found the other by changing
the extension — no registry, no lookup, no guessing.

**On by default.** An observability surface nobody knows to ask for is one
nobody uses, and the cost is one short append per transition against a rollout
that is far larger.

**It can never fail a session.** A destination that cannot be opened leaves
the session unwatched and running; a write that fails is dropped. This is the
same rule `Rollout::record_moves` and `glasshouse::emit_lifecycle` already
keep, and the latter writes down why: *a harness that fails a turn because a
hook could not be delivered is worse than one that runs without telemetry.* A
full disk stops the watching, never the work.

## 3. The envelope

Four fields on every line, then the kind's own.

| field | meaning |
|---|---|
| `kind` | one of §4's, always `noun.verb` |
| `at` | ISO-8601 UTC, ms — when the runtime **accepted** the transition |
| `source` | `session/<id>` |
| `span` | the correlating id |

`kind`, `at` and `source` are `events-contract.md` §1's fields with §1's
meanings, including its clock rule. That contract's `payload` and `priority`
are deliberately absent: they belong to inbound events a session *consumes*,
and these are outbound facts a session *produces*.

**`source` is the envelope's and nothing may displace it.** A cell's text is
spelled `program` for exactly this reason — written as `source` it silently
replaced the origin of the event, and a test now pins that it does not.

A **span** opens and closes, its halves sharing one `span`: `session`, `task`,
`turn`, `cell`, `command`, `helper`, `agent`. A **moment** happens once and
closes nothing. A reader takes a duration by pairing, never by parsing prose.

## 4. The kinds

### The cell, where the power is

| kind | carries | refusable |
|---|---|---|
| `cell.submit` | `program`, `commands`, `cell`, `description` | **yes** |
| `cell.repair` | the parse error and the amendment | **yes** |
| `cell.end` | `outcome`, `calls` | no |

`cell.submit` is the seam. The program is parsed, nothing has run, and the
command lines `runtime/commands.rs` has proved are *certain* in that source
are already known. **The program is carried whole** because a reader that may
refuse a program cannot judge it from a summary; it is cut at 2,048 bytes like
any other value.

`cell.submit`'s `cell` is the rollout's own cell number, so correlating the
two files needs no translation.

**Why this beats a per-call seam, and it is not only about cost.** On a
program that makes twenty calls this is one question where a per-call harness
asks nineteen — but the saving is the smaller half. A reader that sees the
whole program sees the *intent*; one that sees a single call sees a step and
can never see where it is going.

### Inside a cell

| kind | carries | refusable |
|---|---|---|
| `command.judge` | `tool`, `arguments` | **yes** |
| `command.end` | `ended`, `exit_code` | no |
| `file.change` | the path | no |
| `helper.begin` / `helper.end` | the helper's name | no |
| `agent.begin` / `agent.end` | the subagent | `begin` **yes** |

`command.judge` is the one call-level event, and it is here for the reason
`commands.rs` states about itself: a line assembled from a variable, a call or
a template is not knowable at submit time, because *"reporting a guess here
would pre-answer a question about a line that never runs."* So that seam has
to exist.

`command.end` is **not** in the approved vocabulary and was added
deliberately: `command.judge` alone is a decision with no outcome, and an
observer that sees only the seam cannot tell a running command from a hung
one, which is the question this file exists to answer.

### What pane has that a per-call harness does not

| kind | carries | refusable |
|---|---|---|
| `answer.propose` | the text that would end the task | **yes** |
| `ask.raise` | the question put to the person | **yes** |
| `supervisor.look` | the verdict | no |
| `ladder.move` | `from`, `to` | **yes** |
| `reduction.made` | what was reduced and by how much | no |

`answer.propose` fires when `answer(text)` has been called and the task has
**not yet** ended. The CHECKER helper occupies that seam model-side today; a
hook is the same seam for the person's own rules.

### The session

| kind | carries |
|---|---|
| `session.begin` | `rollout`, `root` |
| `session.end` | |
| `task.begin` | `task`, `mode`, `rung`, `model` |
| `task.end` | `reason` |
| `turn.begin` / `turn.end` | `turn` |
| `approval.raise` | what is being confirmed |

`session.begin` carries **no model**: `/model` can change it mid-session, so
one recorded at the top would be a fact with a shelf life. `task.begin`
carries the model actually about to be asked.

`task.end` carries **no cell count**. The seam does not hold one, and a zero
written there would be a lie a reader could not tell from a task that truly
ran nothing; counting `cell.end` lines gives the real figure.

`session.begin` is written *beside* the existing `glasshouse hook --event
SessionStart` and not instead of it. That one runs a command and is silent
when the binary is absent; this one is a line in a file that needs nothing
installed.

## 5. Redaction

An argument value carrying a known provider-key prefix whose tail clears
`scripts/check-secrets.py`'s entropy bar is replaced with `«redacted»`. Both
halves are required: `sk-proj-example` is a placeholder, and redacting it
would teach a reader that redaction is noise.

Keys are never redacted, only values. A value longer than 2,048 bytes is cut
on a character boundary and says its real size, so a truncated value is never
mistaken for a complete one.

This file is readable by anything that can read the directory. It carries
command lines by design — that is what makes the seam useful — so a session
whose commands carry secrets in argv puts them here, exactly as it already
puts them in the rollout.

## 6. What a hook would still need

The stream is the vocabulary; a hook is the synchronous half, and **none of it
is built**. What is missing is only the answering:

- **A way to be asked rather than told.** Nothing here returns anything. The
  seven refusable kinds are marked (`Kind::decides`) so the set is a decision
  and not a slip, but no caller waits for a verdict.
- **Where a hook is registered**, and the rule that an unregistered event
  costs nothing. A spawn is paid only when a hook exists for that kind *and*
  the kind fires.
- **A ceiling that measures inactivity, not duration.** A blocking hook costs
  a process spawn per firing and one that hangs stalls the session. When that
  ceiling is designed it must reset on output rather than cap total time —
  `wire.rs`'s `send_errand_streaming` learned this the expensive way, after a
  reasoning model was killed at exactly 120 seconds for thinking.
- **`approval.rs` already suspends a callback while a person decides**, so the
  mechanics of pausing a live call are built and tested; `permissions::judge`
  already takes a `CommandJudge` consulted before a command runs, and a
  `command.judge` hook is a second reader at that same seam.
