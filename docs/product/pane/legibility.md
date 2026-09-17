# Pane's legibility: what the person watching can actually work out

The user, 2026-09-17: *"another agent should work on user facing verbosity and
information so a user can actually understand what's happening. Cells are pretty
tough to read."* And earlier: *"Claude code is verbal about what it is doing. A
model should deliver a descriptor for a cell to make user facing communication of
inner working and thought process visible."*

**The corpus is one real session**, not a hypothesis:
`.worktrees/pane-dogfood/.pane/sessions/tlitep-13fv.jsonl` — 120 cells, 247
turns, timestamps spanning 20:16:12–20:37:44 (model time inside a much longer
sitting), implementing `ssh.run`. Outcomes: 65 returned, 40 yielded, 15 threw.
Calls: `context` 58, `rg` 39, `read` 33, `bash` 18, `edit` 14, `fd` 4,
`agent.run` 4, `write` 2.

**The single measurement this document exists for.** Across 123 assistant turns
the model emitted **121 `tool_use` blocks and 2 text blocks**, both in the last
two turns. For 120 cells the person was shown TypeScript and a handle table and
**not one sentence of English**. Pane is not under-designed here; it is silent by
construction.

Two decisions are already taken and this builds on them: the per-cell
`description` in `.agent-runtime/next-packages.md` §1 (one required line on the
native `execute_cell` call, surviving `prompt::compact_result`), and
`.agent-runtime/limits-inventory.md`'s rule that no cap drops something silently.

---

## 1. What the person sees today

The default screen is compact (`session.rs:865` sets `compact: true`;
`tui.rs:2001` is the branch). Everything below is the compact path.

**An ordinary cell — cell 1, two reads.** The model's message *is* the program
(`const design = await read({path: "docs/product/pane/network-tools.md"}); …`),
and the screen renders, in order: `✓ Cell executed  · 1`, the recorded calls
`├─ read network-tools.md · returned / └─ read architecture.md · returned`, and
`Ctrl-O · code and results · /cell 1 or click this header` (`tui.rs:2030`,
`:2064`, `:2081`). **Workable:** two files were read. **Not workable:** why, what
was looked for, or that a four-item plan was written in the same cell — `todo`
produces no visible line at all.

**A cell that threw — cell 2.** The screen gets one line (`tui.rs:2126`: the
class, then `error.message.lines().next()`): ``ToolError: `rg` failed with exit
2: rg: …/crates/pane/src/config: IO error for operation on …``. **Not workable:**
that two of the three searches succeeded and their results were discarded with
the third; that the cause is a path that does not exist. The `line: 5, column: 2`
the runtime holds is dropped in compact (`push_error_region`, `tui.rs:2550`).

**A cell that delegated — cell 13.** `agent.run(…, {turns: 40, model:
"gpt-5.6-sol"})` plus `yieldNow("A focused implementation agent is editing the
SSH feature and its security probes; waiting for its report before
verification.")`. The screen shows `└─ agent.run agent/job1 · started`. **That
yield sentence — the one line the model wrote in the person's language — is drawn
only in the expanded path (`tui.rs:2282`).** 40 of 123 views carry a
`yield_reason`; the default screen shows none of them.

**A long cell — cell 69**, five calls including two `write`s creating
`crates/pane/src/ssh.rs` and `crates/pane/tests/ssh_escape_probes.rs`. The code
folds at 10 lines (`tui.rs:2218`): `… 118 more lines · Ctrl-O expands`. Its diff
*is* drawn (`push_changes`, 18 lines). But **112 of 123 views print `No observed
changes; capture incomplete.` and 119 carry the incomplete marker** — under a
header reading `CHANGES OBSERVED`. Files were written all session; the person was
told nothing changed.

