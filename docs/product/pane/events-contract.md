# pane — the events contract

Unblocks **61G**. What an event is, how a window of them becomes one batch, what a background job
and a standing handler are, and how one session sends to another. The runtime side of every name is
`runtime-contract.md`, the bytes are `model-contract.md`'s, and §10 shows one window in four views;
the three must agree on every name and number. Every sentence is a decision. §1's dedup keys restate
the hook protocol's `task_id + event_id + normalized event` in product terms on purpose:
`scripts/check-doc-boundary.sh` forbids shipped code citing a process document, so the crate cites
this section.

## 1. The event

Five fields, and no sixth:

| field | type | meaning |
|---|---|---|
| `kind` | one of the nine below | what happened |
| `source` | string | `worker/<name>`, `session/<id>`, `github/<workflow>#<run>`, `bg/<handle>`, `pane` |
| `at` | ISO-8601 UTC, ms | when the runtime **accepted** it, never when the source claims it happened |
| `payload` | handle | materialised on first access, in `runtime-contract.md` §2's terms |
| `priority` | `batch` or `interrupt` | exactly two values; §2 says which kinds are which |

The nine kinds, each with the **dedup key** deciding whether a second arrival is a second event:

| kind | dedup key | |
|---|---|---|
| `worker.report` | `source + path + mtime` | a rewritten report is a new event |
| `worker.quiet` | `source + quiet-since` | one per transition into quiet, never one per poll |
| `prompt` | `source` | a session waiting on a permission prompt; the latest question stands |
| `ci.run` | `source + conclusion` | |
| `ci.cell` | `source + cell + conclusion` | a cell landing twice is one event |
| `bg.done` | `bg/<handle> + emission` | a job completes once; a watch emits one per match |
| `hook.<name>` | `source + hook + tool call id` | thirty tool calls are thirty events |
| `message` | `sender + message id` | |
| `timer` | `bg/<handle> + deadline` | the only kind pane itself sources |

Three rules the keys may not break. **A still-running tick is not an event**: polling raises
nothing, only a transition does, and no kind means "unchanged". **Dedup drops the later arrival**,
so the first event of a key stands with its original `at`; the count is not a field because the
rollout records every arrival, dropped ones included. **`worker.quiet` never displaces
`worker.report`** — with both in one window the quiet is dropped: idle is not finished, and the
reverse reading loses work.

## 2. The window

One window is always open: it opens when the previous batch was delivered, or at session start, and
closes on the earlier of **2,000 ms after its first event** — measured from the first, not the last,
because a debounce that restarts on every arrival never closes under a storm — or **an `interrupt`
event**, at once.

Three kinds are `interrupt` by default and no others: **`prompt`** (a session blocked, no token
moving), **`ci.run` with a failing conclusion**, **`message` marked interrupt by its sender**.
`pane.toml`'s `[events] interrupt = [...]` replaces that list wholesale; a kind in it that §1 does
not enumerate is a startup error, never a silent ignore.

**An interrupt does not get its own turn.** It closes the window and travels at the top of the batch
it closed. There is no delivery path that is not a batch.

The batch cap is **200**. A wider window fills the batch oldest-first to the cap and spills the rest
into the next window, still oldest-first, so order survives a storm; an `interrupt` is exempt.

## 3. The batch is a handle

The batch arrives as one handle named `batch`, type `Events.Batch`. Its preview, in this order and
no other: every `interrupt` in full — kind, source, time and the one summary line the *producer*
wrote; then counts by kind, descending; then **5** samples of the rest, one per kind **rarest kind
first** and cycling while slots remain, so a thirty-event storm cannot fill every slot; then `… and
K more`. **A payload is never previewed** — forty would breach `runtime-contract.md` §3's 256-token
cap on the first storm — and the preview shrinks by that section's rule, five → 2 → 0. Three
methods, called from the model's own program:

    batch.where({kind, source})   // both optional; kind matches `hook.*` by prefix
    batch.ack(ids)                // ids the model itself named
    batch.rest()                  // this batch's events not yet acked

**Unacked events roll into the next batch** carrying an `age`, the number of batches they have
appeared in, `0` on first delivery. At **age 4** an unacked event is dropped: it leaves the inbox,
the rollout records `event.dropped` with its id, kind and source, the next preview carries one line
`… and K dropped unacked (see rollout)`, and touching that payload throws `PayloadDropped(id)`.
Rolled events never occupy more than **half** the cap, so a backlog cannot starve what the model waits
for; a fully acked batch is freed with the cell.

## 4. Delivery

