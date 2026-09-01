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
        /// `Some(n)` only when the harness reported it explicitly.
        ///
        /// GH-FIREWALL-BRIDGE's real capture (installed Claude Code
        /// 2.1.252, `PostToolUse` fired against a live Bash tool call,
        /// documented in that package's report) settled what `None` means
        /// here: a **successful** Bash call's real `tool_response` is
        /// `{"stdout", "stderr", "interrupted", "isImage",
        /// "noOutputExpected"}` — no `exit_code` key at all — and a
        /// **failing** one never reaches `PostToolUse` in the first place;
        /// it fires `PostToolUseFailure` instead, with an entirely
        /// different shape (`error`, `is_interrupt`) this adapter does not
        /// subscribe to. So `exit_code: None` is the ordinary, expected
        /// shape of every real success this build's own hook registration
        /// ever sees, not an unknown to be conservative about — the
        /// event's own arrival on `PostToolUse` is the positive exit
        /// signal. An explicit non-zero value, if some future shape ever
        /// sends one, still refuses reduction; see
        /// [`ToolPayload::confirmed_clean_exit`].
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
    /// exit.
    ///
    /// `Text` has no such concept and answers `true` — it carries nothing
    /// box 1990 needs to preserve. For `Command`, an explicit non-zero
    /// `exit_code` always refuses; a missing one does not, per
    /// [`ToolPayload::Command::exit_code`]'s doc — the real harness never
    /// sends this adapter a failing Bash result to begin with, so treating
    /// its absence as "cannot guarantee" would keep Bash unreducible
    /// forever regardless of how conservative the rest of the ladder is.
    pub fn confirmed_clean_exit(&self) -> bool {
        match self {
            ToolPayload::Text(_) => true,
            ToolPayload::Command {
                interrupted,
                exit_code,
                ..
            } => !*interrupted && !matches!(exit_code, Some(code) if *code != 0),
        }
    }
}
