//! Portable session checkpoints.
//!
//! A checkpoint is what one session hands the next when work has to move:
//! what it was trying to do, where it got to, what it already ruled out, and
//! what to do next. Small enough that reading it is cheap, plain enough that
//! any harness can be given it.
//!
//! # What this deliberately is not
//!
//! Phase 19's fixed architectural requirement: *Glasshouse checkpoints
//! contain portable Glasshouse metadata and bounded handoff context. They do
//! not attempt to clone or replace proprietary native harness session
//! formats.*
//!
//! So there is no field here for a transcript, a message history, a native
//! session identifier, a tool-call log, or a token count. Those are the
//! harness's own business and it keeps them itself; a Glasshouse checkpoint
//! that tried to hold them would be a worse copy of a file that already
//! exists, and it would stop being portable the moment two harnesses
//! disagreed about the shape. `the_format_holds_no_native_session_state`
//! pins the document's field list so that a field of that kind cannot arrive
//! by accident.
//!
//! The `harness` field is the exception that proves the rule: it records
//! *which* harness produced the checkpoint, which is portable metadata about
//! provenance, and says nothing whatever about that harness's format.
//!
//! # Small is a constraint, not an aspiration
//!
//! *Keep checkpoints deliberately small enough to bootstrap a fresh session
//! cheaply.* A handoff document that costs as much to read as the work it
//! describes has no reason to exist, so the bound is enforced three times
//! over, deliberately:
//!
//! 1. [`Checkpoint::fit`] trims the least load-bearing content until the
//!    rendered document fits, and records that it did;
//! 2. [`CheckpointStore::save`] calls `fit` itself, so a caller cannot skip
//!    step 1;
//! 3. the `checkpoints` table carries a `CHECK` on the document's byte
//!    length, so nothing that opens the file can store an oversized one
//!    either.
//!
//! # Where a checkpoint's *content* comes from
//!
//! Glasshouse fills in what it can know by itself — the session, the harness,
//! the timestamp, and the Git position read straight off the repository. It
//! does **not** invent the objective, the state, or the next actions. Those
//! are authored, by whoever asks for the checkpoint, and a checkpoint whose
//! objective Glasshouse had guessed from terminal output would be exactly the
//! kind of confident fiction this project refuses everywhere else.

pub mod git;
pub mod store;

pub use git::{GitPosition, WorkingTreeStatus};
pub use store::{CheckpointId, CheckpointStore, ProjectCheckpoints, StoreError, Stored};

use serde::{Deserialize, Serialize};

use crate::session::SessionId;

/// The largest a rendered checkpoint may be, in bytes.
///
/// Eight kibibytes is roughly two thousand tokens: enough for a real handoff,
/// cheap enough that bootstrapping a session with one is not a decision
/// anybody has to think about.
///
/// Defined once, in `crate::database` beside the schema that enforces it, and
/// re-exported here — so the module and the `CHECK` constraint cannot drift.
/// The SQL literal itself is held to this number by
/// `the_schema_enforces_exactly_the_documented_bound`, which inserts a
/// document of exactly this size and one byte more.
pub const MAX_BYTES: usize = crate::database::MAX_CHECKPOINT_BYTES;

/// The longest any single authored string may be before it is cut.
///
/// A per-field cap as well as a total one, because one runaway paragraph
/// should not be able to evict every other section on its own.
pub const MAX_FIELD_BYTES: usize = 1024;

/// The document format's own version.
///
/// Present in every rendered checkpoint so that a reader can tell what it is
/// looking at without guessing from the field names — a format that is
/// meant to travel between tools needs to say what it is.
pub const FORMAT_VERSION: u32 = 1;

/// Why a checkpoint was taken.
///
/// Recorded because the two are read differently: a checkpoint the user asked
/// for marks a moment they chose, and one taken at a task boundary is the
/// most recent automatic snapshot. A single flag would have made "is this the
/// one I asked for?" unanswerable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointReason {
    /// The user asked for it.
    Manual,
    /// Glasshouse took it because a turn ended.
    TaskBoundary,
}

