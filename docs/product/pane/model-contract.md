# pane — the model contract

Unblocks **61C** and **61E**. The prompt schema, byte for byte: what pane
sends, in what order, with what delimiters, and what it accepts back. The
runtime side of every name used here is `runtime-contract.md`; §7 shows the
same worked turn from this side, and the two must agree.

## 1. The message layout

One Anthropic Messages request per turn. The system block is stable for the
whole task — it is what the provider's prompt cache holds — and the
conversation carries alternating assistant cells and runtime results.

    system    : preamble · tool declarations · project instructions
    user[0]   : the task
    assistant : ```pane block (cell 1)
    user[1]   : cell result 1
    assistant : ```pane block (cell 2)
    …

Nothing else is ever in a message. No tool-use blocks, no tool-result blocks,
no second serialization of any object (61E's last line, and it is structural
here: the runtime has no code path that writes a payload into a message).

## 2. The system preamble, verbatim

    You act by writing TypeScript. Each turn you emit exactly one code block
    tagged `pane`; pane runs it in a persistent V8 isolate and answers with
    what your program produced.

    Tool results are live objects, not text. `await grep(...)` returns an
    array you can filter, index and count in the next line of the same
    program. You are shown each object's name and a short preview; you are
    never shown its payload, and you never need it.

    Bindings persist. A top-level `const` in one cell is in scope in the
    next. Redeclaring a name replaces the object and frees the old one.

    A cell that runs off the end yields: you get the handle table and another
    turn. A top-level `return` ends the task with that value. Return when the
    task is answered, not before.

    A cell that throws is answered, not retried. You get the error, the line,
    and every binding that completed before the throw. Write the next cell.

    A call outside this session's sandbox grant throws PermissionDenied. It
    is catchable and it is final: nothing you write widens a grant.

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

## 4. The handle table

Rendered fresh every turn, after the tools and before the budget. Empty on
turn 1 and written as `## Handles\n(none)`. Otherwise one entry per live
handle, in declaration order, in the shape `runtime-contract.md` §3 fixes.
The whole table is capped at 2,048 tokens; over it, the oldest entries are
dropped from the rendering with one line saying how many and how to list them.

Provenance is not shown in the table. A stale handle carries the one word
`stale` and nothing else; the model gets the recorded call in the
`StaleHandle` message if it touches one.

## 5. The block delimiter

The model's program is a fenced block whose info string is exactly `pane`:

    ```pane
    const report = await cargo_test({ target: "firewall_bridge" });
    ```

**Not `ts`.** A model writing about TypeScript emits ```` ```ts ```` blocks
constantly, and a parser that executed them would run the model's
explanations. The language inside is TypeScript; the tag names the channel.

**Exactly one such block per assistant message.** A message with two is a
protocol error: **neither** runs, and the model is told
`two pane blocks in one turn; send one`. Running the first and ignoring the
second is the silently-wrong reading — the second is usually the one the model
meant.

A message with no `pane` block and no `return` is prose; pane shows it in the
conversation column and answers with the unchanged handle table and one line
saying no program ran. The task does not advance and the cell counter does not
move.

## 6. The result block, and the budget line

The runtime's reply is one user message with up to four sections, always in
this order and each omitted when empty:

    [cell 1 yielded in 412 ms]

    ## Handles
    …the table from §4…

    ## stdout
    …last 512 tokens of the program's console output…

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
a firewall reduction on the relayed path
(`crates/glasshouse/src/firewall/mod.rs:284`) can never apply to pane, because
pane sends no tool-result blocks for it to reduce.

## 9. What this contract does not decide

- **The project instruction block's assembly** — how `CLAUDE.md`, `AGENTS.md`
  and the skills directories are concatenated into the system block's third
  section. That is 61C's drop-in package; this contract fixes only its
  position and that it is inside the cached system block.
- **Slash-command rendering.** `/handles`, `/budget` and the rest are TUI
  commands; whether any of them injects text is 61C's.
- **Sampling parameters, thinking budgets and cache breakpoints.** These are
  request fields, not prompt bytes, and the gateway may rewrite them
  (`gateway/translate/canonical.rs`). See `phase-minus-one.md` §5 for the one
  place that is not yet safe.

CONTRACT
behaviour:  The model receives one cached system block of preamble, TypeScript tool declarations and project instructions, and answers each turn with exactly one ```pane fenced TypeScript program; it never sends or receives a tool-use or tool-result block.
invariant:  The serialised request body is byte-identical with and without the Glasshouse gateway hop, and a message carrying two `pane` blocks executes neither.
path:       `crates/pane/src/prompt/`: the renderer that assembles the system block, the handle table and the budget line, and the single parser that extracts the one `pane` block from an assistant message.
test:       `crates/pane/tests/prompt_bytes.rs::the_worked_turn_renders_byte_for_byte` — a golden file of §7's four messages, plus `the_gateway_hop_changes_no_byte` asserting equality of the serialised body across both base URLs.
