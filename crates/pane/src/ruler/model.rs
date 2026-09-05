//! The vocabulary the ruler's four stages share: what a task is, what one
//! attempt of it recorded, and what a token figure nobody counted looks like.
//!
//! Nothing here decides anything. [`tasks`](super::tasks) supplies the
//! catalogue, [`attempt`](super::attempt) fills an [`Attempt`],
//! [`score`](super::score) aggregates and [`report`](super::report) renders.

use std::time::Duration;

/// A workload tier, in the router's own vocabulary
/// (`glasshouse::routing::classify::WorkloadTier`).
///
/// Three variants, not five: `Deterministic` and `Frontier` have no producer
/// in that build, and a tier nothing can classify into would score an empty
/// set. This gains a variant the day one of them has a producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Leaf,
    Standard,
    Heavy,
}

impl Tier {
    /// The three tiers in report order, which is also difficulty order.
    pub const ALL: [Tier; 3] = [Tier::Leaf, Tier::Standard, Tier::Heavy];

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Leaf => "leaf",
            Tier::Standard => "standard",
            Tier::Heavy => "heavy",
        }
    }
}

/// One task: a commit of this repository that closed a map line or fixed a
/// defect, plus the four things a harness is handed or judged by.
///
/// `statement` is derived from the commit's subject and the map line it
/// names, never from its diff, and it is a fixed string so that neither
/// harness gets a word the other does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Task {
    /// `L1`, `S3`, `H4` — tier letter and index.
    pub id: &'static str,
    pub tier: Tier,
    /// The commit that closed the line. The harness starts from its *parent*.
    pub commit: &'static str,
    /// What the harness is told to do. Equal across harnesses, by construction.
    pub statement: &'static str,
    /// The commit's own test commands, run by the ruler after the harness
    /// stops, in order; the task completes only if every one of them exits 0.
    ///
    /// Each inner slice is a **complete command** — program first, arguments
    /// after — because four tasks carry two invocations (`ruler.md` §2's
    /// `S1`, `H1`, `H2`, `H4`) and one carries a shell script rather than
    /// cargo (`H3`). A single flat argv could express neither: concatenating
    /// two cargo invocations turns the second target selector into a
    /// test-name filter over both, which runs something else entirely.
    pub test: &'static [&'static [&'static str]],
    /// The commit's own changed-line count. An attempt whose diff is under a
    /// tenth of this is reported `pass (suspect)` with the figure — the ruler
    /// does not judge the diff, it prints the number that makes a human look.
    pub shortstat_lines: u32,
}

impl Task {
    /// The bound below which a passing attempt is suspect: a tenth of the
    /// commit's own changed-line count, rounded down.
    pub fn suspect_bound(&self) -> u32 {
        self.shortstat_lines / 10
    }
}

/// Which harness ran an attempt, as the meter records it in
/// `routing_observations.harness`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Harness(String);

impl Harness {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Harness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the meter counted for one attempt. **Absent is not zero**, and the
/// distinction is the whole reason this is three `Option`s rather than three
/// integers: a figure nobody read must never be able to look like a figure
/// somebody read and found to be nought.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cached_input: Option<u64>,
}

impl Tokens {
    /// The attempt's total, or `None` when it was never counted.
    ///
    /// `input` and `output` are both-or-neither at the source (the gateway
    /// writes `NULL` to both when a provider stated only one), so either
    /// being absent means the exchange was not metered. `cached_input`
    /// absent is a different fact — the provider stated no cache figure —
    /// and contributes nothing to a total that is still real.
    pub fn total(&self) -> Option<u64> {
        match (self.input, self.output) {
            (Some(input), Some(output)) => Some(input + output + self.cached_input.unwrap_or(0)),
            _ => None,
        }
    }

    /// Sum across attempts, absent unless at least one was counted; counted
    /// attempts sum and uncounted ones are skipped rather than read as zero.
    pub fn sum<'a>(attempts: impl IntoIterator<Item = &'a Tokens>) -> Option<u64> {
        attempts
            .into_iter()
            .filter_map(Tokens::total)
            .fold(None, |acc, n| Some(acc.unwrap_or(0) + n))
    }
}

/// The result of running the task's own test command on the harness's tree
/// after the harness stops. **There is no partial credit and no rubric.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    /// The test passed and the diff is under [`Task::suspect_bound`]. Counted
    /// as a completion, printed with the figure so a human looks.
    PassSuspect {
        changed_lines: u32,
        bound: u32,
    },
    /// The test command exited non-zero. A harness that reports success and
    /// fails the test scores this.
    Fail,
    /// The attempt never reached its test: the worktree, the harness launch
    /// or the ruler itself failed. Not the harness's loss, and never folded
    /// into either token column.
    Errored,
}

impl Outcome {
    /// Whether this attempt completed the task. The **only** predicate the
    /// headline may be filtered by — see [`super::score`].
    pub fn completed(self) -> bool {
        matches!(self, Outcome::Pass | Outcome::PassSuspect { .. })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::PassSuspect { .. } => "pass (suspect)",
            Outcome::Fail => "fail",
            Outcome::Errored => "errored",
        }
    }
}

/// One attempt of one task by one harness: everything the report and the
/// JSONL line are rendered from, and the only thing [`super::score`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub task: &'static str,
    pub tier: Tier,
    pub harness: Harness,
    /// The parent commit the attempt's worktree was cut at.
    pub base_commit: String,
    /// 1-based, within `(task, harness)`.
    pub attempt: u32,
    pub outcome: Outcome,
    pub tokens: Tokens,
    /// First request leaving the gateway to the test command exiting.
    pub wall_clock: Duration,
    /// Exchanges the meter recorded for this attempt. Printed; **never
    /// divided into** — see map line 2432 and `report::NO_PER_TURN_DIVISION`.
    pub turns: Option<u32>,
    /// The attempt's own `--shortstat` insertions plus deletions.
    pub changed_lines: Option<u32>,
}