impl CheckpointReason {
    /// The value stored in SQLite; the schema's `CHECK` lists exactly these.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::TaskBoundary => "task_boundary",
        }
    }

    /// Read back a value this enum stored.
    ///
    /// Named `from_stored` rather than `from_str` deliberately: it parses one
    /// of exactly two spellings this module wrote, not arbitrary text, and a
    /// `from_str` on a public type is read as `std::str::FromStr` by every
    /// tool and most readers. That is a different and much wider promise.
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "manual" => Some(Self::Manual),
            "task_boundary" => Some(Self::TaskBoundary),
            _ => None,
        }
    }
}

impl std::fmt::Display for CheckpointReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// The authored half of a checkpoint: what only a person or an agent doing
/// the work can supply.
///
/// Separate from [`Checkpoint`] because the split is the honest one. Every
/// field here is something Glasshouse cannot derive; everything a
/// [`Checkpoint`] adds is something it can read for itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Handoff {
    /// What this work is trying to achieve. Required by the map in every
    /// checkpoint.
    pub objective: String,
    /// Where it has got to. Required by the map in every checkpoint.
    pub implementation_state: String,
    /// Decisions discovered during this task, when there are any.
    pub decisions: Vec<String>,
    /// The project's current binding memory records — invariants, constraints
    /// and decisions Phase 21A classifies as rules rather than context — at
    /// the moment this checkpoint was taken. Line 1641: a fresh session gets
    /// these without having to query memory for itself before its first turn.
    pub memory: Vec<String>,
    /// Approaches already tried and abandoned, so the next session does not
    /// spend its first hour repeating them.
    pub failed_approaches: Vec<String>,
    /// Files and symbols that matter to this work.
    pub files: Vec<String>,
    /// What the tests currently say.
    pub test_state: Option<String>,
    /// What to do next, in order.
    pub next_actions: Vec<String>,
}

/// One portable checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// The Glasshouse session this describes.
    pub session: SessionId,
    /// The harness that was running it, as an integration slug. Provenance,
    /// not format — see the module doc.
    pub harness: String,
    pub reason: CheckpointReason,
    /// Seconds since the Unix epoch.
    pub created_at: i64,
    /// The repository position, when the project is a Git repository at all.
    pub git: Option<GitPosition>,
    /// Whether the working tree holds changes the index does not, when that
    /// is knowable at all — see [`WorkingTreeStatus`] for exactly what this
    /// does and does not compare. Independent of `git`: a `None` here says
    /// nothing about whether `git` is present, and vice versa.
    pub working_tree: Option<WorkingTreeStatus>,
    pub handoff: Handoff,
    /// Whether [`Checkpoint::fit`] had to drop anything to meet the bound.
    ///
    /// Reported rather than hidden, for the same reason a subscriber says how
    /// many events it lost: a reader deciding whether to go and look at the
    /// original should be told that there is one.
    pub trimmed: bool,
}

/// The document as it is written down and read back.
///
/// Named fields, no harness anywhere in the shape, and a version. Optional
/// sections are omitted rather than written empty, which is what keeps a
/// minimal checkpoint genuinely small.
#[derive(Debug, Serialize, Deserialize)]
struct Document {
    version: u32,
    session: String,
    harness: String,
    reason: String,
    created_at: i64,
    objective: String,
    implementation_state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    decisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    memory: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    failed_approaches: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    test_state: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    next_actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_commit: Option<String>,
    // `git_dirty` is the discriminator for whether a working-tree status was
    // recorded at all: `None` means unavailable, exactly as an absent `git`
    // position means unavailable, and `Some(false)` is a real, present
    // "clean" that must round-trip rather than being treated as empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_dirty: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    git_changed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    trimmed: bool,
}

/// Why a stored document could not be read back as a checkpoint.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("a checkpoint document could not be parsed")]
    Malformed {
        #[source]
        source: serde_json::Error,
    },
    #[error("a checkpoint document declares format version {found}; this build reads {supported}")]
    Version { found: u32, supported: u32 },
    #[error("a checkpoint document records an unrecognized reason `{value}`")]
    Reason { value: String },
}

