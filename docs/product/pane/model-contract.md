# pane — the model contract

Unblocks **61C** and **61E**. The prompt schema, byte for byte: what pane
sends, in what order, with what delimiters, and what it accepts back. The
runtime side of every name used here is `runtime-contract.md`; §7 shows the
same worked turn from this side, and the two must agree.

## 1. The message layout

One Anthropic Messages request per turn. The base system block is sampled at task start and stays stable between
inferences, with the configured request model appended. Newly applicable
directory instructions are appended only at an execution boundary. A request-only
context block marks the current user request and its fresh runtime; the
user's saved text is unchanged. The provider can cache the stable prefix. The
conversation carries alternating assistant cells and runtime results.

    system    : preamble · tool declarations · scoped project instructions · environment
    user[0]   : the task
    assistant : native execute_cell tool_use (cell 1)
    user[1]   : correlated tool_result for cell 1
    assistant : native execute_cell tool_use (cell 2)
    …

A plain-language assistant response with no native call ends the request as its answer. Calls and results remain provider-native blocks with the original call id; the result is never duplicated into plain text.

## 2. The system preamble, verbatim

This is the currently shipped preamble. Phase 61H's normative migration table
is `improvement-register.md` under *Prompt boundary*: mechanically enforceable
tool and recovery advice moves to schemas, validators and one-turn diagnostics,
leaving only durable protocol invariants here. Until that implementation lands,
the byte-for-byte text below remains the compatibility contract.

    You are Pane, a coding assistant. Answer conversational questions naturally.
    To act with tools, make exactly one `execute_cell` call in an assistant turn.
    Put every operation in that one TypeScript program; `execute_cell` is the only
    provider-native tool. Runtime tools are callable only inside its code.
    While you construct the call, none of THIS cell has executed. Code may await
    tools and branch on their actual returned values. Batch deterministic work
    when useful; stop at the next decision that needs unseen evidence. After
    submitting a cell, wait for its correlated result. Never invent output or
    infer success: only that result is runtime evidence.

    A cell is a program, and that is what earns it a turn. One cell can read
    several files, search the tree, edit, run the tests and branch on what comes
    back: every call is awaited, and every result is a live value the next line
    uses. So spend the turn on a whole step — gather what the step needs, act on
    it, and check the result in the same program — then yield when the next
    decision needs evidence that does not exist yet.

      // one inspection cell: everything the next step is about to change
      const [limits, callback, hits] = await Promise.all([
        context({path: "src/config.rs", symbol: "Limits"}),
        context({path: "src/runtime/bindings.rs", symbol: "tool_callback"}),
        rg({pattern: "cell_wall_clock|response_bytes", path: "src"}),
      ]);
      return {omissions: limits.omissions, matched: hits.length};

      // the next cell: the edits those results earned, and the check for them
      await edit({path: "src/config.rs", old: OLD, replacement: REPLACEMENT});
      const run = await bash({command: "cargo test -p pane --lib config"});
      const failures = await helper.reduce(run.stdout);
      return {passed: run.exit_code === 0, failures};

      // judge what the cell already holds, and branch on it, in the same turn
      const diff = await bash({command: "git diff --stat"});
      const call = await decide.choice(
        "Does this diff do more than rename a symbol?",
        {rename_only: "every hunk renames one symbol", wider: "anything else"},
        diff.stdout);
      if (call.choice === "wider" && call.confidence > 0.85) { /* inspect */ }

    `helper.<name>` and `decide.choice` answer from inside the cell and cost no
    turn, so a summary or a judgement belongs in the step that needs it rather
    than in a turn of its own; the Runtime block below declares the ones this
    session has.

    Changing existing source has a rhythm worth knowing before you start: `edit`
    writes against the version `context` delivered in a previous completed cell.
    So fetch every symbol the step will change in one cell, and make all of those
    edits in the next — two turns for a batch of edits, rather than two turns for
    each one.

    Every cell carries a description: one short line, in the person's language,
    saying what it is for and why — not which functions it calls. It is the only
    account of your work the person sees while you run, and you read it back after
    compaction. Pass it as the `description` argument; in the fenced form it is the
    line immediately before the fence.

    A cell is validated before it runs. A parse error runs nothing and may offer
    `pane-edit`; a return, yield, or throw stops later code. Tool results are live
    objects, but unseen fields are not model-visible: use declared fields and
    standard JavaScript, and bind your own values to names no declared tool or
    host global already has. Reuse live handles rather than repeating a read.
    For an existing source change,
    `context({path, symbol})` is the first source-reading tool and delivers its
    complete target automatically; `read` and whole-file prints are for files the
    step is not about to edit. Then `edit({path, old, replacement})` — the second
    argument is `replacement`, since `new` is a JavaScript keyword — and Pane binds
    it to the latest observed source version. For file or script text containing
    `$`, quotes or heredocs, `write` and `edit` take line arrays:
    one double-quoted JavaScript string per logical line, and Pane supplies the
    separators. Prefer compact structured summaries or bounded excerpts to broad
    prints. `glob` may return directories, so select a file before `read`. A
    `bash` result succeeded only when its `exit_code` says so.

    Bindings persist between cells of this user request; redeclaring replaces
    them. Each new user request starts a fresh runtime. Earlier requests are
    history, not unfinished work. Work on the current request, including its
    requested tests. Running off the end, `yieldNow(reason)` or a top-level
    `return` all give results and another turn: returning a value displays it
    as notebook output and finishes nothing. Return whatever you want to look
    at, as often as you like. A returned object is shown field by field as
    text -- an excerpt as its lines, an array of strings one per line -- within
    the return budget the usage line names; a field over its share is paged at
    a line and ends in one cursor line saying how to read on. Return what you
    need to read next, not everything you hold.

    The task ends only where you say it ends.
    `answer(text)` inside a cell ends the task with that text.
    A prose response with no `execute_cell` call ends the task as the answer.
    Use either only when the request is finished and the answer is grounded in
    observed results; do not use prose to announce work you still intend to
    perform.
    To interpret a file, inspect and yield first, then answer from the feedback.

    A thrown error carries its position and completed bindings. Continue from
    that state; failed or skipped calls did not succeed. PermissionDenied is
    final: code cannot widen the session's sandbox grant.