The batch extends `model-contract.md` §4's handle table with exactly one row, named `batch`,
**always last**, so the model's own bindings keep the order it made them in. It obeys
`runtime-contract.md` §2's replacement rule — each delivery replaces and frees the previous batch,
rendered `batch  (replaced at cell 5)` — and it is the **one** binding the runtime declares in the
model's scope, the only exemption from §2's naming rule, which no model can satisfy for an object
that did not exist when it last wrote a program.

**A turn with an empty batch and no user input does not happen**: the runtime waits.

## 5. Background jobs

    bg.run(cmd, {cwd, env, timeout})    → Job handle, immediately
    bg.watch(cmd, {every, until})       → Watch handle, immediately
    bg.cancel(handle)                   → void, idempotent

`bg.run` returns before the process has done anything: the model never blocks on output and never
polls. On exit a `bg.done` event carries a payload handle whose `stdout`, `stderr` and `status` are
themselves handles, so a job that printed 40 MB costs a status line and nothing else. `bg.watch`
runs `cmd` every `every` ms and emits one `bg.done` per **match**, sourced `bg/<handle>`, until
`until` matches or it is cancelled; the deadline expiring emits a `timer`, and identical output in
one window is one event by §1. `bg.cancel` sends `SIGTERM`, then `SIGKILL` after 5 s; a cancelled or
timed-out job still emits `bg.done` with `status: "cancelled"`, so nothing waits for a dead result.

**A background job runs under the same sandbox grant and budget as a foreground turn**, so nothing
widens because it is asynchronous: a `bg.run` outside the grant throws `PermissionDenied` at the
call, before any handle exists, as `sandbox-grants.md` §5's refusal does.

## 6. Standing handlers

    on(pattern, program)   → Handler handle
    off(handle)            → void

`on` registers a program the runtime runs against every future batch matching `pattern` — §3's
`{kind, source}` shape — **before** the batch reaches the model. Events the handler acks never reach
the model's batch: routine noise is drained by a program the model wrote once.

- Shown in `/handlers` and in the sidebar count; `/handlers off <name>` or `off(handle)` cancels it.
- **A handler cannot register a handler**: `on()` inside one throws `HandlerNesting`, because a tree
  of handlers is a control flow nobody can read.
- A handler runs under the **same grant and budget as a turn**, with the same per-cell timeout; its
  run is one rollout line with a `handler` field and does not count against the cell cap.
- **A handler that throws is disabled, not retried.** Its handle goes `stale`, the next preview
  carries `handler <name> disabled: <class>`, `/handlers` shows the error, and retrying a
  half-processed batch would re-run side effects the runtime cannot know are idempotent.

## 7. Messaging

    send(session, message)   → void on success, throws on failure

Every session has an inbox: the message lands as a `message` event in the recipient's **next
batch**, carrying the sender's id. The guarantee is *once, or an error to the sender* — at most
once, never retried, and a `send` that throws delivered nothing.

**Inside Glasshouse this rides what Phase 12 and Phase 13 ship, and adds no transport.** Outbound is
`Request::SendMessage` (`crates/glasshouse/src/api/protocol.rs:220`), dispatched at
`crates/glasshouse/src/api/unix/mod.rs:642`; its `origin` defaults to `RequestOrigin::Machine`,
which is what a `send` from a program is, and the sender's session id rides the envelope, not
`origin`. Inbound is the lifecycle bus — `EventBus::publish` and `EventBus::subscribe`
(`crates/glasshouse/src/events/bus.rs:261`, `:314`) in process, and `Request::Events`
(`crates/glasshouse/src/api/protocol.rs:398`) across the socket, whose `after`/`head` cursor and
1,000-event ceiling (`crates/glasshouse/src/api/unix/events.rs:20`) are pane's inbox cursor.

A pane session addressed by `send` receives a `message` event; a **non-pane** Glasshouse session
receives a line of text, because that is what `Request::SendMessage` has always done — `send` is a
strict superset of an existing verb, not a second messaging system. **Standalone**, `session` names
a socket: `pane.sock` in the project's own state directory, beside Glasshouse's `control.sock`
(`crates/glasshouse/src/api/unix/mod.rs:68`), under the same 90-byte path bound (`:78`) so a long
project id cannot bind with `ENAMETOOLONG` after the session looks started.

## 8. Hooks

**Batch delivery fires no hook.** Hooks are per tool call; a batch is input, not a call. A hook able
to block delivery could stop the model from ever learning an interrupt happened, and that an
interrupt is seen is the one thing this contract owes. **A standing handler's tool calls fire
`PreToolUse` and `PostToolUse` exactly as a turn's do**: a hook firing for a turn's `write` and not
a handler's would state which code path ran rather than what happened to the project.

## 9. The sidebar line

    inbox 7 · batches 12 · handlers 2