impl Checkpoint {
    /// A checkpoint for one session, with everything Glasshouse can read for
    /// itself already filled in.
    ///
    /// The Git position is read straight off the repository — see
    /// [`git::GitPosition::detect`], which opens two small files and spawns
    /// nothing. `None` means the project is not a Git repository, or its HEAD
    /// could not be read; a checkpoint is still worth having without one, so
    /// this never fails.
    pub fn capture(
        session: &SessionId,
        harness: &str,
        reason: CheckpointReason,
        created_at: i64,
        project_root: &std::path::Path,
        handoff: Handoff,
    ) -> Self {
        Self {
            session: session.clone(),
            harness: harness.to_owned(),
            reason,
            created_at,
            git: GitPosition::detect(project_root),
            working_tree: WorkingTreeStatus::detect(project_root),
            handoff,
            trimmed: false,
        }
    }

    /// The portable document, as JSON.
    pub fn render(&self) -> String {
        // `to_string_pretty` cannot fail for this shape — every field is a
        // string, an integer, a bool, or a list of strings — but a rendering
        // that somehow did must not panic on a path that runs while a user is
        // holding a live session. An empty document is refused by the reader
        // and by the schema, so the failure stays visible.
        serde_json::to_string_pretty(&self.document()).unwrap_or_default()
    }

    /// Read a rendered document back.
    pub fn parse(document: &str) -> Result<Self, FormatError> {
        let parsed: Document =
            serde_json::from_str(document).map_err(|source| FormatError::Malformed { source })?;
        if parsed.version != FORMAT_VERSION {
            return Err(FormatError::Version {
                found: parsed.version,
                supported: FORMAT_VERSION,
            });
        }
        let reason =
            CheckpointReason::from_stored(&parsed.reason).ok_or_else(|| FormatError::Reason {
                value: parsed.reason.clone(),
            })?;
        Ok(Self {
            session: SessionId::new(parsed.session),
            harness: parsed.harness,
            reason,
            created_at: parsed.created_at,
            git: GitPosition::from_parts(parsed.git_branch, parsed.git_commit),
            working_tree: parsed.git_dirty.map(|dirty| WorkingTreeStatus {
                dirty,
                changed_files: parsed.git_changed_files,
            }),
            handoff: Handoff {
                objective: parsed.objective,
                implementation_state: parsed.implementation_state,
                decisions: parsed.decisions,
                memory: parsed.memory,
                failed_approaches: parsed.failed_approaches,
                files: parsed.files,
                test_state: parsed.test_state,
                next_actions: parsed.next_actions,
            },
            trimmed: parsed.trimmed,
        })
    }

    fn document(&self) -> Document {
        Document {
            version: FORMAT_VERSION,
            session: self.session.as_str().to_owned(),
            harness: self.harness.clone(),
            reason: self.reason.as_str().to_owned(),
            created_at: self.created_at,
            objective: self.handoff.objective.clone(),
            implementation_state: self.handoff.implementation_state.clone(),
            decisions: self.handoff.decisions.clone(),
            memory: self.handoff.memory.clone(),
            failed_approaches: self.handoff.failed_approaches.clone(),
            files: self.handoff.files.clone(),
            test_state: self.handoff.test_state.clone(),
            next_actions: self.handoff.next_actions.clone(),
            git_branch: self.git.as_ref().and_then(|git| git.branch.clone()),
            git_commit: self.git.as_ref().map(|git| git.commit.clone()),
            git_dirty: self.working_tree.as_ref().map(|status| status.dirty),
            git_changed_files: self
                .working_tree
                .as_ref()
                .map(|status| status.changed_files.clone())
                .unwrap_or_default(),
            trimmed: self.trimmed,
        }
    }