### 2.0 Pushed blocks after the preamble

Two blocks may follow the project's instructions in the system block, both
derived before the first turn and both paid for once: the Scout's preflight
block (`## Scouting record`), and since 2026-09-14 the acceptance list
(`## Acceptance list`, `acceptance.rs`) — the request's own verifiable items,
which the completion is checked against. Neither changes a sentence of the
preamble.

### 2.1 Interface variants

`prompt::preamble_for(interface)` renders the block above for the interface the
request declares (`tool-abi.md` §3). `Cells` is the block verbatim and the
default since 2026-09-13 (a session without `--interface` shows exactly the
block in §2).

**What the 2026-09-13 ablation measured, and what it did not.** It scored
cells 12/12, hybrid 11/12 and tools 10/12 — under the preamble as it then
stood, whose only instruction to chain was one subordinate clause inside a
paragraph of prohibitions. Two sessions on 2026-09-17 then measured 1.6 and
1.98 tool calls per cell, with 20 of 120 cells making no call at all: a cell
was being used as a single tool call, in every mode. That ablation therefore
compared three interfaces through a prompt that taught none of them to chain,
and its margins say little about the interfaces themselves. Re-running it
against the preamble above is what would.

`Hybrid` and
`Tools` are the same constant with exactly the segments below replaced and
nothing else changed, so every shared sentence has one copy;
`prompt_bytes.rs::the_interface_variants_are_the_contracts_verbatim` pins each
indented block here to the rendered variant. No variant says or implies that
`execute_cell` is the only native tool or that runtime tools are callable only
inside a cell, because in hybrid mode neither is true.

**Hybrid.** The opening tool sentences (*To act with tools … callable only
inside its code.*) become:

    To act, call a familiar tool directly for one independent operation, or make
    exactly one `execute_cell` call for dependent, branching, looped or batched
    work. Inside a cell the same tools are typed async functions with the same
    arguments and results.

and the completion sentence (*A prose response with no `execute_cell` call …*)
becomes:

    A prose response with no tool call ends the task as the answer.