**And the handle table.** `push_output_region` folds it to its **first six
lines** (`tui.rs:2573` → `push_folded_region(…, 6, compact)`), while
`render_table_delta` emits entries oldest-first (`handles.rs:363`), every
untouched handle a `name  Type  (unchanged since cell N)`. By cell 120 the table
is 136 lines of which **134 are `(unchanged since …)`; 9,450 such rows across the
session.** The six the person sees are the six *stalest* handles, and the value
the cell just produced sits behind `… 130 more lines`. The fold is backwards.

---

## 2. What the person needs, per cell

**The descriptor.** One line, present tense, first person, no handle names, no
tool names, no jargon; what this cell is *for*, not what it calls. Required and
bounded to a line (`next-packages.md` §1); absent, the fallback stays today's
first source line. Six, rewritten from the corpus:

| cell | what it did | descriptor |
|---|---|---|
| 1 | two reads + `todo.write` | Reading the ssh design and the architecture page to find what I have to change. |
| 2 | four `rg` over src/tests/config | Searching for the command-refusal grammar and the call record. |
| 13 | `agent.run` 40 turns | Handing the ssh implementation to a subagent and waiting for its report. |
| 16 | `batch.where`, no calls | Checking whether the ssh subagent has reported yet. |
| 69 | 2 edits, 2 writes, `bash` | Adding the ssh module and its escape probes, then compiling. |
| 110 | `rustfmt` + `cargo check` | Formatting the twelve changed files and type-checking the crate. |

**Default at rest: four lines per cell.**

```
▸ Reading the ssh design and the architecture page to find what I have to change.
  read · 2 files · 27 KB                                          [1]  1.4s
  nothing changed on disk
  ⏎ code · h handles · o expand
```

Line 1 is the descriptor, line 2 the verb line (§3), line 3 the outcome (§4),
omitted when there is nothing true to say; line 4 is the affordance row, drawn
**only on the focused cell** — so an unfocused cell at rest is three lines, and
one that changed nothing is two.

**Collapses by default:** the program, the handle table entirely, `stdout`, the
`Output` JSON, the per-call tree. **Only on request:** a handle's full value, a
subagent's own transcript, the raw throw with frames and position. A failed cell
is the one exception: never folded below the four lines of §5.

---

## 3. The verbs

Seven, fixed, in the person's language, derived from `CallRecord.tool`
(`runtime/outcome.rs:237`) — never from parsing the program, which is what
`possible_tool_calls` (`tui.rs:2394`) does today and why the screen can print
`◇ planned: batch.where → …` for a cell that ran nothing.

| verb | `tool` | line |
|---|---|---|
| read | `read`, `context`, excerpts | `read · 3 files · 41 KB` |
| search | `rg`, `fd`, `search` | `search · 4 patterns · 61 hits in 12 files` |
| edit | `edit`, `write` | `edit · 2 files · +214 −6` |
| run | `bash`, `checks.run` | `run · cargo check · exit 0 · 12.1s` |
| delegate | `agent.run` | `delegate · gpt-5.6-sol · running 4m12s` |
| wait | no calls + `yieldNow` | `wait · ssh subagent · 3m40s` |
| work out | no calls, no yield | `work out · no tools ran` |

**A cell doing several is summarised by ordering the verbs by weight — edit >
run > delegate > read > search > wait > work out — and naming the top two:**
`edit · 2 files · +214 −6 · then run cargo check · exit 0`. Never a count of
verbs, never "did 3 things". Cell 69's real line is
`edit · 4 files · +631 −2 · then run cargo fmt && cargo check · exit 0`.

**It cannot lie: every field comes from a recorded call.** A branch that did not
execute contributes nothing, and the speculative `◇ planned:` line is deleted.

---

## 4. What happened, in plain words

The outcome line is **derived in Rust from what the runtime already holds**:
`CallRecord` (`runtime/outcome.rs:237` — `tool`, `args` as checked, `exit_code`,
`ended`, `lifted_from`, `repeat_of`) and `Snapshot::changed_paths`
(`changes.rs:292`, `(PathBuf, ChangeKind)`). No second model call. Rules:

- **Disk.** `wrote crates/pane/src/ssh.rs (new, 148 lines) · changed
  crates/pane/src/lib.rs, sandbox/modes.rs`. Four or more: `wrote 2 files,
  changed 4 · o lists them`.
- **Nothing changed** is said only when the capture was complete. When it was not
  — 119 views in the corpus — the line names the file and the bound:
  `could not tell what changed: crates/pane/src/session.rs is over the 1 MiB
  capture limit`. Today's `No observed changes; capture incomplete.` under a
  header saying `CHANGES OBSERVED` is the worst sentence on the screen; it goes.
- **Found.** From the search's own result: `61 matches in 12 files, most in
  crates/pane/src/config.rs`. Zero is loud: `no matches`.
- **Failed.** §5.
- **Repeat.** `repeat_of` already records a byte-identical earlier call:
  `read crates/pane/src/config.rs — unchanged since cell 31`. The corpus read
  `config.rs` 16 times and `bindings.rs` 15; the person never saw that.

---

## 5. Failure

Fifteen throws in the corpus, in three families: `context` could not find a
symbol (7), a tool refused or found nothing (3), the cell hit the 30-second
wall clock (2), the program had a bug (2), an edit was a no-op (1). A throw gets
**four lines and no fold**:

```
× Could not read the symbol `Config` in crates/pane/src/config.rs — it is not
  a name in that file. Nothing was packed and the cell stopped here.
  2 of 3 calls had already succeeded; their results were discarded.
  ⏎ code (line 3) · o full error