    /// Trim until the rendered document fits [`MAX_BYTES`], and say whether
    /// anything had to go.
    ///
    /// # The order is the design
    ///
    /// Sections are given up least-useful-first, and "useful" means *useful
    /// to somebody starting from nothing*: the objective and the current
    /// state are what a fresh session cannot work without, so they are cut
    /// last and truncated rather than dropped. Failed approaches go first —
    /// they are the largest section in practice and the least costly to
    /// rediscover — then the working tree's changed-file list (Glasshouse
    /// read it off disk and can read it again), then the file list, then
    /// decisions, then **project memory**, then the test state, then next
    /// actions.
    ///
    /// Project memory sheds right after decisions and before the test state:
    /// it is never disposable in the way a failed approach or a changed-file
    /// list is — a binding record is a constraint the next session must not
    /// silently violate, not a convenience this session happened to jot down
    /// — but the test state and next actions are what tells a fresh session
    /// where to resume at all, and those outrank everything else that can be
    /// given up.
    ///
    /// Nothing is ever silently perfect: whatever is dropped sets `trimmed`,
    /// so a reader knows to go and look at the session itself.
    pub fn fit(mut self) -> Self {
        // Per-field caps first, so one runaway paragraph cannot evict every
        // other section by itself.
        clamp(&mut self.handoff.objective, &mut self.trimmed);
        clamp(&mut self.handoff.implementation_state, &mut self.trimmed);
        if let Some(tests) = self.handoff.test_state.as_mut() {
            clamp(tests, &mut self.trimmed);
        }
        for list in [
            &mut self.handoff.decisions,
            &mut self.handoff.memory,
            &mut self.handoff.failed_approaches,
            &mut self.handoff.files,
            &mut self.handoff.next_actions,
        ] {
            for item in list.iter_mut() {
                clamp(item, &mut self.trimmed);
            }
        }
        if let Some(status) = self.working_tree.as_mut() {
            for item in status.changed_files.iter_mut() {
                clamp(item, &mut self.trimmed);
            }
        }

        // Then whole sections, least load-bearing first.
        while self.render().len() > MAX_BYTES {
            let over = self.render().len() - MAX_BYTES;
            // Shed at least the overshoot in one pass. Dropping a single item
            // and re-rendering would be quadratic in the number of items that
            // have to go — a subcontractor's first draft of the size-bound
            // test spent 57 seconds inside this loop and said so, which is
            // exactly the sort of thing a test written from outside catches.
            if self.shed(over) {
                self.trimmed = true;
                continue;
            }
            // Nothing optional is left, so the two required fields are what
            // is over the bound. Halve the longer of the two and go round
            // again; the loop terminates because every pass removes bytes and
            // `truncate_bytes` bottoms out at the empty string.
            let longer = if self.handoff.implementation_state.len() >= self.handoff.objective.len()
            {
                &mut self.handoff.implementation_state
            } else {
                &mut self.handoff.objective
            };
            if longer.is_empty() {
                // Both required fields are empty and the document is *still*
                // over the bound, which can only mean the metadata alone
                // exceeds it. Nothing further can be given up here; the
                // schema's own `CHECK` is the backstop, and it refuses.
                break;
            }
            let half = longer.len() / 2;
            truncate_bytes(longer, half);
            self.trimmed = true;
        }
        self
    }