and the sentence about `helper.<name>` and `decide.choice` (*… the Runtime
block below declares the ones this session has.*) gains what a direct call
costs, becoming:

    `helper.<name>` and `decide.choice` answer from inside the cell and cost no
    turn, so a summary or a judgement belongs in the step that needs it rather
    than in a turn of its own; the Runtime block below declares the ones this
    session has. A direct call spends a whole turn on one operation, which suits
    an independent step whose result needs nothing further this turn; dependent,
    branching or repeated work is what a cell is for.

The chaining paragraph with its three worked cells, and the edit-rhythm
paragraph, are kept **verbatim** in `Hybrid`: a cell is where dependent,
branching or repeated work belongs in either mode, and the hybrid sentence
above is what tells the model when a direct call is the cheaper route.

**Tools.** The opening tool sentences become:

    To act, call the familiar tools directly; each call's result is runtime
    evidence.

The rest of the opening paragraph (*While you construct the call … runtime
evidence.*) becomes:

    Stop at the next decision that needs unseen evidence. After each call, wait
    for its correlated result. Never invent output or infer success: only that
    result is runtime evidence.

The chaining paragraph, the `helper`/`decide` sentence and the edit-rhythm
paragraph are **dropped entirely**, each with its trailing blank line: a
request that declares no `execute_cell` has no cell for a worked program to
fill, and the two host globals those examples call are bound only inside one.

The descriptor paragraph (*Every cell carries a description … before the
fence.*) is **dropped entirely**: a request that declares no `execute_cell`
runs no cell, so there is no call for the argument to sit on.

The second paragraph (*A cell is validated … `exit_code` says so.*) describes
cell mechanics a request without `execute_cell` cannot use, and becomes:

    Pane binds an edit to the latest observed source version. A command result
    succeeded only when its `exit_code` says so.

The completion sentence becomes the hybrid one:

    A prose response with no tool call ends the task as the answer.

The persistent-bindings and completion paragraphs stay in every variant: a
top-level returned string or a prose reply still ends the task.

## 3. Tool declarations are TypeScript, one line of prose each

Each tool renders as a `declare function` signature, one `//` doc line, and
one `// @callers` line. The signature is the contract; the prose never repeats
what the types already say.

    ## Tools

    declare function grep(a: {pattern: string; glob?: string; path?: string}): Promise<Grep.Match[]>;
    // Search the project for a regular expression. Pure: same tree, same result.
    // @callers program

    declare function read(a: {path: string}): Promise<File>;
    // Read one file inside the project. Pure.
    // @callers program

    declare function cargo_test(a: {target?: string; filter?: string}): Promise<TestReport>;
    // Run the project's tests. Not pure; spawns a process under the sandbox.
    // @callers program

`@callers` takes exactly two values, and it is Anthropic's `allowed_callers`
mapped honestly onto a harness with one action channel:

- **`program`** — pane's own isolate runs it. Every tool pane ships is this.
- **`provider`** — the provider's runtime runs it server-side; pane declares
  it so the model knows it exists and **never executes it itself**. The line
  exists for exactly one hazard: a gateway that fronts a provider with native
  programmatic tool calling would otherwise run the same call twice.

`Pure:` in the doc line is not decoration — it is the declaration
`runtime-contract.md` §4 relies on to re-materialise a handle after
`pane resume`, and it is the tool's own claim, never inferred.

### Bounded source inspection

`context({path, symbol?})` is the editing-oriented view. It returns the full
target definition (or a small file whole), a SHA-256 version, ranked imports,
nearby definitions, callers and tests, and explicit omissions. Its `text`
field is the same bounded, numbered rendering, automatically delivered once
with the correlated result before the model may decide an edit.
The rollout records the exact path, version and visible ranges as source
evidence; bytes kept only in a live object are never claimed as model-visible.
The automatic rendering shows only a 12-hex display version. The live object
and rollout retain the full SHA-256 used for stale-write enforcement.
A `read` of a large source file with exactly one unfinished definition is
promoted to this same context deterministically; ordinary and ambiguous files
retain normal `read` semantics.

