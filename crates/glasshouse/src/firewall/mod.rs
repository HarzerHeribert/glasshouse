//! The context firewall's harness-agnostic core — Phase 57's first package
//! (map lines 1980-1990) plus the semantic reducer (Phase 57B, map lines
//! 1997-2003). Normalize, run the deterministic half of the ladder, run the
//! optional semantic half over its retained candidates, preserve raw bytes,
//! annotate with provenance, and hand back everything the hook subcommand
//! needs for telemetry and its response.
//!
//! Every architectural question here is answered in
//! `docs/product/design-decisions.md` §Phase 57 — this module implements
//! it rather than re-deciding it. The Claude Code JSON shape stays confined
//! to [`adapter`], and the semantic stage is routed entirely through
//! [`reducer::Reducer`] — this module knows nothing about
//! `DisposableRouting`, a `JobKind`, or a provider; the binary's own
//! `main.rs` builds whatever [`reducer::Reducer`] is configured and hands
//! it in.

pub mod adapter;
pub mod eligibility;
pub mod estimate;
pub mod model;
pub mod provenance;
pub mod reduce;
pub mod reducer;
pub mod store;

pub use model::{ToolPayload, ToolResult};
pub use store::{RawEntry, RawStore, WindowSavings};

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

/// Everything [`process`] needs to decide whether the semantic stage runs at
/// all, and what to ask it — Phase 57B, map lines 1997-2003. This module
/// never builds a [`reducer::Reducer`] itself; the caller (`crate::main`)
/// resolves configuration and disposable routing into one, or passes `None`.
pub struct SemanticContext<'a> {
    pub mode: crate::config::firewall::FirewallMode,
    /// `None` disables the whole semantic stage in every mode — map line
    /// 1992's guarantee, restated for the reducer field: an absent reducer
    /// is not a special case `process` has to remember, it is simply
    /// nothing to call.
    pub reducer: Option<&'a dyn reducer::Reducer>,
    pub task: &'a str,
    pub tool_query: Option<&'a str>,
    /// File paths named in the tool's own input, for the privacy gate (map
    /// line 2003). Never inspected for anything else.
    pub file_paths: &'a [String],
    pub min_semantic_tokens: u64,
    pub aggressive_drops_uncertain: bool,
}

impl<'a> SemanticContext<'a> {
    /// No reducer, no gate ever opens — used by every caller that has not
    /// configured one, including every existing test that predates this
    /// package.
    pub fn disabled() -> Self {
        Self {
            mode: crate::config::firewall::FirewallMode::Off,
            reducer: None,
            task: "",
            tool_query: None,
            file_paths: &[],
            min_semantic_tokens: crate::config::firewall::DEFAULT_MIN_SEMANTIC_TOKENS,
            aggressive_drops_uncertain: false,
        }
    }
}

/// Why the semantic stage did not apply its reducer's answer — map line
/// 2001's exact vocabulary, plus the two structural cases (privacy, no
/// resource) that are not reducer failures but still leave the deterministic
/// result standing unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticBypassReason {
    TimedOut,
    Transport,
    RateLimited,
    Refused,
    Unavailable,
    Schema,
    Validation,
    Failed,
    /// The local reducer's own executable could not be started at all —
    /// Phase 58, map line 2029's `local-reducer-absent`.
    LocalAbsent,
    /// The local reducer did not answer inside its configured timeout —
    /// map line 2029's `local-reducer-timeout`.
    LocalTimeout,
    /// The local reducer exited non-zero, or its reply was not the local
    /// contract's shape — map line 2029's `local-reducer-failed`.
    LocalFailed,
    /// The local reducer's reported `tool_version` does not prefix-match
    /// the configured pin — map line 2029's `local-reducer-version`.
    LocalVersion,
}

impl SemanticBypassReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TimedOut => "reducer-timed-out",
            Self::Transport => "reducer-transport",
            Self::RateLimited => "reducer-rate-limited",
            Self::Refused => "reducer-refused",
            Self::Unavailable => "reducer-unavailable",
            Self::Schema => "reducer-schema",
            Self::Validation => "reducer-validation",
            Self::Failed => "reducer-failed",
            Self::LocalAbsent => "local-reducer-absent",
            Self::LocalTimeout => "local-reducer-timeout",
            Self::LocalFailed => "local-reducer-failed",
            Self::LocalVersion => "local-reducer-version",
        }
    }
}