    /// Plain text a fresh session in **any** harness can be given.
    ///
    /// Prose rather than the JSON document on purpose: this is what gets
    /// typed into a harness, and every harness reads prose. It names no
    /// harness, no protocol and no tool, which is what makes a checkpoint
    /// written by one able to start work in another.
    pub fn bootstrap_prompt(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();

        let _ = writeln!(
            out,
            "You are continuing work that another session started. Everything below is \
             the handoff it left."
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "OBJECTIVE");
        let _ = writeln!(out, "{}", self.handoff.objective);
        let _ = writeln!(out);
        let _ = writeln!(out, "CURRENT STATE");
        let _ = writeln!(out, "{}", self.handoff.implementation_state);

        if self.git.is_some() || self.working_tree.is_some() {
            let _ = writeln!(out);
            let _ = writeln!(out, "REPOSITORY");
        }
        if let Some(git) = &self.git {
            match &git.branch {
                Some(branch) => {
                    let _ = writeln!(out, "branch {branch}, commit {}", git.commit);
                }
                None => {
                    let _ = writeln!(out, "detached HEAD at commit {}", git.commit);
                }
            }
        }
        if let Some(status) = &self.working_tree {
            if status.dirty {
                // `changed_files` is capped (`WorkingTreeStatus::detect`'s
                // own doc), so its length is the true count only when the
                // cap was not reached — otherwise there is more, unnamed.
                let count = if status.changed_files.len() < git::MAX_CHANGED_FILES {
                    status.changed_files.len().to_string()
                } else {
                    format!("at least {}", status.changed_files.len())
                };
                let _ = writeln!(
                    out,
                    "working tree: dirty ({count} file{} changed — run `git diff` to see them)",
                    if status.changed_files.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                );
                for file in &status.changed_files {
                    let _ = writeln!(out, "  {file}");
                }
            } else {
                let _ = writeln!(out, "working tree: clean");
            }
        }

        section(&mut out, "DECISIONS ALREADY MADE", &self.handoff.decisions);
        section(&mut out, "RELEVANT MEMORY", &self.handoff.memory);
        section(
            &mut out,
            "APPROACHES ALREADY TRIED AND ABANDONED — do not repeat these",
            &self.handoff.failed_approaches,
        );
        section(&mut out, "IMPORTANT FILES AND SYMBOLS", &self.handoff.files);

        if let Some(tests) = &self.handoff.test_state {
            let _ = writeln!(out, "\nTEST STATE\n{tests}");
        }

        if !self.handoff.next_actions.is_empty() {
            let _ = writeln!(out, "\nNEXT ACTIONS");
            for (index, action) in self.handoff.next_actions.iter().enumerate() {
                let _ = writeln!(out, "{}. {action}", index + 1);
            }
        }

        if self.trimmed {
            let _ = writeln!(
                out,
                "\n(This handoff was trimmed to fit a size bound; the session it came \
                 from has more.)"
            );
        }
        out
    }
}

impl Checkpoint {
    /// Give up at least `bytes` of optional content, least load-bearing
    /// first. Returns whether anything was given up at all.
    ///
    /// The order is [`Checkpoint::fit`]'s and the reasoning is there. What is
    /// here is only the accounting: each dropped item is charged its own
    /// length plus a small constant for the quoting, comma and indentation
    /// the document spends on it, which is an estimate — so the caller
    /// re-renders and goes round again rather than trusting it.
    fn shed(&mut self, bytes: usize) -> bool {
        /// What the document spends on an item beyond its own text: two
        /// quotes, a comma, a newline and the indentation.
        const PER_ITEM: usize = 8;

        let mut freed = 0usize;
        let mut dropped = false;
        while freed < bytes {
            let item = self
                .handoff
                .failed_approaches
                .pop()
                .or_else(|| {
                    self.working_tree
                        .as_mut()
                        .and_then(|status| status.changed_files.pop())
                })
                .or_else(|| self.handoff.files.pop())
                .or_else(|| self.handoff.decisions.pop())
                // Project memory sheds only once decisions are gone: a
                // binding record is a constraint on the next session, not
                // disposable context, so it outlives everything shed above it
                // — but the test state and next actions below still matter
                // more, because those are what tell a fresh session where to
                // resume at all.
                .or_else(|| self.handoff.memory.pop())
                .or_else(|| self.handoff.test_state.take())
                .or_else(|| self.handoff.next_actions.pop());
            let Some(item) = item else {
                // Nothing optional is left. Whether anything went at all is
                // what the caller needs to know.
                return dropped;
            };
            freed += item.len() + PER_ITEM;
            dropped = true;
        }
        dropped
    }
}

fn section(out: &mut String, title: &str, items: &[String]) {
    use std::fmt::Write as _;
    if items.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n{title}");
    for item in items {
        let _ = writeln!(out, "- {item}");
    }
}

fn clamp(value: &mut String, trimmed: &mut bool) {
    if value.len() > MAX_FIELD_BYTES {
        truncate_bytes(value, MAX_FIELD_BYTES);
        *trimmed = true;
    }
}