`edit({path, expected_sha256?, old, replacement})` replaces one nonempty, exact,
uniquely matching string. It writes only when the current file still has the
version returned by `context` in a prior completed cell, or the version a
previous `edit`/`write` of the same path produced — Pane registers its own
mutations as visible, so only an external change is stale.
`edit({path, olds, replacements})` applies several hunks as one atomic
mutation: each `olds[i]` must match exactly once in the original text and the
matches must not overlap, or nothing is written and the error names the hunk. Pane supplies the hash
when exactly one complete version of that path is visible; the caller supplies
`expected_sha256` to disambiguate multiple versions. Stale, missing, ambiguous
and no-op requests throw without writing or retrying. `write` remains the
whole-file and new-file operation.

For literal multiline content, `write` also accepts `lines: string[]` instead
of `content`, and `edit` accepts `oldLines` and `replacementLines` instead of
their string forms. Each item is one logical line. Pane accepts and removes one
trailing `\n` or `\r\n` from an item, refuses an embedded line ending, joins
the items with `\n`, and adds one final newline; an empty array is an empty
file. Exactly one form must be given for each value. Double-quoted JavaScript
strings keep syntax such as `${BASH_SOURCE[0]}`, apostrophes and `$WORKTREE`
literal without a template literal; the string forms remain available when
byte-exact newline control matters.

The result of `read` has an `excerpt({start?, lines?})` method over its already
loaded `lines` array. It adds no filesystem access. `start` is one-based;
`lines` defaults to 400 and caps at 1,000. `console.log(file.excerpt(...).text)`
prints at most 24,576 Unicode scalar values, including the line range and next
options for the same File. Ordinary lines are paginated whole. A line too
large for one page is explicitly marked partial and names a bounded UTF-16
slice of the immediate continuation (at most 1,600 units); the source itself
remains intact in the handle. Metadata also supplies `start`, `end`,
`lineCount`, `next`, and `truncatedLines`. A File with no further lines has
`next: null`.

Console truncation retains a true suffix and says what was omitted. Both the
per-string bound and the final aggregate tail reserve space for their omission
marker; an excerpt fitting its documented character limit is preserved by the
canonical console call, including astral Unicode text. The console still has
a bounded transcript, so logging many excerpts in one cell may drop earlier
ones with an explicit notice.

## 4. The handle table

Rendered fresh every turn, after the tools and before usage. Empty on
turn 1 and written as `## Handles\n(none)`. Otherwise one entry per live
handle, in declaration order, in the shape `runtime-contract.md` §3 fixes.
The whole table is capped at 2,048 tokens; over it, the oldest entries are
dropped from the rendering with one line saying how many and how to list them.

Provenance is not shown in the table. A stale handle carries the one word
`stale` and nothing else; the model gets the recorded call in the
`StaleHandle` message if it touches one.

## 5. The native execution handoff

The primary action channel is the provider-native `execute_cell` tool. Its input schema is exactly `{code: string}`. The assistant's generation ends at the call; Pane executes only the complete, decoded input and returns a correlated `tool_result` before the model can interpret it. Malformed or truncated JSON never executes. Unknown or multiple calls remain explicit typed calls so the session can reject each without silently dropping it.

A provider response stopped at `max_tokens` is incomplete, including a
prose-only response. JSON and streaming transports fail the request explicitly
without automatic continuation or executing code from that response. Partial
prose is retained in the error diagnostic, with terminal controls escaped;
it is not recorded as a completed assistant turn or accepted as the final answer.

Legacy fenced `pane` blocks remain an input compatibility and repair path, not the advertised action channel:

    ```pane
    const report = await cargo_test({ target: "firewall_bridge" });
    ```

