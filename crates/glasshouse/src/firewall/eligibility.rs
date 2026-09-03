//! Tool eligibility — map line 1989.
//!
//! Two lists, not one: a configurable allowlist a `--tools` flag may widen
//! or narrow, and a hard block that no flag can override. The hard block is
//! checked first and always wins.

/// Eligible by default when `--tools` names nothing: search, read, and
/// log-shaped outputs, per the packet's own list.
pub const DEFAULT_ELIGIBLE_TOOLS: &[&str] = &["Grep", "Glob", "Read", "Bash"];

/// Never eligible, regardless of `--tools` — edits, writes, and anything the
/// map calls permission- or security-shaped.
const HARD_BLOCKED_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// The names [`is_writing_tool`] answers `true` for, for the one caller that
/// needs the *list* rather than the question:
/// `harness::claude_code::edit_intent_tool_matcher` builds a Claude Code
/// hook matcher from it, so the coordination hook is spawned for exactly the
/// tools that can change a file and for no others.
///
/// An alias rather than a second list — a fifth editing tool is still added
/// in exactly one place, and the matcher cannot drift from the predicate.
pub const WRITING_TOOLS: &[&str] = HARD_BLOCKED_TOOLS;

/// Whether `tool_name` is eligible for reduction under `configured` (the
/// resolved `--tools` list; pass [`DEFAULT_ELIGIBLE_TOOLS`] when the flag
/// named nothing).
pub fn is_eligible(tool_name: &str, configured: &[String]) -> bool {
    if is_hard_blocked(tool_name) {
        return false;
    }
    configured.iter().any(|t| t.eq_ignore_ascii_case(tool_name))
}

/// Whether `tool_name` is a tool that **changes a file** — the four
/// `HARD_BLOCKED_TOOLS` name, and map line 1139's producer gate.
///
/// # Why this reads the block list rather than a list of its own
///
/// The two questions have different words and one answer. Reduction refuses
/// these tools because their results are not the kind of output a reducer may
/// touch; recording refuses everything *else* because *touched* has to mean
/// the session changed the file. A second list would be a second place to
/// remember a fifth editing tool, and the failure mode of forgetting is
/// silent in both directions — a reduction that mangles an edit's result, or
/// a `referenced` association earned by a tool nobody meant to count.
///
/// Note what this deliberately does **not** inherit from
/// `is_hard_blocked`: the `permission`/`security` substring rule. A tool
/// named `SecurityScan` must never be reduced and does not edit anything, so
/// folding that rule in here would record a file it merely read.
///
/// Case-insensitive, exactly as [`is_eligible`] is, because the harness's
/// spelling of a tool name is not something this build should depend on.
pub fn is_writing_tool(tool_name: &str) -> bool {
    HARD_BLOCKED_TOOLS
        .iter()
        .any(|t| t.eq_ignore_ascii_case(tool_name))
}

/// A block no `--tools` flag can lift: the tool's own name says edit/write,
/// or names permission/security explicitly. This is a name-based guard, not
/// a content inspection — the response-shape check in
/// [`crate::firewall::model::ToolPayload`] is the other half of "never
/// eligible" for results this build cannot positively classify.
fn is_hard_blocked(tool_name: &str) -> bool {
    if HARD_BLOCKED_TOOLS
        .iter()
        .any(|t| t.eq_ignore_ascii_case(tool_name))
    {
        return true;
    }
    let lower = tool_name.to_ascii_lowercase();
    lower.contains("permission") || lower.contains("security")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Vec<String> {
        DEFAULT_ELIGIBLE_TOOLS
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn default_list_admits_the_named_tools() {
        for tool in DEFAULT_ELIGIBLE_TOOLS {
            assert!(is_eligible(tool, &defaults()), "{tool}");
        }
    }

    #[test]
    fn edit_and_write_are_blocked_even_when_named_explicitly() {
        let configured = vec!["Edit".to_string(), "Write".to_string()];
        assert!(!is_eligible("Edit", &configured));
        assert!(!is_eligible("Write", &configured));
    }

    #[test]
    fn a_permission_shaped_tool_name_is_blocked_regardless_of_flags() {
        let configured = vec!["PermissionPrompt".to_string()];
        assert!(!is_eligible("PermissionPrompt", &configured));
    }

    #[test]
    fn an_unnamed_tool_is_ineligible_under_a_narrowed_list() {
        let configured = vec!["Grep".to_string()];
        assert!(!is_eligible("Read", &configured));
    }

    #[test]
    fn every_hard_blocked_editing_tool_is_a_writing_tool_whatever_its_case() {
        for tool in HARD_BLOCKED_TOOLS {
            assert!(is_writing_tool(tool), "{tool}");
            assert!(is_writing_tool(&tool.to_ascii_lowercase()), "{tool}");
            assert!(is_writing_tool(&tool.to_ascii_uppercase()), "{tool}");
        }
    }

    /// The distinction map line 1139 rests on: *touched* means the session
    /// changed the file. A tool that reads one is not a producer of that
    /// fact, and neither is a tool blocked from reduction for the unrelated
    /// `permission`/`security` reason.
    #[test]
    fn a_read_shaped_or_security_shaped_tool_is_not_a_writing_tool() {
        for tool in [
            "Read",
            "Grep",
            "Glob",
            "Bash",
            "SecurityScan",
            "PermissionPrompt",
        ] {
            assert!(!is_writing_tool(tool), "{tool}");
        }
    }
}
