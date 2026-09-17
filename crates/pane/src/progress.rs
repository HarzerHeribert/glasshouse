//! The no-progress guard and the verified checkpoint.
//!
//! The invariant: **a repeat is the same calls, the same failure and the
//! same tree.** A fingerprint that ignored the tree would flag a legitimate
//! retry after a fix; one that ignored the failure would flag a retry that
//! got further. The guard therefore never reads the model's words, only the
//! trajectory and the changed-file digest.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::runtime::outcome::{CellRecord, Ended};

/// The notice the guard hands the model, once per streak.
pub const NO_PROGRESS_NOTICE: &str = "This action repeated without progress: the same calls, the same failure and unchanged files. Change approach, or say plainly what blocks it.";
/// How many identical consecutive frames make a streak by default.
pub const DEFAULT_THRESHOLD: u32 = 2;
const MESSAGE_CHARS: usize = 120;

/// SHA-256 over one frame's calls, failure and tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fingerprint(String);

impl Fingerprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn fingerprint(
    record: &CellRecord,
    error: Option<(&str, &str)>,
    tree_digest: Option<&str>,
) -> Fingerprint {
    let calls: BTreeSet<String> = record
        .calls
        .iter()
        .map(|call| {
            let mut line = call.tool.clone();
            for (key, value) in &call.args {
                line.push('\0');
                line.push_str(key);
                line.push('=');
                line.push_str(value);
            }
            line.push('\0');
            line.push_str(&match &call.ended {
                Ended::Ok => "ok".to_string(),
                Ended::Threw { class } => format!("threw:{class}"),
                Ended::Denied { rule } => format!("denied:{rule}"),
            });
            line
        })
        .collect();
    let mut hash = Sha256::new();
    for line in &calls {
        hash.update(line.as_bytes());
        hash.update([1]);
    }
    hash.update([2]);
    if let Some((class, message)) = error {
        hash.update(class.as_bytes());
        hash.update([0]);
        hash.update(
            message
                .chars()
                .take(MESSAGE_CHARS)
                .collect::<String>()
                .as_bytes(),
        );
    }
    hash.update([3]);
    hash.update(tree_digest.unwrap_or("").as_bytes());
    Fingerprint(format!("{:x}", hash.finalize()))
}

/// Counts consecutive identical frames and speaks once per streak.
#[derive(Debug, Clone)]
pub struct Guard {
    threshold: u32,
    last: Option<Fingerprint>,
    streak: u32,
    notices: u32,
}

impl Default for Guard {
    fn default() -> Self {
        Self::new(DEFAULT_THRESHOLD)
    }
}

impl Guard {
    pub fn new(threshold: u32) -> Self {
        Self {
            threshold: threshold.max(1),
            last: None,
            streak: 0,
            notices: 0,
        }
    }

    /// `Some(notice)` exactly when this frame is the `threshold`-th identical
    /// one in a row; the streak keeps counting silently after that.
    pub fn observe(&mut self, fp: Fingerprint) -> Option<String> {
        if self.last.as_ref() == Some(&fp) {
            self.streak += 1;
        } else {
            self.last = Some(fp);
            self.streak = 1;
        }
        if self.streak == self.threshold {
            self.notices += 1;
            return Some(NO_PROGRESS_NOTICE.to_string());
        }
        None
    }

    pub fn notices(&self) -> u32 {
        self.notices
    }

    /// A frame that made progress — or at least did not fail — ends the
    /// streak: two successful cells with the same shape are not a repeat.
    pub fn reset(&mut self) {
        self.last = None;
        self.streak = 0;
    }

    /// How many identical frames the current streak holds.
    pub fn streak(&self) -> u32 {
        self.streak
    }
}

/// Whether the last verified tree is still the tree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckpointStatus {
    None,
    Verified {
        cell: u64,
    },
    Disturbed {
        verified_cell: u64,
        mutation_cell: u64,
    },
}

/// The verified checkpoint: the cell and tree digest of the last passing
/// verification, and the first mutation cell that moved the tree since.
#[derive(Debug, Clone, Default)]
pub struct Checkpoints {
    verified: Option<(u64, String)>,
    disturbed_at: Option<u64>,
}

impl Checkpoints {
    /// A passing verification becomes the checkpoint; a failing one changes
    /// nothing, because the tree it ran on proved nothing.
    pub fn note_verification(&mut self, cell: u64, digest: &str, passed: bool) {
        if passed {
            self.verified = Some((cell, digest.to_string()));
            self.disturbed_at = None;
        }
    }