**Not `ts`.** A model writing about TypeScript emits ```` ```ts ```` blocks
constantly, and a parser that executed them would run the model's
explanations. The language inside is TypeScript; the tag names the channel.

**One or more complete `pane` blocks run in source order as one cell.** The
whole combined source is compiled and preflighted before any call starts.
A return, explicit yield, or uncaught error stops subsequent code. This is
not transactional rollback: effects completed before a runtime error remain.
Ordinary `bash`, `ts`, or other Markdown examples never execute. An unclosed
Pane block is invalid and runs nothing. Mixed/multiple repair blocks are
still ambiguous and run nothing. The parser bounds source size and block count.

A prose response with no native call is the final answer, matching ordinary
assistant tool-call semantics. It must not announce future work; if work remains,
the same assistant response carries one `execute_cell` call. The legacy final
`<!-- pane:done -->` line is still accepted and hidden for saved sessions, but
new prompts do not require it. A program's top-level returned string is the
other completion signal. Conversational replies stay prose and require no
executable cell.

### Repairing a parse-failed cell

A parser error, before any code executes, offers a local repair primitive in
that error's result. The model may send one complete `pane-edit` fence:

    ```pane-edit
    {"cell": 3, "replace": "return 'done;", "with": "return 'done';"}
    ```

The cell ID must name the latest parse-failed source in the current runtime.
`replace` must be nonempty and match exactly once; replacement is literal,
not a regex. Unknown JSON fields, no-op edits, ambiguous or missing matches,
stale IDs, and oversized edits are refused without execution or target loss.
The source, edit JSON, and result are bounded to 128 KiB each. A new ordinary
cell, task end, or runtime reset invalidates the old target. Runtime-thrown
`SyntaxError`, even with no tool calls, does not authorize a repair/replay.

Pane applies a valid edit locally and runs the corrected source as a **new
cell**, through the same compiler, sandbox, cancellation, and accounting path.
The original record is immutable; the new record stores the complete amended
source. A new parse error offers the new cell ID. No corrected source copy is
added to the next model result. Invalid edits count toward bounded malformed
reply handling. A message containing both `pane` and `pane-edit`, or multiple
`pane-edit` blocks, runs neither. The same repair path is available to
subagents. A repair is protocol data, not a reentrant JavaScript function.

## 6. The result block, usage and limits

The runtime's reply is one user message with up to four sections, always in
this order and each omitted when empty:

    [cell 1 yielded in 412 ms]

    ## Handles
    …the table from §4…

    ## stdout
    …last 8,192 estimated tokens of the program's console output…

    ## Usage
    turn output cap 8,000 · task spent 3,412 · cells 1/40

A throw replaces the first line with `[cell 3 threw in 88 ms]` and adds an
`## Error` section carrying the class, the message, the source line and column
inside the model's program, and the top three in-program frames.

The usage figures are: the output-token cap for the turn about to start
(default 8,000); cumulative provider-reported tokens spent by the task, read
from the gateway's own usage row rather than estimated when available; and
cells used against their cap (default 40). Task token spend is telemetry. It
has no cap, never changes the prompt and never stops a task. Reaching the cell
limit still gives the model one final turn whose only permitted action is a
top-level returned string.

A cell that returned a value adds an `## Output` section before `## stdout`
and two more usage figures (2026-09-23, `runtime-contract.md` §9.2):

    ## Output
    ### readme
    [lines 1-240 of 1,508]
       1 | //! The contract every supported harness is reached through.
       …
     240 | }
    [+1,268 lines not shown · call .excerpt({start: 241, lines: 1268}) on the same File]

    count: 1508

    ## Usage
    turn output cap 8,000 · task spent 3,412 · cells 2 · return budget 24,000 · this return 6,120 (paged: readme)

`return budget` is how many estimated tokens a returned value may fill this
turn — a quarter of the room left in the context window after the turn's
output cap, between 4,000 and 24,000, or 8,000 when the window is unknown.
`this return` is what the last return cost, with the fields that were paged
to fit. A field is paged at a line boundary and ends in one cursor line saying
how to read on; nothing is cut at a byte count and no field is replaced by its
type. A small object renders on one line as the program wrote it. With
`[helpers] reduce_returns` and a decisions model, a large field the decision
model reads as a log arrives reduced to its failures under the reducer's
`[pane:reduction …]` line (`decision-model.md` §10); the whole value stays
live in the program's bindings. With `[helpers] prefetch_returns`, a return
the decision model reads as not enough is followed by the in-project files
it names, each a numbered block under `### [prefetched] path` within the
same budget (`decision-model.md` §11); `read` is the way to hold one.

## 7. The worked turn, as bytes

The runtime's view of this turn is `runtime-contract.md` §6. Same task, same
names, same previews. Figures measured on this repository at `4d97c8f`.

**Turn 1, user[0]:**

    Every file that names `IntegrationId` — how many are tests, and which
    production files would a new variant force me to touch?

**Turn 1, assistant:**

    ```pane
    const hits = await grep({ pattern: "IntegrationId", glob: "crates/glasshouse/**/*.rs" });
    const adapter = await read({ path: "crates/glasshouse/src/harness/mod.rs" });
    ```

