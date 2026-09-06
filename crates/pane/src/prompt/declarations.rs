//! The TypeScript return type and doc sentence for each registered tool —
//! `docs/product/pane/model-contract.md` §3. [`crate::tools::registry`] does
//! not carry either fact yet, so this table is the one place they live.
//! `prompt_bytes.rs::every_registered_tool_has_exactly_one_declaration_and_no_other_does`
//! pins that [`ENTRIES`]' names equal `registry::names()` exactly, so the day
//! the registry does carry them, this file is what goes.

use crate::tools::registry::Purity;

/// One tool's return type and its own descriptive sentence.
pub struct Entry {
    pub name: &'static str,
    pub return_type: &'static str,
    pub summary: &'static str,
}

pub const ENTRIES: &[Entry] = &[
    Entry {
        name: "read",
        return_type: "File",
        summary: "Read one file inside the project.",
    },
    Entry {
        name: "glob",
        return_type: "string[]",
        summary: "List paths inside the project matching a glob pattern.",
    },
    Entry {
        name: "grep",
        return_type: "Grep.Match[]",
        summary: "Search the project for a regular expression.",
    },
    Entry {
        name: "bash",
        return_type: "{stdout: string; stderr: string; exit_code: number | null}",
        summary: "Run a command line under the sandbox grant.",
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
                      // PermissionDenied, before any handle exists. Do not poll a job; do not\n\
                      // sleep waiting for one.",
    },
    Binding {
        global: "batch",
        declaration: "declare const batch: {\n  \
                      n: number;\n  \
                      where(query: {kind?: string; source?: string}): Event[];\n  \
                      ack(ids: number[]): void;\n  \
                      rest(): Event[];\n\
                      };\n\
                      type Event = {id: number; kind: string; source: string; result?: unknown};\n\
                      // Everything that happened while you were not looking — finished\n\
                      // background jobs, hooks, CI, messages — delivered as one object per\n\
                      // turn rather than as an interruption each. `batch.where({kind: \"bg.done\"})`\n\
                      // selects; `kind` matches by prefix. Ack what you have dealt with, and\n\
                      // anything you leave returns in the next batch. Absent when nothing\n\
                      // happened.",
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
                      // Printed with the cell's result, bounded per argument. It is for you to\n\
                      // read next turn, not a way to answer the person.",
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