impl From<reducer::ReducerErrorKind> for SemanticBypassReason {
    fn from(kind: reducer::ReducerErrorKind) -> Self {
        match kind {
            reducer::ReducerErrorKind::TimedOut => Self::TimedOut,
            reducer::ReducerErrorKind::Transport => Self::Transport,
            reducer::ReducerErrorKind::RateLimited => Self::RateLimited,
            reducer::ReducerErrorKind::Refused => Self::Refused,
            reducer::ReducerErrorKind::Unavailable => Self::Unavailable,
            reducer::ReducerErrorKind::Schema => Self::Schema,
            reducer::ReducerErrorKind::Validation => Self::Validation,
            reducer::ReducerErrorKind::Failed(_) => Self::Failed,
            reducer::ReducerErrorKind::LocalAbsent => Self::LocalAbsent,
            reducer::ReducerErrorKind::LocalTimeout => Self::LocalTimeout,
            reducer::ReducerErrorKind::LocalFailed => Self::LocalFailed,
            reducer::ReducerErrorKind::LocalVersion => Self::LocalVersion,
        }
    }
}

/// What the evidence ledger should know about a reducer call, whether it was
/// applied or ultimately bypassed — map line 1987's second half: the
/// provider, model, route and provider-reported token counts, exactly as
/// [`reducer::ReducerCallInfo`] carries them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCallInfo {
    pub provider: String,
    pub model: String,
    pub route: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
}

impl From<reducer::ReducerCallInfo> for SemanticCallInfo {
    fn from(call: reducer::ReducerCallInfo) -> Self {
        Self {
            provider: call.provider,
            model: call.model,
            route: call.route,
            input_tokens: call.input_tokens,
            output_tokens: call.output_tokens,
            cached_input_tokens: call.cached_input_tokens,
        }
    }
}

