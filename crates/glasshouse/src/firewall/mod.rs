//! The context firewall's harness-agnostic core — Phase 57's first package,
//! map lines 1980-1990. Normalize, run the deterministic half of the
//! ladder, preserve raw bytes, annotate with provenance, and hand back
//! everything the hook subcommand needs for telemetry and its response.
//!
//! Every architectural question here is answered in
//! `docs/product/design-decisions.md` §Phase 57 — this module implements
//! it rather than re-deciding it. In particular: no semantic reduction, no
//! settings-file config, and the Claude Code JSON shape stays confined to
//! [`adapter`].

pub mod adapter;
pub mod eligibility;
pub mod estimate;
pub mod model;
pub mod provenance;
pub mod reduce;
pub mod store;

pub use model::{ToolPayload, ToolResult};
pub use store::{RawEntry, RawStore};

/// One hook invocation's thresholds and eligibility list — carried entirely
/// on the subcommand's own flags, per the ruling that the registered
/// command line is the config carrier (map line 1981).
#[derive(Debug, Clone)]
pub struct FirewallConfig {
    pub passthrough_tokens: u64,
    pub eligible_tools: Vec<String>,
}

impl FirewallConfig {
    /// `tools` is the `--tool` flag's raw values; an empty list resolves to
    /// [`eligibility::DEFAULT_ELIGIBLE_TOOLS`], never to "nothing eligible".
    pub fn new(passthrough_tokens: u64, tools: Vec<String>) -> Self {
        let eligible_tools = if tools.is_empty() {
            eligibility::DEFAULT_ELIGIBLE_TOOLS
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            tools
        };
        Self {
            passthrough_tokens,
            eligible_tools,
        }
    }
}

/// Why a result passed through untouched instead of being reduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BypassReason {
    /// Not on the eligible-tools list, or hard-blocked regardless of it
    /// (map line 1989).
    IneligibleTool,
    /// The adapter could not positively recognize the response shape (map
    /// lines 1989, 1990).
    UnknownShape,
    /// A command result whose exit status this build cannot positively
    /// confirm as clean (map line 1990's fallback).
    UnconfirmedExit,
    /// The raw store could not be written; fail open rather than block or
    /// silently drop the result (evidence ledger's "fail open, never
    /// empty").
    StoreWriteFailed,
}

impl BypassReason {
    pub fn as_str(self) -> &'static str {
        match self {
            BypassReason::IneligibleTool => "ineligible-tool",
            BypassReason::UnknownShape => "unknown-shape",
            BypassReason::UnconfirmedExit => "unconfirmed-exit",
            BypassReason::StoreWriteFailed => "store-write-failed",
        }
    }
}

/// What processing one tool result decided. Carries everything the caller
/// needs to build the hook response and record telemetry — no further
/// inspection of intermediate state is needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Below the passthrough threshold: untouched, header-free (map line
    /// 1981).
    Passthrough { tool_name: String, tokens: u64 },
    /// Above the threshold and reduced (map line 1982).
    Reduced {
        tool_name: String,
        original_tokens: u64,
        forwarded_tokens: u64,
        retained_candidates: usize,
        total_candidates: usize,
        raw_ref: String,
        /// The provenance header plus the reduced body — and, for a
        /// command result, its untouched stderr — ready to hand to the
        /// harness as `updatedToolOutput`.
        forwarded_text: String,
    },
    Bypass {
        tool_name: String,
        reason: BypassReason,
    },
}

impl Outcome {
    pub fn tool_name(&self) -> &str {
        match self {
            Outcome::Passthrough { tool_name, .. }
            | Outcome::Reduced { tool_name, .. }
            | Outcome::Bypass { tool_name, .. } => tool_name,
        }
    }
}