    /// A mutation disturbs the checkpoint when the tree digest differs from
    /// the verified one, and clears the disturbance when it returns to it.
    pub fn note_mutation(&mut self, cell: u64, digest: &str) {
        let Some((_, verified_digest)) = &self.verified else {
            return;
        };
        if verified_digest == digest {
            self.disturbed_at = None;
        } else if self.disturbed_at.is_none() {
            self.disturbed_at = Some(cell);
        }
    }

    pub fn status(&self) -> CheckpointStatus {
        match (&self.verified, self.disturbed_at) {
            (None, _) => CheckpointStatus::None,
            (Some((cell, _)), None) => CheckpointStatus::Verified { cell: *cell },
            (Some((cell, _)), Some(mutation_cell)) => CheckpointStatus::Disturbed {
                verified_cell: *cell,
                mutation_cell,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::outcome::{CallRecord, CellOutcomeKind};

    fn frame(cell: u64, command: &str, rule: &str) -> CellRecord {
        CellRecord {
            cell,
            source: format!("cell {cell}"),
            description: None,
            outcome: CellOutcomeKind::Yielded,
            handles: Vec::new(),
            calls: vec![CallRecord {
                tool: "bash".into(),
                args: [("command".to_string(), command.to_string())]
                    .into_iter()
                    .collect(),
                evidence: None,
                lifted_from: None,
                exit_code: None,
                repeat_of: None,
                error: None,
                ended: Ended::Denied { rule: rule.into() },
            }],
        }
    }

    #[test]
    fn the_fingerprint_ignores_cell_number_and_source_but_not_args_or_tree() {
        let a = fingerprint(&frame(1, "rm -rf x", "no allow"), None, Some("t"));
        let b = fingerprint(&frame(2, "rm -rf x", "no allow"), None, Some("t"));
        assert_eq!(a, b);
        assert_ne!(
            a,
            fingerprint(&frame(3, "rm -rf y", "no allow"), None, Some("t"))
        );
        assert_ne!(
            a,
            fingerprint(&frame(3, "rm -rf x", "no allow"), None, Some("u"))
        );
        assert_ne!(
            a,
            fingerprint(
                &frame(3, "rm -rf x", "no allow"),
                Some(("E", "m")),
                Some("t")
            )
        );
        let long = "x".repeat(200);
        let longer = format!("{long}y");
        assert_eq!(
            fingerprint(&frame(1, "c", "r"), Some(("E", &long)), None),
            fingerprint(&frame(1, "c", "r"), Some(("E", &longer)), None)
        );
    }

    #[test]
    fn the_guard_speaks_once_per_streak_and_counts() {
        let mut guard = Guard::default();
        let fp = fingerprint(&frame(1, "c", "r"), None, None);
        assert!(guard.observe(fp.clone()).is_none());
        assert_eq!(
            guard.observe(fp.clone()).as_deref(),
            Some(NO_PROGRESS_NOTICE)
        );
        assert!(guard.observe(fp.clone()).is_none());
        assert_eq!(guard.streak(), 3);
        let other = fingerprint(&frame(1, "d", "r"), None, None);
        assert!(guard.observe(other).is_none());
        assert!(guard.observe(fp.clone()).is_none());
        assert!(guard.observe(fp).is_some());
        assert_eq!(guard.notices(), 2);
    }

    #[test]
    fn checkpoints_move_none_verified_disturbed_and_back() {
        let mut checkpoints = Checkpoints::default();
        assert_eq!(checkpoints.status(), CheckpointStatus::None);
        checkpoints.note_verification(2, "a", false);
        assert_eq!(checkpoints.status(), CheckpointStatus::None);
        checkpoints.note_verification(3, "a", true);
        assert_eq!(checkpoints.status(), CheckpointStatus::Verified { cell: 3 });
        checkpoints.note_mutation(4, "a");
        assert_eq!(checkpoints.status(), CheckpointStatus::Verified { cell: 3 });
        checkpoints.note_mutation(5, "b");
        checkpoints.note_mutation(6, "c");
        assert_eq!(
            checkpoints.status(),
            CheckpointStatus::Disturbed {
                verified_cell: 3,
                mutation_cell: 5
            }
        );
        checkpoints.note_mutation(7, "a");
        assert_eq!(checkpoints.status(), CheckpointStatus::Verified { cell: 3 });
        checkpoints.note_mutation(8, "d");
        checkpoints.note_verification(9, "d", true);
        assert_eq!(checkpoints.status(), CheckpointStatus::Verified { cell: 9 });
    }
}

/// Cells in a row without progress before the stall notice fires.
pub const DEFAULT_STALL_WINDOW: u32 = 6;

/// Stall windows in a row, each already noticed and each followed by nothing,
/// before the task is ended.
///
/// **Three, because three is what a person would call patience**: with the
/// default window that is eighteen consecutive cells in which the tree did not
/// change, no fact was recorded and no verification result moved — after the
/// model has been told so twice and carried on regardless. Nothing here counts
/// *work*; it counts the absence of any.
pub const DEFAULT_STALL_LIMIT: u32 = 3;

/// The stall notice: a nudge with the count. The task ends only after
/// [`DEFAULT_STALL_LIMIT`] of these in a row, never on one.
#[must_use]
pub fn stall_notice(cells: u32) -> String {
    format!(
        "No progress for {cells} cells: the tree did not change, no new fact was recorded \
         and no verification result changed. Say what is blocking, change the approach, \
         or finish with what holds."
    )
}

/// Watches for a run of cells that changes nothing — no tree change, no new
/// fact, no verification — and says so once per window
/// (`smarter-cheaper-roadmap.md`, *Stall detection replaces the cell cap*).
///
/// **One stall is a notice; a run of them is the end of the task** (the user,
/// 2026-09-17: limits are dumb for abstract tasks, but a task that has stopped
/// producing anything must still stop without a person watching it). This is
/// the ender that needs no model, and it is what a session with no supervisor
/// configured falls back on. It counts nothing but the absence of progress:
/// a task writing files, recording facts or running verifications resets it
/// however long it takes, which is exactly what a cell count could not do.
#[derive(Debug, Clone)]
pub struct Stall {
    window: u32,
    since: u32,
    notices: u32,
    /// Notices since the last real progress — the streak
    /// [`Stall::stalled_windows`] reports and `session.rs` ends a task on.
    windows: u32,
}

impl Default for Stall {
    fn default() -> Self {
        Self::new(DEFAULT_STALL_WINDOW)
    }
}

impl Stall {
    pub fn new(window: u32) -> Self {
        Self {
            window: window.max(1),
            since: 0,
            notices: 0,
            windows: 0,
        }
    }

    /// `Some(notice)` on the `window`-th cell in a row without progress; the
    /// count restarts after a notice so a long stall is noticed again.
    ///
    /// Progress clears the streak as well as the count: a task that gets
    /// somewhere, however slowly, is never closer to being ended than one
    /// that has just started.
    pub fn observe(&mut self, progressed: bool) -> Option<String> {
        if progressed {
            self.since = 0;
            self.windows = 0;
            return None;
        }
        self.since += 1;
        if self.since >= self.window {
            self.since = 0;
            self.notices += 1;
            self.windows += 1;
            return Some(stall_notice(self.window));
        }
        None
    }

    /// Whole windows of nothing since the last progress. `session.rs` ends the
    /// task at [`DEFAULT_STALL_LIMIT`].
    pub fn stalled_windows(&self) -> u32 {
        self.windows
    }

    pub fn notices(&self) -> u32 {
        self.notices
    }

    /// Cells since the last progress or notice.
    pub fn since_progress(&self) -> u32 {
        self.since
    }
}

#[cfg(test)]
mod stall_tests {
    use super::*;

    /// The ender that needs no model: whole windows of nothing, in a row.
    #[test]
    fn a_run_of_empty_windows_is_what_ends_a_task_and_any_progress_clears_it() {
        let mut stall = Stall::new(3);
        assert_eq!(stall.stalled_windows(), 0);
        for _ in 0..3 {
            stall.observe(false);
        }
        assert_eq!(stall.stalled_windows(), 1, "one window of nothing");
        for _ in 0..3 {
            stall.observe(false);
        }
        assert_eq!(stall.stalled_windows(), 2);
        // A task that gets anywhere is never closer to being ended than one
        // that has just started.
        stall.observe(true);
        assert_eq!(stall.stalled_windows(), 0, "progress clears the streak");
        for _ in 0..(3 * DEFAULT_STALL_LIMIT) {
            stall.observe(false);
        }
        assert_eq!(stall.stalled_windows(), DEFAULT_STALL_LIMIT);
    }

    #[test]
    fn a_stall_is_noticed_on_the_window_and_progress_resets_it() {
        let mut stall = Stall::new(3);
        assert!(stall.observe(false).is_none());
        assert!(stall.observe(false).is_none());
        assert!(stall.observe(true).is_none(), "progress resets the count");
        assert!(stall.observe(false).is_none());
        assert!(stall.observe(false).is_none());
        let notice = stall.observe(false).expect("the third idle cell in a row");
        assert!(notice.contains("No progress for 3 cells"), "{notice}");
        assert_eq!(stall.notices(), 1);
        assert_eq!(
            stall.since_progress(),
            0,
            "the count restarts after a notice"
        );
        assert!(stall.observe(false).is_none());
        assert!(stall.observe(false).is_none());
        assert!(
            stall.observe(false).is_some(),
            "a long stall is noticed again"
        );
        assert_eq!(stall.notices(), 2);
    }
}