/// What the semantic stage actually did — present only when it was
/// *attempted* (the gate opened: safe/aggressive mode, a reducer configured,
/// the deterministic result above the minimum, and the privacy gate clear).
/// A `None` on [`Outcome::Reduced::semantic`] means the stage was never
/// asked, which is not a failure and gets no bypass telemetry row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticOutcome {
    pub applied: bool,
    /// `Some` exactly when `applied` is `false`.
    pub reason: Option<SemanticBypassReason>,
    /// `Some` whenever a real call was dispatched — on success always, and
    /// on a schema or validation failure whenever the call itself
    /// completed and returned a parseable reply (map line 1987's "reducer
    /// calls are real model calls").
    pub call: Option<SemanticCallInfo>,
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
        /// The semantic stage's own outcome — Phase 57B, map lines
        /// 1997-2003. `None` when it was never attempted (no reducer
        /// configured, mode not safe/aggressive, the deterministic result
        /// at or below `--min-semantic-tokens`, or the privacy gate
        /// blocked it): the deterministic reduction above is then the
        /// whole of what happened, exactly as it was before this package.
        semantic: Option<SemanticOutcome>,
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
/// the optional semantic stage, raw preservation, and provenance.
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
    semantic: &SemanticContext<'_>,
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
    // Map line 2005's shadow-comparison record, populated here because this
    // is the one point in the pipeline that has the deterministic ladder's
    // own answer but has not yet run the optional semantic stage — see
    // `RawEntry::forwarded_token_estimate`'s doc comment for why it is
    // this stage's number and not the (possibly further-reduced) final one.
    let entry = RawEntry {
        session_id: session_id.to_string(),
        tool_use_id: tool_use_id.to_string(),
        tool: tool_name.to_string(),
        timestamp_unix,
        content: original_text.to_string(),
        original_token_estimate: original_tokens,
        forwarded_token_estimate: Some(estimate::estimate_tokens(&reduction.forwarded)),
        retained_candidates: Some(reduction.retained_candidates),
        total_candidates: Some(reduction.total_candidates),
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

    // Phase 57B, map lines 1997-2003: the semantic stage runs only when
    // every gate opens, and is otherwise simply never asked — `None`, not a
    // failure. Order matters: the privacy gate (2003) is checked before the
    // reducer is ever called, per this package's own security invariant.
    let semantic_gate_open = semantic.reducer.is_some()
        && matches!(
            semantic.mode,
            crate::config::firewall::FirewallMode::Safe
                | crate::config::firewall::FirewallMode::Aggressive
        )
        && estimate::estimate_tokens(&reduction.forwarded) > semantic.min_semantic_tokens
        && !reducer::privacy_blocks_reduction(semantic.file_paths);

    let mut semantic_forwarded: Option<String> = None;
    let mut semantic_outcome: Option<SemanticOutcome> = None;
    let mut semantic_kept: Option<usize> = None;
    if semantic_gate_open {
        // `semantic_gate_open` only when `semantic.reducer.is_some()`.
        let active_reducer = semantic.reducer.expect("checked by semantic_gate_open");
        let request = reducer::ReductionRequest {
            task: semantic.task,
            tool_name,
            tool_query: semantic.tool_query,
            candidates: &reduction.candidates,
        };
        match active_reducer.select(&request) {
            Ok(answer) => {
                let keep = reducer::decide_keep_set(
                    &answer.verdicts,
                    &reduction.candidates,
                    semantic.mode == crate::config::firewall::FirewallMode::Aggressive,
                    semantic.aggressive_drops_uncertain,
                );
                semantic_kept = Some(keep.len());
                semantic_forwarded = Some(reduce::rebuild(&reduction.candidates, &keep));
                semantic_outcome = Some(SemanticOutcome {
                    applied: true,
                    reason: None,
                    call: Some(answer.call.into()),
                });
            }
            Err(err) => {
                tracing::warn!(
                    tool = tool_name,
                    reason = %err,
                    "context firewall: the semantic reducer failed; the deterministic result \
                     forwards unchanged"
                );
                semantic_outcome = Some(SemanticOutcome {
                    applied: false,
                    reason: Some(err.kind.into()),
                    call: err.call.map(|boxed| (*boxed).into()),
                });
            }
        }
    }
    let ladder_forwarded = semantic_forwarded
        .as_deref()
        .unwrap_or(&reduction.forwarded);

    // Only `stdout` is ever a reduction candidate; a command's stderr
    // survives untouched and clearly separated (map line 1990).
    let reduced_body = match &result.payload {
        ToolPayload::Command { stderr, .. } if !stderr.is_empty() => {
            format!(
                "{ladder_forwarded}\n[glasshouse context firewall: stderr, preserved \
                 untouched]\n{stderr}"
            )
        }
        _ => ladder_forwarded.to_owned(),
    };
    let forwarded_tokens = estimate::estimate_tokens(&reduced_body);
    let provenance = provenance::Provenance {
        original_tokens,
        forwarded_tokens,
        retained_candidates: reduction.retained_candidates,
        total_candidates: reduction.total_candidates,
        raw_ref: raw_ref.clone(),
        semantic: semantic_outcome
            .as_ref()
            .map(|outcome| provenance::SemanticProvenance {
                applied: outcome.applied,
                kept: semantic_kept.unwrap_or(0),
                considered: reduction.candidates.len(),
                reason: outcome.reason.map(|reason| reason.as_str().to_owned()),
                reducer: outcome
                    .call
                    .as_ref()
                    .map(|call| format!("{} {}", call.provider, call.model)),
            }),
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
        semantic: semantic_outcome,
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
            &SemanticContext::disabled(),
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
            &SemanticContext::disabled(),
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
            &SemanticContext::disabled(),
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
            &SemanticContext::disabled(),
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
            &SemanticContext::disabled(),
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
            &SemanticContext::disabled(),
        );
        match outcome {
            Outcome::Reduced { forwarded_text, .. } => {
                assert!(forwarded_text.contains("a real warning that must survive"));
            }
            other => panic!("expected Reduced, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Phase 57B: the semantic stage, map lines 1997-2003.
    // -----------------------------------------------------------------

    /// A [`reducer::Reducer`] whose answer is a plain function of the
    /// request — no network, no disposable routing, so these tests are
    /// about `process`'s own gating and rebuild logic, not about a real
    /// call (that is `firewall::reducer`'s and `tests/firewall_reducer.rs`'s
    /// job).
    struct FakeReducer<F>(F);

    impl<F> reducer::Reducer for FakeReducer<F>
    where
        F: Fn(
            &reducer::ReductionRequest<'_>,
        ) -> Result<reducer::ReducerAnswer, reducer::ReducerError>,
    {
        fn describe(&self) -> String {
            "fake reducer".to_string()
        }

        fn select(
            &self,
            request: &reducer::ReductionRequest<'_>,
        ) -> Result<reducer::ReducerAnswer, reducer::ReducerError> {
            (self.0)(request)
        }
    }

    fn fake_call() -> reducer::ReducerCallInfo {
        reducer::ReducerCallInfo {
            provider: "fixture-provider".to_string(),
            model: "fixture-model".to_string(),
            route: Some("openai-chat".to_string()),
            input_tokens: Some(100),
            output_tokens: Some(20),
            cached_input_tokens: None,
        }
    }

    /// A needle among thousands of duplicate hits, oversized enough to
    /// cross `min_semantic_tokens` after the deterministic ladder — the
    /// package's flagship recall fixture, first half: the reducer marks the
    /// needle `uncertain`, and safe mode forwards it anyway.
    fn needle_fixture() -> (String, &'static str) {
        let mut text = String::new();
        for _ in 0..2000 {
            text.push_str(
                "distinct unique noise line that is long enough to cross the token minimum\n",
            );
        }
        text.push_str("THE-ONE-RELEVANT-NEEDLE-LINE\n");
        (text, "THE-ONE-RELEVANT-NEEDLE-LINE")
    }

    #[test]
    fn safe_mode_forwards_a_needle_the_reducer_marked_only_uncertain() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(10, Vec::new());
        let (text, needle) = needle_fixture();
        let result = ToolResult {
            tool_name: "Grep".to_string(),
            payload: ToolPayload::Text(text),
        };

        let reducer_impl = FakeReducer(|request: &reducer::ReductionRequest<'_>| {
            let verdicts = request
                .candidates
                .iter()
                .map(|c| reducer::Verdict {
                    id: c.id,
                    relevance: if c.text.contains("NEEDLE") {
                        reducer::Relevance::Uncertain
                    } else {
                        reducer::Relevance::Discard
                    },
                    reason: "fixture".to_string(),
                })
                .collect();
            Ok(reducer::ReducerAnswer {
                verdicts,
                call: fake_call(),
            })
        });

        let semantic = SemanticContext {
            mode: crate::config::firewall::FirewallMode::Safe,
            reducer: Some(&reducer_impl),
            task: "",
            tool_query: None,
            file_paths: &[],
            min_semantic_tokens: 10,
            aggressive_drops_uncertain: false,
        };

        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Grep",
            Some(result),
            &semantic,
        );

        match outcome {
            Outcome::Reduced {
                forwarded_text,
                semantic,
                ..
            } => {
                assert!(
                    forwarded_text.contains(needle),
                    "safe mode must forward an uncertain candidate: {forwarded_text}"
                );
                let semantic = semantic.expect("the gate must have opened");
                assert!(semantic.applied);
                assert!(semantic.call.is_some());
            }
            other => panic!("expected Reduced, got {other:?}"),
        }
    }

    /// The flagship fixture's second, honest half: when the reducer
    /// outright discards the one relevant candidate, the rebuilt forwarded
    /// text drops it, the provenance header says so — and (proven in
    /// `tests/firewall_reducer.rs`, through the shipped binary) `show <id>`
    /// still has it, because the raw store is written before the semantic
    /// stage ever runs.
    #[test]
    fn a_candidate_the_reducer_discards_is_dropped_and_the_header_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(10, Vec::new());
        let (text, needle) = needle_fixture();
        let result = ToolResult {
            tool_name: "Grep".to_string(),
            payload: ToolPayload::Text(text),
        };

        let reducer_impl = FakeReducer(|request: &reducer::ReductionRequest<'_>| {
            let verdicts = request
                .candidates
                .iter()
                .map(|c| reducer::Verdict {
                    id: c.id,
                    relevance: if c.text.contains("NEEDLE") {
                        reducer::Relevance::Discard
                    } else {
                        reducer::Relevance::Relevant
                    },
                    reason: "fixture".to_string(),
                })
                .collect();
            Ok(reducer::ReducerAnswer {
                verdicts,
                call: fake_call(),
            })
        });

        let semantic = SemanticContext {
            mode: crate::config::firewall::FirewallMode::Safe,
            reducer: Some(&reducer_impl),
            task: "",
            tool_query: None,
            file_paths: &[],
            min_semantic_tokens: 10,
            aggressive_drops_uncertain: false,
        };

        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Grep",
            Some(result),
            &semantic,
        );

        match outcome {
            Outcome::Reduced {
                forwarded_text,
                semantic,
                ..
            } => {
                assert!(
                    !forwarded_text.contains(needle),
                    "an explicitly discarded candidate must not survive: {forwarded_text}"
                );
                let semantic = semantic.expect("the gate must have opened");
                assert!(semantic.applied);
                assert!(
                    forwarded_text.contains(
                        "semantic reduction by fixture-provider \
                                              fixture-model kept"
                    ),
                    "the header must say the semantic stage ran, naming the reducer that \
                     produced the reduction: {forwarded_text}"
                );
            }
            other => panic!("expected Reduced, got {other:?}"),
        }
    }

    /// Map line 2001: fail open on every reducer failure. The reducer times
    /// out; the deterministic result forwards exactly as if no reducer were
    /// configured, and never an empty result.
    #[test]
    fn a_timed_out_reducer_fails_open_to_the_deterministic_result() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(10, Vec::new());
        let (text, needle) = needle_fixture();
        let result = ToolResult {
            tool_name: "Grep".to_string(),
            payload: ToolPayload::Text(text),
        };

        let reducer_impl = FakeReducer(|_: &reducer::ReductionRequest<'_>| {
            Err(reducer::ReducerError {
                kind: reducer::ReducerErrorKind::TimedOut,
                call: None,
            })
        });

        let semantic = SemanticContext {
            mode: crate::config::firewall::FirewallMode::Safe,
            reducer: Some(&reducer_impl),
            task: "",
            tool_query: None,
            file_paths: &[],
            min_semantic_tokens: 10,
            aggressive_drops_uncertain: false,
        };

        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Grep",
            Some(result),
            &semantic,
        );

        match outcome {
            Outcome::Reduced {
                forwarded_text,
                semantic,
                ..
            } => {
                assert!(!forwarded_text.is_empty());
                assert!(
                    forwarded_text.contains(needle),
                    "a failed reducer must never lose the deterministic result: {forwarded_text}"
                );
                let semantic = semantic.expect("the gate opened, so the attempt is recorded");
                assert!(!semantic.applied);
                assert_eq!(semantic.reason, Some(SemanticBypassReason::TimedOut));
                assert!(
                    semantic.call.is_none(),
                    "a timeout never reaches a parsed reply"
                );
            }
            other => panic!("expected Reduced, got {other:?}"),
        }
    }

    /// Below `min_semantic_tokens`, the semantic stage is never asked at
    /// all — not a failure, simply not attempted.
    #[test]
    fn the_semantic_stage_never_runs_below_the_minimum() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(10, Vec::new());
        let result = ToolResult {
            tool_name: "Grep".to_string(),
            payload: ToolPayload::Text("line one\nline two\nline three\n".repeat(3)),
        };

        let reducer_impl = FakeReducer(|_: &reducer::ReductionRequest<'_>| {
            panic!("the reducer must never be called below the minimum")
        });

        let semantic = SemanticContext {
            mode: crate::config::firewall::FirewallMode::Safe,
            reducer: Some(&reducer_impl),
            task: "",
            tool_query: None,
            file_paths: &[],
            min_semantic_tokens: 1_000_000,
            aggressive_drops_uncertain: false,
        };

        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Grep",
            Some(result),
            &semantic,
        );

        match outcome {
            Outcome::Reduced { semantic, .. } => {
                assert!(semantic.is_none());
            }
            other => panic!("expected Reduced, got {other:?}"),
        }
    }

    /// Map line 2003: a `.env`-shaped path in the tool's own file metadata
    /// suppresses semantic reduction for this result entirely, before any
    /// candidate would have left the process.
    #[test]
    fn a_secret_shaped_path_suppresses_semantic_reduction() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(10, Vec::new());
        let (text, _needle) = needle_fixture();
        let result = ToolResult {
            tool_name: "Read".to_string(),
            payload: ToolPayload::Text(text),
        };

        let reducer_impl = FakeReducer(|_: &reducer::ReductionRequest<'_>| {
            panic!("the reducer must never be called against a secret-shaped path")
        });

        let paths = vec![".env".to_string()];
        let semantic = SemanticContext {
            mode: crate::config::firewall::FirewallMode::Safe,
            reducer: Some(&reducer_impl),
            task: "",
            tool_query: None,
            file_paths: &paths,
            min_semantic_tokens: 10,
            aggressive_drops_uncertain: false,
        };

        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Read",
            Some(result),
            &semantic,
        );

        match outcome {
            Outcome::Reduced { semantic, .. } => {
                assert!(semantic.is_none());
            }
            other => panic!("expected Reduced, got {other:?}"),
        }
    }

    /// With no reducer configured, `process` behaves exactly as batch 72
    /// left it — [`SemanticContext::disabled`] is what every pre-existing
    /// test above already uses; this test states the guarantee by name.
    #[test]
    fn no_reducer_configured_leaves_semantic_outcome_absent() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(10, Vec::new());
        let (text, _needle) = needle_fixture();
        let result = ToolResult {
            tool_name: "Grep".to_string(),
            payload: ToolPayload::Text(text),
        };

        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Grep",
            Some(result),
            &SemanticContext::disabled(),
        );

        match outcome {
            Outcome::Reduced { semantic, .. } => assert!(semantic.is_none()),
            other => panic!("expected Reduced, got {other:?}"),
        }
    }

    /// Map line 1999's containment guarantee, one stage further than
    /// [`reduce`]'s own: every line the semantic stage forwards is a
    /// verbatim slice of the ORIGINAL text, never anything the reducer's
    /// own words (a `reason`, or any other generated string) could have
    /// contributed. The rebuild reads ids only — see [`reduce::rebuild`].
    #[test]
    fn semantic_reduction_never_introduces_text_absent_from_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let config = FirewallConfig::new(10, Vec::new());
        let (text, _needle) = needle_fixture();
        let result = ToolResult {
            tool_name: "Grep".to_string(),
            payload: ToolPayload::Text(text.clone()),
        };

        let reducer_impl = FakeReducer(|request: &reducer::ReductionRequest<'_>| {
            let verdicts = request
                .candidates
                .iter()
                .map(|c| reducer::Verdict {
                    id: c.id,
                    relevance: reducer::Relevance::Relevant,
                    // Deliberately generated text a mutant might leak into
                    // the forwarded result — never a substring of `text`.
                    reason: "GENERATED-TEXT-THAT-MUST-NEVER-APPEAR-IN-OUTPUT".to_string(),
                })
                .collect();
            Ok(reducer::ReducerAnswer {
                verdicts,
                call: fake_call(),
            })
        });

        let semantic = SemanticContext {
            mode: crate::config::firewall::FirewallMode::Safe,
            reducer: Some(&reducer_impl),
            task: "",
            tool_query: None,
            file_paths: &[],
            min_semantic_tokens: 10,
            aggressive_drops_uncertain: false,
        };

        let outcome = process(
            &store(dir.path()),
            &config,
            "session-1",
            "tu-1",
            1_700_000_000,
            "Grep",
            Some(result),
            &semantic,
        );

        match outcome {
            Outcome::Reduced { forwarded_text, .. } => {
                assert!(
                    !forwarded_text.contains("GENERATED-TEXT"),
                    "the reducer's own words must never reach the forwarded result: \
                     {forwarded_text}"
                );
                for line in forwarded_text.lines() {
                    if line.starts_with("[glasshouse context firewall") {
                        continue;
                    }
                    assert!(
                        text.contains(line),
                        "forwarded line `{line}` is not a verbatim slice of the original"
                    );
                }
            }
            other => panic!("expected Reduced, got {other:?}"),
        }
    }
}
