//! The TypeScript return type and doc sentence for each registered tool —
//! `docs/product/pane/model-contract.md` §3. [`crate::tools::registry`] does
//! not carry either fact yet, so this table is the one place they live.
//! `prompt_bytes.rs::every_registered_tool_has_exactly_one_declaration_and_no_other_does`
//! pins that [`ENTRIES`]' names equal `registry::names()` exactly, so the day
//! the registry does carry them, this file is what goes.

use crate::tools::registry::Purity;

pub const EXECUTE_CELL_NAME: &str = "execute_cell";
pub const EXECUTE_CELL_DESCRIPTION: &str = "Make exactly one native call in this assistant turn to run one Pane TypeScript cell. Put every runtime operation inside its single program; functions such as read and bash are not separate native calls. While constructing the input, none of this cell has run. Code may branch on results returned by awaited tools inside the cell; after submitting it, stop and wait for the correlated tool result before interpreting outcomes. Prose and comments are not runtime evidence.";

/// One tool's return type and its own descriptive sentence.
pub struct Entry {
    pub name: &'static str,
    pub return_type: &'static str,
    pub summary: &'static str,
}

pub const ENTRIES: &[Entry] = &[
    Entry {
        name: "read",
        return_type: "{path: string; text: string; lines: string[]; bytes: number; lineCount: number; mtime: string; sha256: string; excerpt(options?: {start?: number; lines?: number}): {text: string; start: number; end: number | null; lineCount: number; next: number | null; truncatedLines: number}}",
        summary: "Read one documentation, configuration, or modest source file inside the project. If a large source has one uniquely unfinished definition, Pane promotes the read to its bounded `context`; otherwise use `context` first when you will edit source. Never print a whole `File.text` or broad `File.lines`.",
    },
    Entry {
        name: "glob",
        return_type: "string[]",
        summary: "List paths inside the project matching a glob pattern. Results may include directories: do not pass a bare directory match to `read`; select a file path.",
    },
    Entry {
        name: "grep",
        return_type: "Grep.Match[]",
        summary: "Search the project for a regular expression.",
    },
    Entry {
        name: "write",
        return_type: "string",
        summary: "Replace one whole file, creating parents. Pass exactly one of `content` or `lines`; `lines` adds a final newline and safely carries literal script text, e.g. `await write({path: \"run.sh\", lines: [\"#!/bin/bash\", 'src=\"${BASH_SOURCE[0]}\"', 'echo \"$WORKTREE\"']})`. Prefer `edit` for an existing region.",
    },
    Entry {
        name: "bash",
        return_type: "{stdout: string; stderr: string; exit_code: number | null}",
        summary: "Run a command line under the sandbox grant; inspect `exit_code` before treating it as successful.",
    },
    Entry {
        name: "context",
        return_type: "{path: string; sha256: string; symbol: string | null; text: string; complete: boolean; ranges: {path: string; start: number; end: number; role: string}[]; omissions: string[]}",
        summary: "Load the editing surface for one file or symbol. For a large source file, supply the target `symbol`. Its complete target, short display version, and ranked support are automatically printed once; the handle retains full `sha256` for rare disambiguation. Do not print `text` or inspect the same file with `read`.",
    },
    Entry {
        name: "edit",
        return_type: "{path: string; before_sha256: string; after_sha256: string; changed_lines: {start: number; before: number; after: number}}",
        summary: "After `const ctx = await context(...)` completed in the prior cell, call `edit({path, old, replacement})`, or use `oldLines` and `replacementLines` for newline-terminated literal blocks. Pass exactly one form for each side. Pane supplies `expected_sha256` when exactly one complete version is visible; pass `expected_sha256: ctx.sha256` only to disambiguate. Stale, missing, ambiguous, unseen, and no-op edits do not write.",
    },
];

/// The entry for `name`, or `None` for a tool this table does not cover.
pub fn lookup(name: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|entry| entry.name == name)
}

/// The doc line's purity clause — the registry's own claim about the tool,
/// rendered rather than re-decided.
pub fn purity_clause(purity: Purity) -> &'static str {
    match purity {
        Purity::Pure => "Pure.",
        Purity::Effectful => "Not pure; it may change the world.",
    }
}

