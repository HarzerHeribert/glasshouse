//! The harness-agnostic tool result — map line 1980. Every adapter converts
//! into this shape before any reduction runs, so the ladder, the raw store,
//! and the provenance header never see one harness's own JSON.

/// One tool call's result, normalized out of whatever the harness sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub tool_name: String,
    pub payload: ToolPayload,
}

/// The textual shape a normalized result can take.
///
/// `Command`'s `stdout` is the only field reduction may ever touch — exit
/// status, stderr, and interruption state are carried alongside it and must
/// survive untouched (map line 1990).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolPayload {
    /// A single block of text with no separate exit/error signal — Grep,
    /// Glob, Read, and any other tool the adapter recognizes as one text
    /// blob.
    Text(String),
    /// A shell command's result, with the fields box 1990 requires kept
    /// apart from the reducible content.
    Command {
        stdout: String,
        stderr: String,
        interrupted: bool,
        /// `Some(n)` only when the harness reported it explicitly; `None`
        /// means this build has no positive signal either way, which
        /// [`crate::firewall::eligibility`] treats as "cannot guarantee",
        /// not as success.
        exit_code: Option<i64>,
    },
}

impl ToolPayload {
    /// The bytes reduction candidates are drawn from — `stdout` for a
    /// command, the whole blob for plain text.
    pub fn reducible_text(&self) -> &str {
        match self {
            ToolPayload::Text(text) => text,
            ToolPayload::Command { stdout, .. } => stdout,
        }
    }

    /// Whether this command result positively confirms a clean, uninterrupted
    /// exit. `Text` has no such concept and answers `true` — it carries
    /// nothing box 1990 needs to preserve.
    pub fn confirmed_clean_exit(&self) -> bool {
        match self {
            ToolPayload::Text(_) => true,
            ToolPayload::Command {
                interrupted,
                exit_code,
                ..
            } => !*interrupted && *exit_code == Some(0),
        }
    }
}
