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

/// Whether `tool_name` is eligible for reduction under `configured` (the
/// resolved `--tools` list; pass [`DEFAULT_ELIGIBLE_TOOLS`] when the flag
/// named nothing).
pub fn is_eligible(tool_name: &str, configured: &[String]) -> bool {
    if is_hard_blocked(tool_name) {
        return false;
    }
    configured.iter().any(|t| t.eq_ignore_ascii_case(tool_name))
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
}
