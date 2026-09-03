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
mod tests;