/// One host binding that is **not** a tool: the global name
/// `runtime::bindings::install` installs, and the TypeScript the model is
/// shown for it.
///
/// The invariant: **a name the isolate binds is a name the system block
/// declares.** `runtime_cells.rs::every_host_global_is_declared_to_the_model`
/// enumerates the real globals out of a real isolate and fails when one is
/// missing here. Observed 2026-09-06: `bg.run`, `bg.watch`, `bg.cancel`,
/// `keep`, `free` and `handles` had been bound and shipped for a full
/// sub-phase while nothing told the model they existed, so 61G's background
/// jobs were unreachable by the only caller they have.
pub struct Binding {
    /// The global as installed — `bg`, not `bg.run`.
    pub global: &'static str,
    /// The declaration block, rendered into the system prompt verbatim.
    pub declaration: &'static str,
}

/// Every host global that is not a registered tool.
pub const RUNTIME: &[Binding] = &[
    Binding {
        global: "send",
        declaration: "declare function send(session: string, message: string): void;\n// Send once to this project's sessions through Glasshouse, using your session id.\n// Recipient: 1–256 bytes; message: 1–65536 UTF-8 bytes. No standalone transport.\n// A definite refusal delivered nothing. DeliveryUnknown means the reply was lost\n// and delivery may have committed: never retry. Inbound message event.payload()\n// returns {sender: string | null, body: string}; bodies are never previewed.",
    },
    Binding {
        global: "on",
        declaration: "type Handler = {name: string};\ndeclare function on(pattern: {kind?: string; source?: string}, program: string): Handler;\n// Register TypeScript source for matching future batches before model inference.\n// Example: const noise = on({kind: \"hook.*\"}, \"batch.ack(batch.where({kind: \\\"hook.*\\\"}).map(e => e.id));\");\n// The saved program shares this task's persistent scope, sandbox and cell timeout.\n// Acknowledged events are removed before you see the batch. A throw or refusal\n// disables the handler without retry. Nested on() throws HandlerNesting.\n// At most 64 handlers per task, with at most 65536 source bytes each. No resume.",
    },
    Binding {
        global: "off",
        declaration: "declare function off(handler: Handler): void;\n// Cancel a standing handler, idempotently. The person can use /handlers off <binding-name>.",
    },
    Binding {
        global: "mcp",
        declaration: "declare const mcp: {\n  list(): {name: string; server: string; tool: string; description: string; inputSchema: object}[];\n  call(name: string, arguments: object): {content: unknown[]; isError?: boolean; structuredContent?: object};\n};\n// Call mcp.list() to discover project MCP tools and their JSON input schemas.\n// Use the returned exact name in mcp.call(name, arguments). Calls may have effects;\n// inspect isError. Keep results as handles and select the fields you need;\n// do not print full content. Only granted, local stdio tools are discoverable.",
    },
    Binding {
        global: "keep",
        declaration: "declare function keep(name: string, value: unknown): void;\n\
                      // Bind `value` under `name` so it outlives this cell. Redeclaring a\n\
                      // top-level `const` does the same thing; `keep` is for a value that is\n\
                      // not one, such as an element you picked out of an array.",
    },
    Binding {
        global: "free",
        declaration: "declare function free(name: string): void;\n\
                      // Release `name`. The object is freed and the binding disappears. Free\n\
                      // what you are done with; a name you keep is paid for every turn.",
    },
    Binding {
        global: "handles",
        declaration: "declare function handles(): string[];\n\
                      // Every name you can address right now, including ones bound earlier\n\
                      // this cell.",
    },
    Binding {
        global: "yieldNow",
        declaration: "declare function yieldNow(reason: string): never;\n\
                      // Hand the turn back from inside a branch, saying why. A yield, not an\n\
                      // error: you get the handle table and another turn.",
    },
    Binding {
        global: "bg",
        declaration: "declare const bg: {\n  \
                      run(command: string, options?: {cwd?: string; env?: string; timeout?: number}): Job;\n  \
                      watch(command: string, options?: {every?: number; until?: string; timeout?: number}): Job;\n  \
                      cancel(job: Job | string): void;\n\
                      };\n\
                      type Job = {id: string; source: string};\n\
                      // Run a command in the background. `bg.run` returns a handle at once and\n\
                      // never blocks: the exit arrives later as a `bg.done` event whose stdout\n\
                      // and stderr are themselves handles, so a job that printed 40 MB costs a\n\
                      // status line. `bg.watch` re-runs `command` every `every` ms (default\n\
                      // 1000) and emits one `bg.done` per match until `until` matches or you\n\
                      // cancel. Both refuse a command outside the sandbox grant with\n\
                      // PermissionDenied, before any handle exists. Use background work only\n\
                      // when separable from the next decision; do not poll or sleep for it.",
    },
    Binding {
        global: "batch",
        declaration: "declare const batch: {\n  \
                      n: number;\n  \
                      where(query: {kind?: string; source?: string}): Event[];\n  \
                      ack(ids: number[]): {acked: number[]; unknown: number[]};\n  \
                      rest(): Event[];\n\
                      };\n\
                      type EventPayload = {status: string; stdout(): string; stderr(): string};\n\
                      type Event = {id: number; kind: string; source: string; at: string; age: number; summary: string; payload(): EventPayload | null};\n\
                      // Everything that happened while you were not looking — finished\n\
                      // background jobs, hooks, CI, messages — delivered as one object per\n\
                      // turn rather than as an interruption each. `batch.where({kind: \"bg.done\"})`\n\
                      // selects; `kind` matches by prefix. Ack what you have dealt with, and\n\
                      // anything you leave returns in the next batch. Call `payload()` only\n\
                      // when you need the full result; absent payloads return null.",
    },
    Binding {
        global: "agent",
        declaration: "declare const agent: {\n  \
                      run(task: string, options?: {turns?: number; model?: string}): Job;\n\
                      };\n\
                      // Start a subagent on one self-contained question. It returns a handle\n\
                      // at once and never blocks; its answer arrives later as an `agent.done`\n\
                      // event whose payload carries status and output. Launch, then yield;\n\
                      // batch.n is zero until events arrive. Match source: job.source, not job.id.\n\
                      // `batch.where({kind: \"agent.done\"})`. It runs under this session's own\n\
                      // grant, spends this task's budget, and cannot start a subagent of its\n\
                      // own. Use it only when the question is separable and its working would\n\
                      // otherwise fill your context. `bg.cancel` stops one.",
    },
    Binding {
        global: "todo",
        declaration: "declare const todo: {\n  \
                      write(items: {text: string; status: \"pending\" | \"active\" | \"done\"}[]): void;\n  \
                      read(): {text: string; status: string}[];\n\
                      };\n\
                      // Your own plan for this task, shown to the person and carried across\n\
                      // cells. `todo.write` replaces the whole list, so read, change, write\n\
                      // back. Worth writing once the task needs more than two steps, and\n\
                      // worth updating as each finishes — mark exactly one `active`. A\n\
                      // malformed write throws and leaves the plan you had. It is cleared\n\
                      // when the task ends.",
    },
    Binding {
        global: "console",
        declaration: "declare const console: {log(...args: unknown[]): void; info: typeof console.log; \
                      warn: typeof console.log; error: typeof console.log; debug: typeof console.log; \
                      trace: typeof console.log};\n\
                      // Printed with the cell's correlated result, bounded per argument. Prefer a\n\
                      // compact structured summary over broad object or file output.",
    },
];

/// The ECMAScript constants that are non-writable and non-configurable on
/// `globalThis` by specification, and so look exactly like a host binding to
/// the test that enumerates them. Three, fixed by the language.
pub const LANGUAGE_CONSTANTS: [&str; 3] = ["undefined", "NaN", "Infinity"];

/// Whether `name` is a host global this table declares — the tools are
/// covered by [`ENTRIES`], everything else by [`RUNTIME`].
pub fn declares_global(name: &str) -> bool {
    ENTRIES.iter().any(|entry| entry.name == name)
        || RUNTIME.iter().any(|binding| binding.global == name)
}
