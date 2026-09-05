//! The task catalogue: twelve commits of this repository, four per tier.
//!
//! The catalogue is a fixed table rather than a discovered one, because a
//! comparison whose task set can drift between runs compares two things that
//! were never the same question. Specification: `docs/product/pane/ruler.md`
//! §2, which is also where each statement's derivation is recorded.

use super::model::{Task, Tier};

/// The twelve tasks, in tier then index order.
///
/// Populated by `GH-PANE-61A-SCORE` from `ruler.md` §2. `shortstat_lines` is
/// each commit's own `git show --shortstat` insertions plus deletions,
/// recorded in the worker report that filled this table. `test` is a list of
/// complete commands (program first, arguments after); four tasks (`S1`,
/// `H1`, `H2`, `H4`) name two commands in `ruler.md`'s table and carry two
/// entries here, each run in order by the ruler.
pub static CATALOGUE: &[Task] = &[
    // Tier 1 -- Leaf
    Task {
        id: "L1",
        tier: Tier::Leaf,
        commit: "fa66efc",
        statement: "a std::fs import used only by the unix permission tests breaks the Windows build after a split",
        test: &[&["cargo", "build", "-p", "glasshouse", "--tests"]],
        shortstat_lines: 6,
    },
    Task {
        id: "L2",
        tier: Tier::Leaf,
        commit: "ca18723",
        statement: "a test pins that a referenced file cannot be stored; line 1139's producer has landed",
        test: &[&["cargo", "test", "--test", "memory_file_observer"]],
        shortstat_lines: 20,
    },
    Task {
        id: "L3",
        tier: Tier::Leaf,
        commit: "9c1b0a5",
        statement: "re-read Codex's hook catalogue from the installed 0.153.3 and make every declaration match",
        test: &[&["cargo", "test", "--lib", "harness::codex"]],
        shortstat_lines: 123,
    },
    Task {
        id: "L4",
        tier: Tier::Leaf,
        commit: "ad2e8f5",
        statement: "the 1836 line must print after served:; the view tests read each account's block by position",
        test: &[&["cargo", "test", "--test", "entitlement_broker"]],
        shortstat_lines: 39,
    },
    // Tier 2 -- Standard
    Task {
        id: "S1",
        tier: Tier::Standard,
        commit: "e9178c0",
        statement: "a verbatim (\\\\?\\) project root refuses every path inside it on Windows",
        test: &[
            &["cargo", "test", "--test", "project_isolation"],
            &["cargo", "test", "--lib", "commands::context_firewall"],
        ],
        shortstat_lines: 206,
    },
    Task {
        id: "S2",
        tier: Tier::Standard,
        commit: "045c71d",
        statement: "the relay's gzip limit differs per harness — Codex always populated, Claude Code conditional",
        test: &[&["cargo", "test", "--test", "relay_usage"]],
        shortstat_lines: 32,
    },
    Task {
        id: "S3",
        tier: Tier::Standard,
        commit: "09e6ae9",
        statement: "map lines 2409–2410: predict a conflict, and name the distinction rather than implying it",
        test: &[&["cargo", "test", "--test", "orchestrator_conflict"]],
        shortstat_lines: 205,
    },
    Task {
        id: "S4",
        tier: Tier::Standard,
        commit: "2bdbbc5",
        statement: "the reranking tripwire fired as designed; invert it into a four-caller census and close 1625",
        test: &[&["cargo", "test", "--test", "memory_reranker"]],
        shortstat_lines: 137,
    },
    // Tier 3 -- Heavy
    Task {
        id: "H1",
        tier: Tier::Heavy,
        commit: "a61ba99",
        statement: "the database bootstrap straggler waits on a timer and fails every module-level run under load",
        test: &[
            &["cargo", "test", "--test", "project_isolation"],
            &["cargo", "test", "--lib", "database"],
        ],
        shortstat_lines: 641,
    },
    Task {
        id: "H2",
        tier: Tier::Heavy,
        commit: "26fb65b",
        statement: "create the project database privately and publish it with one hard link; the race's successor",
        test: &[
            &["cargo", "test", "--lib", "database"],
            &["cargo", "test", "--test", "memory_store"],
        ],
        shortstat_lines: 1217,
    },
    Task {
        id: "H3",
        tier: Tier::Heavy,
        commit: "ee7799b",
        statement: "main.rs is over the size ratchet; split it into commands/ with every import path kept valid",
        test: &[&["scripts/blast-radius.sh"]],
        shortstat_lines: 58943,
    },
    Task {
        id: "H4",
        tier: Tier::Heavy,
        commit: "f2883ca",
        statement: "map lines 2402–2405: edit intent, and 2392 finally gets a producer",
        test: &[
            &["cargo", "test", "--test", "edit_intent"],
            &["cargo", "test", "--test", "file_claims"],
        ],
        shortstat_lines: 1766,
    },
];

/// The task with this id, or `None`. Ids are case-sensitive: `L1`, not `l1`.
pub fn lookup(id: &str) -> Option<&'static Task> {
    CATALOGUE.iter().find(|task| task.id == id)
}

/// Every task in one tier, in catalogue order.
pub fn in_tier(tier: Tier) -> impl Iterator<Item = &'static Task> {
    CATALOGUE.iter().filter(move |task| task.tier == tier)
}
