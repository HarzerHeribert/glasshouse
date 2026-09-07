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

    You are Pane, a coding assistant. Answer conversational questions naturally.
    To act with tools, make exactly one `execute_cell` call in an assistant turn.
    Put every operation in that one TypeScript program; `execute_cell` is the only
    provider-native tool. Runtime tools are callable only inside its code.
    While you construct the call, none of THIS cell has executed. Code may await
    tools and branch on their actual returned values. Batch deterministic work
    when useful; stop at the next decision that needs unseen evidence. After
    submitting a cell, wait for its correlated result. Never invent output or
    infer success: only that result is runtime evidence.

    A cell is validated before it runs. A parse error runs nothing and may offer
    `pane-edit`; a return, yield, or throw stops later code. Tool results are live
    objects, but unseen fields are not model-visible. Use declared fields and
    standard JavaScript; do not bind a declared tool or host-global name. Reuse
    live handles rather than repeating reads. For an existing source change,
    `context({path, symbol})` is the first source-reading tool; do not `read` or
    print the whole source first. Its complete target is delivered automatically.
    In the next cell use `edit({path, old, replacement})`; do not name a variable
    `new`. Pane binds the edit to the sole visible source version. Use
    compact structured summaries or bounded excerpts instead of broad prints.
    `glob` may return directories, so select a file before `read`. A `bash`
    result succeeded only when its
    `exit_code` says so.

    Bindings persist between cells of this user request; redeclaring replaces
    them. Each new user request starts a fresh runtime. Earlier requests are
    history, not unfinished work. Work on the current request, including its
    requested tests. Running off the end or `yieldNow(reason)` gives results
    and another turn. A top-level `return` ends the task; return an answer
    grounded in results you observed.

    A prose response with no `execute_cell` call ends the task as the answer.
    Use prose-only output only when the request is finished; do not use it to
    announce work you still intend to perform.
    To interpret a file, inspect and yield first, then answer from the feedback.
    You may return values computed directly from objects.

    A thrown error carries its position and completed bindings. Continue from
    that state; failed or skipped calls did not succeed. PermissionDenied is
    final: code cannot widen the session's sandbox grant.

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
version returned by `context` in a prior completed cell. Pane supplies the hash
when exactly one complete version of that path is visible; the caller supplies
`expected_sha256` to disambiguate multiple versions. Stale, missing, ambiguous
and no-op requests throw without writing or retrying. `write` remains the
whole-file and new-file operation.

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

Rendered fresh every turn, after the tools and before the budget. Empty on
turn 1 and written as `## Handles\n(none)`. Otherwise one entry per live
handle, in declaration order, in the shape `runtime-contract.md` §3 fixes.
The whole table is capped at 2,048 tokens; over it, the oldest entries are
dropped from the rendering with one line saying how many and how to list them.

Provenance is not shown in the table. A stale handle carries the one word
`stale` and nothing else; the model gets the recorded call in the
`StaleHandle` message if it touches one.

## 5. The native execution handoff

The primary action channel is the provider-native `execute_cell` tool. Its input schema is exactly `{code: string}`. The assistant's generation ends at the call; Pane executes only the complete, decoded input and returns a correlated `tool_result` before the model can interpret it. Malformed or truncated JSON never executes. Unknown or multiple calls remain explicit typed calls so the session can reject each without silently dropping it.

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
new prompts do not require it. A program's top-level `return` is the other
completion signal. Conversational replies stay prose and require no executable
cell.

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
cell**, through the same compiler, sandbox, cancellation, and budget path.
The original record is immutable; the new record stores the complete amended
source. A new parse error offers the new cell ID. No corrected source copy is
added to the next model result. Invalid edits count toward bounded malformed
reply handling. A message containing both `pane` and `pane-edit`, or multiple
`pane-edit` blocks, runs neither. The same repair path is available to
subagents. A repair is protocol data, not a reentrant JavaScript function.

## 6. The result block, and the budget line

The runtime's reply is one user message with up to four sections, always in
this order and each omitted when empty:

    [cell 1 yielded in 412 ms]

    ## Handles
    …the table from §4…

    ## stdout
    …last 8,192 estimated tokens of the program's console output…

    ## Budget
    turn cap 8,000 · task 3,412/400,000 · cells 1/40

A throw replaces the first line with `[cell 3 threw in 88 ms]` and adds an
`## Error` section carrying the class, the message, the source line and column
inside the model's program, and the top three in-program frames.

The three budget figures are: the output-token cap for the turn about to
start (default 8,000); total provider-reported tokens for the task (default
400,000, read from the gateway's own usage row rather than estimated); and
cells used against their cap (default 40). At 90%
of the task budget the line gains `— finish or return`; when it is exhausted
the next turn's preamble is replaced by one sentence saying the only permitted
action is a top-level `return`.

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

    ## Budget
    turn cap 8,000 · task 3,412/400,000 · cells 1/40

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
`crates/glasshouse/src/gateway/mod.rs:469`) or the provider directly. Not
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
boundaries, handles, cell output and budget snapshots stay after the cached
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
and its historical form without handle, plan and budget snapshots. Requests
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