```

Line 1 is the failure in English, per class: a `ReferenceError` says *the cell
used a name nothing defines*; a `RuntimeTimeout` says *the cell ran 30s and Pane
stopped it — `cargo check` does not fit in a cell, run it with `bg.run`* (hit
twice, at cells 110 and 120, the second being the session's last cell). Line 2
names what was lost, the thing a person cannot reconstruct: cell 2 threw on its
third `rg` and the first two searches' output vanished. Line 3 carries the
position the runtime already attributed (`CellError.line`, `tui.rs:683`) and
drops in compact today.

**The class name is never the first word.** `ToolError:` tells a person nothing;
it stays, one line down, behind `o`.

---

## 6. Progressive disclosure

Another package is making the TUI clickable (`tui/hit.rs`: a click is a second
route to an action a key already has). What must be reachable, with its twin:

| affordance | key | reveals |
|---|---|---|
| the descriptor line | `⏎` / click | the program, formatted, full |
| the verb line | `h` / click | the handle table for **this cell only** |
| a handle name in it | click | that handle's full value, sampled as `render_preview` samples it |
| the outcome line | `d` / click | the full diff, unfolded |
| `delegate` | `a` / click | the subagent's own session — it has a `.pane/sessions/*.jsonl` of its own |
| a failed cell | `o` / click | class, message, position, frames |
| the whole cell | `Ctrl-O`, `/cell <n>` | today's expanded view, unchanged |

`Hit::Cell(usize)` already exists; these are sub-regions of the same rectangle,
so the map grows rows, not concepts. **The expanded view is not redesigned** — it
is the escape hatch and should stay a faithful dump.

---

## 7. Long-running and quiet periods

The corpus's worst stretch: cells 14–18, 21–24, 29–35, 86–90 and 111–118 are
`batch.where` polls that ran **no tool at all** — 43 of 120 cells made zero calls
— each drawing its own block saying nothing. The screen grew by four lines every
twenty seconds while nothing happened.

**A run of consecutive waiting cells on the same source collapses into one live
line that updates in place:**

```
⏳ waiting for the ssh subagent · 4m12s · 9 checks · gpt-5.6-sol, up to 40 turns
```

Elapsed comes from the rollout's turn timestamps; the count is the number of
collapsed cells; a click expands them. **The same line serves a `bg.run` job**
(`bg.rs`), and resolves in place to §4's outcome line when the subagent reports.

While a cell is executing, the verb line is drawn as soon as its first call is
recorded, present tense: `reading crates/pane/src/config.rs…` — strictly better
than the activity ribbon, which says a model is thinking without saying of what.

---

## 8. What this must not become

1. **No second model call to explain the first.** Every line in §§3–5 is a
   projection of `CallRecord` and the change capture. A helper that narrates a
   cell is a second thing that can be wrong about the first, and it costs tokens
   on every cell.
2. **No narration that can drift from what ran.** The descriptor is the model's
   *intention*, written before execution, labelled as such by position: above the
   verb line, which is the record. When they disagree the verb line is the truth,
   and the supervisor already treats that gap as signal (`next-packages.md` §1).
   Delete `possible_tool_calls`' speculative line (`tui.rs:2394`, `:2073`) — it
   is narration inferred from source text.
3. **Nothing hides a refusal, a throw or a denial.** `Ended::Denied { rule }`
   and every throw are drawn unfolded, always, at every verbosity.
4. **No cap drops something silently** — the inventory's rule applied to the
   screen: a fold says how much it folded and which key opens it.
5. **No wall of prose replacing a wall of code.** Four lines per cell. A
   descriptor that needs two lines is a descriptor that is wrong.
6. **The handle table is demoted, not deleted.** It is how the model addresses
   values and it stays exactly as it is, behind `h`.

---

## 9. Five changes, most understanding per line of code first

1. **Draw the descriptor and the yield reason in the compact path.**
   `crates/pane/src/tui.rs` (the compact branch at `:2001`, which today ends at
   `continue` before `:2282` where `yield_reason` is drawn). The yield sentence
   already exists on 40 views and costs nothing to show; the descriptor lands in
   the same slot when `next-packages.md` §1 ships. Pinned by
   `crates/pane/tests/tui_look.rs`. **Cost:** ~20 lines; one more line per cell
   on screen.
2. **Reverse the handle fold, and scope it to this cell.**
   `tui.rs:2573` (`push_output_region` takes the first six lines) and
   `crates/pane/src/runtime/handles.rs:357` (`render_table_delta`, which knows
   exactly which entries are news — `changed_at_cell == current_cell`). Show the
   news, count the rest as `+134 live handles · h`. Pinned by `tui_look.rs` plus
   the handle tests. **Cost:** ~30 lines. Buys back six lines of pure noise on
   every cell after cell 10.
3. **The outcome line, derived.** A `fn outcome_line(record: &CellRecord,
   changed: &[(PathBuf, ChangeKind)]) -> String` beside the execution-tree
   builder at `crates/pane/src/session.rs:2196`, rendered by `tui.rs` in place of
   the call tree; and `crates/pane/src/changes.rs` stops printing
   `No observed changes` when the capture was incomplete, naming the file and the
   bound instead. Pinned by `tui_look.rs` (`local_file_diffs_are_visible_in_…`)
   and a `changes.rs` unit test. **Cost:** ~80 lines. Removes the screen's one
   false statement, made 112 times in the corpus.
4. **The human throw.** Replace the compact one-liner at `tui.rs:2126` and
   generalise `push_error_region` (`tui.rs:2550`) into a plain-English first line
   per error class plus the count of calls whose results were discarded (already
   in `record.calls`). Pinned by `tui_look.rs` and
   `crates/pane/tests/cell_inspection.rs`. **Cost:** ~60 lines and a sentence per
   class. Fifteen cells in the corpus; the two that ended the session were both
   the same timeout with no advice attached.
5. **Collapse a run of waiting cells into one live line.** `tui.rs`
   `notebook_lines`, keyed on consecutive cells with zero calls and a
   `yield_reason`, with elapsed from the rollout timestamps; the same line serves
   `bg.rs` jobs. Pinned by `tui_look.rs`. **Cost:** ~70 lines, and it is the only
   change here that alters the transcript's structure rather than a cell's
   contents — so it ships last. Buys back roughly 40 of the corpus's 120 cell
   blocks.