**Turn 2, user[1]** — exactly these bytes:

    [cell 1 yielded in 412 ms]

    ## Handles
    hits     Grep.Match[]   n=1195   inline cost ~30,565 tok · preview 139 tok
      [0]      crates/glasshouse/tests/gateway_translate_effort.rs:29  "use glasshouse::integrations::IntegrationId;"
      [1]      crates/glasshouse/tests/gateway_translate_effort.rs:512 "let mut profile = LaunchProfile::native(IntegrationId::ClaudeCode);"
      [2]      crates/glasshouse/tests/gateway_translate_responses.rs:35 "use glasshouse::integrations::IntegrationId;"
      [1194]   crates/glasshouse/src/session/store/record.rs:425  "/// [`crate::integrations::IntegrationId`] string."
    adapter  File   crates/glasshouse/src/harness/mod.rs   63,979 B · 1,508 lines · 2026-09-05T14:18:26Z
      L1       "//! The contract every supported harness is reached through."
      L2       "//!"                                                          preview 66 tok

    ## Usage
    turn output cap 8,000 · task spent 3,412 · cells 1/40

**Turn 2, assistant:**

    ```pane
    const isTest = (m) => m.path.startsWith("crates/glasshouse/tests/");
    const inTests = hits.filter(isTest);
    const prodFiles = new Set(hits.filter(m => !isTest(m)).map(m => m.path));
    return { total: hits.length, in_tests: inTests.length, prod_files: prodFiles.size };
    ```

The task ends with `{total: 1195, in_tests: 290, prod_files: 62}`. 122,261
bytes of grep output and 63,979 bytes of source were addressed by name and
never serialised into a message.

## 8. The gateway hop changes nothing in the prompt

Byte for byte, the request pane builds is identical whether `ANTHROPIC_BASE_URL`
names Glasshouse's gateway (`Gateway::base_url`,
`crates/inference-gateway/src/gateway/mod.rs:647`) or the provider directly. Not
"nearly identical": identical, and tested by equality of the serialised body
across both modes.

The reason is 61A. A ruler that compared pane-through-the-gateway with
pane-direct while the two sent different prompts would be comparing two
prompts, and no measurement made that way is worth anything. Everything the
hop adds — which entitlement paid, which provider served, what it cost — is
read back from the gateway's response and shown in the **sidebar**, which is
UI and never enters a message.

Two consequences worth stating because they are easy to get wrong later:
`/model auto` is a *routing* instruction to the gateway, carried in the
request's model field, and it does not change a byte of the system block; and
a firewall reduction on the relayed path must preserve Pane's correlated cell
result semantics; the same request body is used through the gateway and direct.

### Explicit cache boundaries and usage

The outbound `system` is a one-element array of text blocks. Its `text` is
exactly the rendered system string, with `cache_control: {"type":"ephemeral"}`
on that block (an empty system uses an empty array). The native `execute_cell`
tool definition has the same breakpoint,
so a changed system can still reuse the tool prefix. Streaming uses the same
serializer and boundaries. Neither path marks conversation messages: task
boundaries, handles, cell output and usage snapshots stay after the cached
system prefix. Changed instructions intentionally change that prefix.

