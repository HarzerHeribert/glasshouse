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
    F: Fn(&reducer::ReductionRequest<'_>) -> Result<reducer::ReducerAnswer, reducer::ReducerError>,
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