/// Run one tool result through the whole core: eligibility, the ladder,
/// deterministic reduction, raw preservation, and provenance.
///
/// `normalized` is [`adapter::normalize`]'s own answer — `None` when the
/// adapter did not positively recognize the response shape, which this
/// function treats identically to any other unrecognized shape.
#[allow(clippy::too_many_arguments)]
pub fn process(
    store: &RawStore,
    config: &FirewallConfig,
    session_id: &str,
    tool_use_id: &str,
    timestamp_unix: i64,
    tool_name: &str,
    normalized: Option<ToolResult>,
) -> Outcome {
    let Some(result) = normalized else {
        return Outcome::Bypass {
            tool_name: tool_name.to_string(),
            reason: BypassReason::UnknownShape,
        };
    };

    if !eligibility::is_eligible(tool_name, &config.eligible_tools) {
        return Outcome::Bypass {
            tool_name: tool_name.to_string(),
            reason: BypassReason::IneligibleTool,
        };
    }

    if !result.payload.confirmed_clean_exit() {
        return Outcome::Bypass {
            tool_name: tool_name.to_string(),
            reason: BypassReason::UnconfirmedExit,
        };
    }

    let original_text = result.payload.reducible_text();
    let original_tokens = estimate::estimate_tokens(original_text);

    if original_tokens <= config.passthrough_tokens {
        return Outcome::Passthrough {
            tool_name: tool_name.to_string(),
            tokens: original_tokens,
        };
    }

    let reduction = reduce::reduce(original_text);
    let entry = RawEntry {
        session_id: session_id.to_string(),
        tool_use_id: tool_use_id.to_string(),
        tool: tool_name.to_string(),
        timestamp_unix,
        content: original_text.to_string(),
        original_token_estimate: original_tokens,
    };
    let raw_ref = match store.write(&entry) {
        Ok(reference) => reference,
        Err(err) => {
            tracing::warn!(
                error = %err,
                tool = tool_name,
                "context firewall: raw store write failed; the result passes through unreduced"
            );
            return Outcome::Bypass {
                tool_name: tool_name.to_string(),
                reason: BypassReason::StoreWriteFailed,
            };
        }
    };

    // Only `stdout` is ever a reduction candidate; a command's stderr
    // survives untouched and clearly separated (map line 1990).
    let reduced_body = match &result.payload {
        ToolPayload::Command { stderr, .. } if !stderr.is_empty() => {
            format!(
                "{}\n[glasshouse context firewall: stderr, preserved untouched]\n{stderr}",
                reduction.forwarded
            )
        }
        _ => reduction.forwarded.clone(),
    };
    let forwarded_tokens = estimate::estimate_tokens(&reduced_body);
    let provenance = provenance::Provenance {
        original_tokens,
        forwarded_tokens,
        retained_candidates: reduction.retained_candidates,
        total_candidates: reduction.total_candidates,
        raw_ref: raw_ref.clone(),
    };
    let forwarded_text = provenance.prepend_to(&reduced_body);

    Outcome::Reduced {
        tool_name: tool_name.to_string(),
        original_tokens,
        forwarded_tokens,
        retained_candidates: reduction.retained_candidates,
        total_candidates: reduction.total_candidates,
        raw_ref,
        forwarded_text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &std::path::Path) -> RawStore {
        RawStore::open(dir)
    }

    #[test]
    fn a_small_text_result_passes_through_below_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(4000, Vec::new());
        let result = ToolResult {
            tool_name: "Grep".to_string(),
            payload: ToolPayload::Text("a few short lines\n".to_string()),
        };
        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Grep",
            Some(result),
        );
        assert!(matches!(outcome, Outcome::Passthrough { .. }));
    }

    #[test]
    fn an_oversized_eligible_result_is_reduced_and_stored() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(10, Vec::new());
        let big =
            "distinct unique line here that is long enough to cross ten tokens easily\n".repeat(3);
        let result = ToolResult {
            tool_name: "Read".to_string(),
            payload: ToolPayload::Text(big),
        };
        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Read",
            Some(result),
        );
        match outcome {
            Outcome::Reduced {
                raw_ref,
                forwarded_text,
                ..
            } => {
                assert!(raw_ref.starts_with(store::REFERENCE_PREFIX));
                assert!(forwarded_text.contains("glasshouse context firewall"));
            }
            other => panic!("expected Reduced, got {other:?}"),
        }
    }

    #[test]
    fn edit_is_never_eligible_even_when_named_on_the_tools_flag() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(1, vec!["Edit".to_string()]);
        let result = ToolResult {
            tool_name: "Edit".to_string(),
            payload: ToolPayload::Text("a diff summary\n".repeat(20)),
        };
        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Edit",
            Some(result),
        );
        assert_eq!(
            outcome,
            Outcome::Bypass {
                tool_name: "Edit".to_string(),
                reason: BypassReason::IneligibleTool
            }
        );
    }

    #[test]
    fn an_unnormalized_result_bypasses_as_unknown_shape() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(1, Vec::new());
        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "SomeMcpTool",
            None,
        );
        assert_eq!(
            outcome,
            Outcome::Bypass {
                tool_name: "SomeMcpTool".to_string(),
                reason: BypassReason::UnknownShape
            }
        );
    }

    #[test]
    fn a_non_zero_exit_bash_result_is_never_reduced() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(1, Vec::new());
        let result = ToolResult {
            tool_name: "Bash".to_string(),
            payload: ToolPayload::Command {
                stdout: "line\n".repeat(50),
                stderr: "boom\n".to_string(),
                interrupted: false,
                exit_code: Some(1),
            },
        };
        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Bash",
            Some(result),
        );
        assert_eq!(
            outcome,
            Outcome::Bypass {
                tool_name: "Bash".to_string(),
                reason: BypassReason::UnconfirmedExit
            }
        );
    }

    #[test]
    fn a_reduced_bash_result_keeps_stderr_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(1, Vec::new());
        let mut stdout = String::new();
        for i in 0..50 {
            stdout.push_str(&format!("build step {i} ok\n"));
        }
        let result = ToolResult {
            tool_name: "Bash".to_string(),
            payload: ToolPayload::Command {
                stdout,
                stderr: "a real warning that must survive\n".to_string(),
                interrupted: false,
                exit_code: Some(0),
            },
        };
        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Bash",
            Some(result),
        );
        match outcome {
            Outcome::Reduced { forwarded_text, .. } => {
                assert!(forwarded_text.contains("a real warning that must survive"));
            }
            other => panic!("expected Reduced, got {other:?}"),
        }
    }
}
