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