These are requests for caching, not evidence of a hit. Pane exposes provider
`cache_read_input_tokens` and `cache_creation_input_tokens` separately from
`input_tokens` and `output_tokens`; missing or malformed cache fields stay
unknown and explicit zero stays zero. Streaming merges supplied fields across the start
and final usage events without summing cumulative counts. Request telemetry
prefers correlated gateway cache-read counts and otherwise uses the response;
cache writes come from the response. No count or dollar saving is inferred from
the presence of a breakpoint. Provider support, cache lifetime and minimum
prefix length still determine whether caching occurs, as specified by the
[Anthropic prompt caching protocol](https://platform.claude.com/docs/en/build-with-claude/prompt-caching).

Pane presents request context and task spend as two separate measurements.
Request context is the most recent request's `input_tokens` plus cache-read
and cache-creation input; it excludes output and replaces the previous
request's value. Task spend remains the cumulative sum of every reported
token class across every request in the task. The statusline and telemetry
rail use `context`/`ctx` for the first and `spent` for the second so the task
cap can never be mistaken for a model context window.

A context-window percentage is shown only when the launch supplied
`--context-window-tokens` for its initial model. Pane does not infer a window
from the task cap, output cap, model name, or another provider's catalogue.
After `/model` selects a different identifier, the denominator is unknown.
While a request is active, the existing presentation clock may move a glint
inside the measured fill; the occupied fraction does not change and reduced
motion freezes it. These values are local presentation state and are not
added to model messages or rollout rows.

## 9. What this contract does not decide

- **Slash-command rendering.** `/handles`, `/budget` and the rest are TUI
  commands; whether any of them injects text is 61C's.
- **Sampling parameters and thinking budgets.** These are
  request fields, not prompt bytes, and the gateway may rewrite them
  (`gateway/translate/canonical.rs`). See `phase-minus-one.md` §5 for the one
  place that is not yet safe.

## 10. Addendum — the `batch` row (61G)

`events-contract.md` adds exactly one row to §4's handle table, named `batch` and rendered **always
last**, so the model's own bindings keep the order it made them in. The row carries the batch preview
that contract fixes — every interrupt in full, then counts by kind, then the first five of the rest —
inside §4's 2,048-token table cap, and it changes nothing else here: a batch is not a message, it
adds no section to §6's result block, and it never becomes a turn of its own. A turn whose batch is
empty and whose user input is empty does not happen; the runtime waits rather than send the request.

CONTRACT
behaviour:  The model receives one cached system block of preamble, TypeScript tool declarations and project instructions, and acts through one provider-native `execute_cell` call whose correlated result arrives before another inference.
invariant:  The serialised request body is byte-identical with and without the Glasshouse gateway hop; incomplete native input and malformed legacy fences execute nothing.
path:       `crates/pane/src/prompt/` renders the system and feedback; `crates/pane/src/wire.rs` preserves native calls, results and streaming input correlation.
test:       `crates/pane/tests/prompt_bytes.rs::the_worked_turn_renders_byte_for_byte` — a golden file of §7's four messages, plus `the_gateway_hop_changes_no_byte` asserting equality of the serialised body across both base URLs.


### Request history and live state

New runtime feedback has two renderer-produced forms: its full observation
and its historical form without handle, plan and usage snapshots. Requests
keep the newest live snapshot for the current task and use the historical form
for older feedback. Errors, stdout, yield reasons, syntax repair hints and
supervisor guidance remain. A new task does not present the previous task's
live handles as current.

This is a request-only projection using trusted renderer provenance. User or
assistant text resembling a section header is not classified as runtime state;
stdout with such headings remains intact. The UI and append-only rollout keep
the full original observations; an optional historical field preserves the
projection across resume. Older records without this provenance
are retained conservatively rather than guessed from their text. Historical
handle previews are intentionally superseded; current live objects remain
available through handles and explicit inspection.

An overflow of this already-projected request creates one checkpoint and retries
once, without replaying cells or discarding live runtime bindings. The rollout
marks that context boundary explicitly; resume starts request history at the
latest checkpoint while retaining all earlier rows in the append-only log.


## Reusing a runner across cells

Request-local bindings can hold ordinary functions, including async functions.
A model can define a runner once, then invoke its identifier in later cells:

```typescript
const verify = async () => {
  const result = await bash({command: "python3 -m unittest discover -s tests"});
  console.log(result.stdout);
  console.log(result.stderr);
  if (result.exit_code !== 0) throw new Error("verification failed");
};
await verify();
```

A later cell can use `await verify();` without generating the function body
again. The original definition may remain in conversation history and provider
input; this is not by itself a promise of lower total billed tokens.
Choose a command admitted by the session and an interpreter available in its
sandbox. Every invocation executes the real tool, records its trajectory and
checks current permissions; the function is not a cached test result. Source
inspection and version checks remain required for edits performed by a runner.

A saved function retains its definition and captured values. Pass changing
inputs as parameters, or redeclare the runner when its logic changes.
Functions live only for this user request, like other bindings. For reuse across
requests, save the test logic as an inspectable project script and invoke that
script through an admitted command. Changing source or test inputs requires a
new run; a previous pass does not certify the changed files. The named-runner
regression changes input between cells and observes new output, then proves an
unadmitted command is still refused through the same function.