/// Cut `value` to at most `bytes`, at a character boundary.
///
/// Walks *back* to a boundary, so the result is never longer than asked for
/// and never invalid — the mirror image of
/// `crate::session::api`'s `tail`, which walks forward for the same reason
/// from the other end.
fn truncate_bytes(value: &mut String, bytes: usize) {
    if value.len() <= bytes {
        return;
    }
    let mut cut = bytes;
    while cut > 0 && !value.is_char_boundary(cut) {
        cut -= 1;
    }
    value.truncate(cut);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handoff() -> Handoff {
        Handoff {
            objective: "close Phase 19".to_owned(),
            implementation_state: "the format exists; storage is next".to_owned(),
            decisions: vec!["JSON, versioned".to_owned()],
            memory: vec!["the project never stores secrets in checkpoints".to_owned()],
            failed_approaches: vec!["cloning the harness session file".to_owned()],
            files: vec!["checkpoint/mod.rs::Checkpoint".to_owned()],
            test_state: Some("973 passing".to_owned()),
            next_actions: vec!["write the store".to_owned()],
        }
    }

    fn checkpoint() -> Checkpoint {
        Checkpoint {
            session: SessionId::new("abc123"),
            harness: "a-harness".to_owned(),
            reason: CheckpointReason::Manual,
            created_at: 1_700_000_000,
            git: Some(GitPosition {
                branch: Some("main".to_owned()),
                commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            }),
            working_tree: Some(WorkingTreeStatus {
                dirty: true,
                changed_files: vec!["src/checkpoint/mod.rs".to_owned()],
            }),
            handoff: handoff(),
            trimmed: false,
        }
    }

    /// Everything the map asks a checkpoint to include survives a round trip
    /// through the document, including the two that are required in every
    /// checkpoint and the six that appear only when present.
    #[test]
    fn every_documented_field_survives_a_round_trip() {
        let original = checkpoint();
        let parsed = Checkpoint::parse(&original.render()).unwrap();
        assert_eq!(parsed, original);
    }

    /// An absent section is absent from the document rather than present and
    /// empty, which is what keeps a minimal checkpoint genuinely small.
    #[test]
    fn a_minimal_checkpoint_writes_only_what_it_has() {
        let minimal = Checkpoint {
            handoff: Handoff {
                objective: "o".to_owned(),
                implementation_state: "s".to_owned(),
                ..Handoff::default()
            },
            git: None,
            working_tree: None,
            ..checkpoint()
        };
        let rendered = minimal.render();
        for absent in [
            "decisions",
            "memory",
            "failed_approaches",
            "files",
            "test_state",
            "next_actions",
            "git_branch",
            "git_commit",
            "git_dirty",
            "git_changed_files",
            "trimmed",
        ] {
            assert!(
                !rendered.contains(absent),
                "an empty `{absent}` was written anyway:\n{rendered}"
            );
        }
        assert_eq!(Checkpoint::parse(&rendered).unwrap(), minimal);
    }

    /// The fixed architectural requirement, as a test over the format itself:
    /// a checkpoint carries portable metadata and bounded handoff context,
    /// and nothing that would make it a copy of a harness's own session file.
    #[test]
    fn the_format_holds_no_native_session_state() {
        let document: serde_json::Value = serde_json::from_str(&checkpoint().render()).unwrap();
        let fields: std::collections::BTreeSet<String> = document
            .as_object()
            .expect("the document is an object")
            .keys()
            .cloned()
            .collect();

        let expected: std::collections::BTreeSet<String> = [
            "version",
            "session",
            "harness",
            "reason",
            "created_at",
            "objective",
            "implementation_state",
            "decisions",
            "memory",
            "failed_approaches",
            "files",
            "test_state",
            "next_actions",
            "git_branch",
            "git_commit",
            "git_dirty",
            "git_changed_files",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();

        assert_eq!(
            fields, expected,
            "the checkpoint format changed; a new field must be portable handoff \
             context, never a copy of a harness's own session state"
        );

        // And the names a native-format clone would need are not among them.
        for forbidden in [
            "transcript",
            "messages",
            "history",
            "native_session_id",
            "tool_calls",
            "conversation",
            "tokens",
        ] {
            assert!(
                !fields.contains(forbidden),
                "`{forbidden}` belongs to a harness's own session file, not to a \
                 portable checkpoint"
            );
        }
    }

    /// The size bound holds against content that ignores it entirely, and the
    /// checkpoint says it was cut rather than pretending to be complete.
    #[test]
    fn an_enormous_handoff_is_trimmed_to_the_bound_and_says_so() {
        let huge = Checkpoint {
            handoff: Handoff {
                objective: "o".repeat(50_000),
                implementation_state: "s".repeat(50_000),
                decisions: (0..500)
                    .map(|i| format!("decision {i} {}", "d".repeat(200)))
                    .collect(),
                memory: (0..500)
                    .map(|i| format!("memory {i} {}", "m".repeat(200)))
                    .collect(),
                failed_approaches: (0..500).map(|i| format!("failed {i}")).collect(),
                files: (0..500).map(|i| format!("file{i}.rs")).collect(),
                test_state: Some("t".repeat(50_000)),
                next_actions: (0..500).map(|i| format!("next {i}")).collect(),
            },
            ..checkpoint()
        }
        .fit();

        assert!(
            huge.render().len() <= MAX_BYTES,
            "a trimmed checkpoint is still {} bytes",
            huge.render().len()
        );
        assert!(huge.trimmed, "trimming must be reported, never silent");
        // What survives is what a fresh session cannot start without.
        assert!(!huge.handoff.objective.is_empty());
        assert!(!huge.handoff.implementation_state.is_empty());
        // And it is still a valid document.
        assert_eq!(Checkpoint::parse(&huge.render()).unwrap(), huge);
    }

    /// Trimming gives up the least load-bearing sections first. A checkpoint
    /// that dropped its objective while keeping five hundred failed
    /// approaches would meet the bound and be useless.
    #[test]
    fn trimming_gives_up_failed_approaches_before_anything_a_session_needs() {
        let trimmed = Checkpoint {
            handoff: Handoff {
                objective: "the objective".to_owned(),
                implementation_state: "the state".to_owned(),
                failed_approaches: (0..2000).map(|i| format!("failed approach {i}")).collect(),
                next_actions: vec!["do the thing".to_owned()],
                ..Handoff::default()
            },
            ..checkpoint()
        }
        .fit();

        assert!(trimmed.render().len() <= MAX_BYTES);
        assert_eq!(trimmed.handoff.objective, "the objective");
        assert_eq!(trimmed.handoff.implementation_state, "the state");
        assert_eq!(
            trimmed.handoff.next_actions,
            vec!["do the thing".to_owned()],
            "next actions must outlive failed approaches"
        );
        assert!(
            trimmed.handoff.failed_approaches.len() < 2000,
            "nothing was given up at all"
        );
    }

    /// Project memory is a constraint, not disposable context: with enough
    /// failed approaches, changed files and decisions to cover the whole
    /// overshoot by themselves, a single memory record is left completely
    /// untouched — the wrong shed order would have reached into it instead
    /// and left some of the less-protected content behind.
    #[test]
    fn trimming_protects_project_memory_while_less_protected_content_remains() {
        let trimmed = Checkpoint {
            handoff: Handoff {
                objective: "the objective".to_owned(),
                implementation_state: "the state".to_owned(),
                failed_approaches: (0..2000).map(|i| format!("failed approach {i}")).collect(),
                files: (0..2000).map(|i| format!("some/file/{i}.rs")).collect(),
                decisions: (0..2000).map(|i| format!("decision {i}")).collect(),
                memory: vec!["a binding constraint that must not be lost lightly".to_owned()],
                test_state: Some("6 of 6 passing".to_owned()),
                next_actions: vec!["do the thing".to_owned()],
            },
            ..checkpoint()
        }
        .fit();

        assert!(trimmed.render().len() <= MAX_BYTES);
        assert!(
            trimmed.handoff.decisions.len() < 2000,
            "nothing was given up at all"
        );
        assert_eq!(
            trimmed.handoff.test_state,
            Some("6 of 6 passing".to_owned()),
            "test state must outlive project memory"
        );
        assert_eq!(
            trimmed.handoff.next_actions,
            vec!["do the thing".to_owned()],
            "next actions must outlive project memory"
        );
        assert_eq!(
            trimmed.handoff.memory,
            vec!["a binding constraint that must not be lost lightly".to_owned()],
            "memory must not be shed while there was still plenty of less-protected \
             content to give up first"
        );
    }

    /// A checkpoint that already fits is left exactly alone, and does not
    /// claim to have been trimmed.
    #[test]
    fn a_small_checkpoint_is_not_touched() {
        let original = checkpoint();
        let fitted = original.clone().fit();
        assert_eq!(fitted, original);
        assert!(!fitted.trimmed);
    }

    /// Truncation never splits a character. A multi-byte objective cut at a
    /// byte offset would produce a document `serde_json` cannot even write.
    #[test]
    fn truncation_never_produces_invalid_text() {
        for len in 0..40usize {
            let mut value = "héllo wörld ✅".repeat(4);
            truncate_bytes(&mut value, len);
            assert!(value.len() <= len);
            assert!(std::str::from_utf8(value.as_bytes()).is_ok());
        }
    }

    /// Line 1641: a project's binding memory reaches the prompt under its own
    /// heading when there is any, and the heading itself is absent — not
    /// present and empty — when a project has no binding memory at all.
    #[test]
    fn the_bootstrap_prompt_includes_relevant_memory_when_present_and_omits_the_section_when_absent()
     {
        let with_memory = checkpoint().bootstrap_prompt();
        assert!(with_memory.contains("RELEVANT MEMORY"));
        assert!(with_memory.contains("the project never stores secrets in checkpoints"));

        let mut without_memory = checkpoint();
        without_memory.handoff.memory.clear();
        let prompt = without_memory.bootstrap_prompt();
        assert!(
            !prompt.contains("RELEVANT MEMORY"),
            "a project with no binding memory must render no section at all:\n{prompt}"
        );
    }

    /// The bootstrap prompt is the artifact that crosses harnesses, so it
    /// must name none of them, and it must carry the sections a fresh session
    /// needs.
    #[test]
    fn the_bootstrap_prompt_is_plain_text_that_names_no_harness() {
        let prompt = checkpoint().bootstrap_prompt();

        assert!(prompt.contains("close Phase 19"));
        assert!(prompt.contains("the format exists; storage is next"));
        assert!(prompt.contains("cloning the harness session file"));
        assert!(prompt.contains("1. write the store"));
        assert!(prompt.contains("branch main"));

        let lowered = prompt.to_ascii_lowercase();
        for named in [
            "claude",
            "codex",
            "antigravity",
            "opencode",
            "cursor",
            "gemini",
        ] {
            assert!(
                !lowered.contains(named),
                "the bootstrap prompt names `{named}`, so it is not portable"
            );
        }
        // Not JSON: this is what gets typed into a harness.
        assert!(!prompt.trim_start().starts_with('{'));
    }

    /// A trimmed checkpoint says so in the prompt too. A session told to
    /// continue from an incomplete handoff should know it is incomplete.
    #[test]
    fn a_trimmed_checkpoint_admits_it_in_the_prompt() {
        let mut trimmed = checkpoint();
        assert!(!trimmed.bootstrap_prompt().contains("trimmed"));
        trimmed.trimmed = true;
        assert!(trimmed.bootstrap_prompt().contains("trimmed"));
    }

    /// A document from a version this build does not read is refused rather
    /// than parsed hopefully — the same rule the project database applies to
    /// a schema version it has never heard of.
    #[test]
    fn a_document_from_another_format_version_is_refused() {
        let rendered = checkpoint()
            .render()
            .replace("\"version\": 1", "\"version\": 99");
        let error = Checkpoint::parse(&rendered).expect_err("version 99 must be refused");
        assert!(
            matches!(error, FormatError::Version { found: 99, .. }),
            "got {error:?}"
        );
    }

    #[test]
    fn every_reason_round_trips_through_its_stored_spelling() {
        for reason in [CheckpointReason::Manual, CheckpointReason::TaskBoundary] {
            assert_eq!(CheckpointReason::from_stored(reason.as_str()), Some(reason));
        }
        assert_eq!(CheckpointReason::from_stored("automatic"), None);
    }
}