| figure | what it counts |
|---|---|
| `inbox` | what the **next** batch would carry — the open window's events plus everything rolled forward unacked. Not a lifetime total; zero after a delivered batch is fully acked. |
| `batches` | batches delivered **to the model** this task. One a handler drained entirely is not one; `/handlers` counts those. |
| `handlers` | registered and not disabled. A disabled handler is excluded here and shown with its error by `/handlers`. |

## 10. The worked turn, in four views

An orchestrator session, four workers, one window: **40 events** — thirty `hook.PostToolUse` from
one worker's loop, four `message`, three `ci.cell`, two `worker.report`, one failing `ci.run` that
closes the window at 1,204 ms.

**View 1 — the runtime's handle.**

```
batch  Events.Batch  n=40 · window closed on interrupt after 1,204 ms
  !  ci.run   github/ci-extended#4471  16:58:41.204Z  "failure — 1 of 12 cells failed"
     hook.PostToolUse  30   worker/api-routing
     message            4   session/glasshouse-9c, session/pane-spec
     ci.cell            3   github/ci-extended#4471
     worker.report      2   worker/pane-events, worker/board-watch
  [1] worker.report     worker/pane-events   16:58:39.881Z  "report-pane-events.md"
  [2] ci.cell           github/ci-extended#4471  16:58:36.744Z  "ubuntu-24.04 · 1.90.0 — success"
  [3] message           session/glasshouse-9c  16:58:33.402Z  "61G drafted; the primary appends it"
  [4] hook.PostToolUse  worker/api-routing   16:58:31.006Z  "Edit routing/mod.rs"
  [5] worker.report     worker/board-watch   16:58:40.117Z  "report-board-watch.md"
  … and 34 more                                                   preview 191 tok
```

**View 2 — the bytes the model receives**, in `model-contract.md` §6's shape:

    [cell 4 yielded in 41 ms]

    ## Handles
    hits   Grep.Match[]  n=1195  …           ← the model's own bindings first
    batch  Events.Batch  n=40 · window closed on interrupt after 1,204 ms
      …the eleven lines of view 1, verbatim…

    ## Budget
    turn cap 8,000 · task 11,208/400,000 · cells 4/40

**View 3 — the program the model returns.**

```pane
const ci    = batch.where({ kind: "ci.run" })[0];
const noise = batch.where({ kind: "hook.PostToolUse" })
                   .concat(batch.where({ kind: "ci.cell" }), batch.where({ kind: "message" }));

const cell = ci.payload.failing_cell;    // "windows-11-arm · 1.90.0"
batch.ack(noise.map(e => e.id));
batch.ack([ci.id]);
```

Thirty-eight acked. The two `worker.report` events are never named, so they stay unacked: reading
them is the next turn's work.

**View 4 — the state after `ack`, and the next batch.**

```
batch  Events.Batch  n=40 · 38 acked · 2 rolling
  [1] worker.report  worker/pane-events  16:58:39.881Z  age 0  "report-pane-events.md"
  [2] worker.report  worker/board-watch  16:58:40.117Z  age 0  "report-board-watch.md"

… the next window closes on its 2,000 ms deadline with five new events, and the two roll on:

batch  Events.Batch  n=7 · 2 rolled (age 1) · window closed after 2,000 ms
```

One payload was read; forty events cost the model 191 tokens to know about.

## 11. What this contract does not decide

- **2,000 ms, 200 and age 4 as tunables** — the defaults; `pane.toml` and who changes them is 61F's.
- **What a `worker.report` payload contains** — the producer writes it and its summary line.
- **A handler surviving `pane resume`** — it does not; handlers are task-scoped, and the day one must
  is a new line inheriting `runtime-contract.md` §4's staleness question.
- **Cross-project addressing** — `send` reaches this project's sessions; another project's is a
  grant question `sandbox-grants.md` §4 has not been asked.

CONTRACT
behaviour:  Events raised while no turn is running accumulate in one open window and reach the model as a single deduplicated batch handle named `batch`, whose payloads are handles and whose preview lists every interrupt in full before counts by kind.
invariant:  No event is delivered as its own turn — an `interrupt` closes the open window and travels at the top of the batch it closed — and an unacked event rolls forward with an `age` until it is acked or dropped at age 4 and logged, never silently lost.
path:       `crates/pane/src/events/`: the window accumulator applying §1's dedup keys, the batch renderer binding `batch` as the handle table's last row, and the background-job and handler supervisors that share a turn's grant and budget.
test:       `crates/pane/tests/events.rs::forty_events_in_one_window_are_one_batch_with_the_interrupt_first` — replays §10's forty events, asserts one batch handle, a preview under 256 tokens listing the `ci.run` first and reporting 34 more, and that the two unacked reports return at `age 1` in the next batch.
