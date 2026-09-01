//! The Claude Code adapter — the only place this crate parses one harness's
//! own JSON (design-decisions.md: "Claude Code JSON lives only in an
//! adapter beside the other integrations"). Codex and later harnesses get
//! their own adapter beside this one; the firewall core downstream of
//! [`normalize`] never sees a harness-specific shape again.

use serde::Deserialize;
use serde_json::Value;

use super::model::{ToolPayload, ToolResult};

/// One `PostToolUse` event, exactly as documented against the installed
/// Claude Code (design-decisions.md's Phase 57 addendum, VERIFIED
/// 2026-09-01): `tool_name`, `tool_input`, `tool_response`, `tool_use_id`,
/// `session_id`, `cwd`, and more this build never reads. Extra fields are
/// pass-through unknowns by construction — `serde`'s default is to ignore
/// them, so no `deny_unknown_fields` is needed or wanted here.
#[derive(Debug, Deserialize)]
pub struct PostToolUseEvent {
    pub tool_name: String,
    pub tool_response: Value,
    pub tool_use_id: String,
    pub session_id: String,
    /// The tool's own input — absent from every batch-71/72 fixture, and
    /// still optional here: a document that omits it parses exactly as
    /// before, with [`tool_query`] and [`tool_input_paths`] both answering
    /// as if it were `{}`. Phase 57B (map lines 1998, 2003) is the first
    /// package to read it, for the semantic reducer's own "tool query"
    /// field and the privacy gate's path exclusions — never for anything
    /// the deterministic ladder or the raw store need.
    #[serde(default)]
    pub tool_input: Value,
}

/// Parse one `PostToolUse` JSON document.
pub fn parse_event(bytes: &[u8]) -> Result<PostToolUseEvent, serde_json::Error> {
    serde_json::from_slice(bytes)
}

/// Turn `tool_response` into [`ToolResult`] — map line 1980's normalization,
/// or `None` when the shape is not one this adapter positively recognizes.
/// An unrecognized shape is exactly what map line 1989/1990 calls "unknown",
/// and the caller must bypass rather than guess.
pub fn normalize(event: &PostToolUseEvent) -> Option<ToolResult> {
    let payload = if event.tool_name.eq_ignore_ascii_case("Bash") {
        normalize_bash(&event.tool_response)?
    } else {
        normalize_text(&event.tool_response)?
    };
    Some(ToolResult {
        tool_name: event.tool_name.clone(),
        payload,
    })
}

/// The uniform shape every built-in tool but Bash carries, verified against
/// the installed harness: `{"type": "text", "text": "..."}`. A bare JSON
/// string is accepted too, for a future or MCP adapter that skips the
/// wrapper — still positively recognized, never guessed.
fn normalize_text(value: &Value) -> Option<ToolPayload> {
    match value {
        Value::String(text) => Some(ToolPayload::Text(text.clone())),
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("text") {
                map.get("text")
                    .and_then(Value::as_str)
                    .map(|text| ToolPayload::Text(text.to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Bash's result, recognized only when the shape carries both a `stdout`
/// string and an `interrupted` flag — the two fields this build has actual
/// evidence for. Without both, box 1990's own fallback applies: this
/// function returns `None`, and the caller treats it exactly like any other
/// unrecognized shape — bypass, never a guess at exit status.
///
/// `exit_code`, when present, is read too; its absence is not treated as
/// "exit 0" (see [`ToolPayload::confirmed_clean_exit`]).
fn normalize_bash(value: &Value) -> Option<ToolPayload> {
    let map = value.as_object()?;
    let stdout = map.get("stdout").and_then(Value::as_str)?;
    let interrupted = map.get("interrupted").and_then(Value::as_bool)?;
    let stderr = map
        .get("stderr")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let exit_code = map.get("exit_code").and_then(Value::as_i64);
    Some(ToolPayload::Command {
        stdout: stdout.to_string(),
        stderr,
        interrupted,
        exit_code,
    })
}

/// The tool's own query or command, read from `tool_input` — map line
/// 1998's "tool query", the second thing (besides the task) the semantic
/// reducer's request may carry. Best-effort and generic across tools rather
/// than a per-tool table: the first of a small, ordered set of common field
/// names that is actually a string wins. `None` for a tool whose input
/// carries nothing query-shaped, or an event with no `tool_input` at all.
pub fn tool_query(tool_input: &Value) -> Option<String> {
    const QUERY_KEYS: &[&str] = &["pattern", "command", "query", "glob"];
    QUERY_KEYS.iter().find_map(|key| {
        tool_input
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

/// Every string value in `tool_input` whose key names a path — map line
/// 2003's privacy gate reads this to refuse a `.env`-shaped result before
/// any candidate leaves the process. A key match is by substring
/// (`contains("path")`, case-insensitive) rather than an exact per-tool
/// list, so a future tool's `notebook_path` or `directory_path` is covered
/// without this function learning its name.
pub fn tool_input_paths(tool_input: &Value) -> Vec<String> {
    let Value::Object(map) = tool_input else {
        return Vec::new();
    };
    map.iter()
        .filter(|(key, _)| key.to_ascii_lowercase().contains("path"))
        .filter_map(|(_, value)| value.as_str())
        .map(str::to_owned)
        .collect()
}

/// The `PostToolUse` hook response Claude Code reads back on stdout.
///
/// `hookSpecificOutput.updatedToolOutput` is the field the Phase 57
/// addendum verified against the installed harness. Every other field this
/// build could set (`systemMessage`, `terminalSequence`) is left unset —
/// nothing in this package's box lines needs them.
pub fn hook_response(updated_output: Option<&str>) -> Value {
    match updated_output {
        None => serde_json::json!({}),
        Some(text) => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "updatedToolOutput": text,
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_shaped_response_normalizes() {
        let event = PostToolUseEvent {
            tool_name: "Grep".to_string(),
            tool_response: serde_json::json!({"type": "text", "text": "hello"}),
            tool_use_id: "tu".to_string(),
            session_id: "s".to_string(),
            tool_input: serde_json::Value::Null,
        };
        let result = normalize(&event).expect("must recognize the text shape");
        assert_eq!(result.payload, ToolPayload::Text("hello".to_string()));
    }

    #[test]
    fn a_bash_response_with_stdout_and_interrupted_normalizes_as_command() {
        let event = PostToolUseEvent {
            tool_name: "Bash".to_string(),
            tool_response: serde_json::json!({
                "stdout": "ok\n",
                "stderr": "",
                "interrupted": false,
                "exit_code": 0,
            }),
            tool_use_id: "tu".to_string(),
            session_id: "s".to_string(),
            tool_input: serde_json::Value::Null,
        };
        let result = normalize(&event).expect("must recognize the command shape");
        assert_eq!(
            result.payload,
            ToolPayload::Command {
                stdout: "ok\n".to_string(),
                stderr: String::new(),
                interrupted: false,
                exit_code: Some(0),
            }
        );
    }

    /// The REAL shape, captured against the installed Claude Code 2.1.252
    /// with a throwaway `PostToolUse` hook teeing stdin to a file, driven by
    /// `claude -p "...echo probe-token-$RANDOM-marker; false..."` in a
    /// scratch project — GH-FIREWALL-BRIDGE's report documents the exact
    /// command. Personal paths (`session_id`, `transcript_path`, `cwd`,
    /// `prompt_id`, `tool_use_id`) are scrubbed; every other key and value
    /// below, including the absent `exit_code`, is the real capture.
    #[test]
    fn the_real_captured_bash_success_shape_normalizes_with_no_exit_code_key() {
        let event = PostToolUseEvent {
            tool_name: "Bash".to_string(),
            tool_response: serde_json::json!({
                "stdout": "capture-probe-line-one\ncapture-probe-line-two",
                "stderr": "",
                "interrupted": false,
                "isImage": false,
                "noOutputExpected": false
            }),
            tool_use_id: "capture-tool-use-id".to_string(),
            session_id: "capture-session".to_string(),
            tool_input: serde_json::Value::Null,
        };
        let result = normalize(&event).expect("must recognize the real captured command shape");
        assert_eq!(
            result.payload,
            ToolPayload::Command {
                stdout: "capture-probe-line-one\ncapture-probe-line-two".to_string(),
                stderr: String::new(),
                interrupted: false,
                exit_code: None,
            }
        );
        assert!(
            result.payload.confirmed_clean_exit(),
            "a real PostToolUse Bash event never carries a failing exit — \
             PostToolUseFailure is the exclusive channel for that, verified \
             empirically and documented on ToolPayload::Command::exit_code"
        );
    }

    #[test]
    fn a_bash_response_shaped_like_the_uniform_text_wrapper_does_not_normalize() {
        // Verified reality: some builds may report Bash the same way as
        // every other built-in tool. That shape carries no positive
        // exit/interruption signal, so it must not be guessed at.
        let event = PostToolUseEvent {
            tool_name: "Bash".to_string(),
            tool_response: serde_json::json!({"type": "text", "text": "ok"}),
            tool_use_id: "tu".to_string(),
            session_id: "s".to_string(),
            tool_input: serde_json::Value::Null,
        };
        assert_eq!(normalize(&event), None);
    }

    #[test]
    fn an_unrecognized_shape_normalizes_to_none() {
        let event = PostToolUseEvent {
            tool_name: "SomeMcpTool".to_string(),
            tool_response: serde_json::json!({"content": [{"type": "image"}]}),
            tool_use_id: "tu".to_string(),
            session_id: "s".to_string(),
            tool_input: serde_json::Value::Null,
        };
        assert_eq!(normalize(&event), None);
    }

    #[test]
    fn an_event_with_no_tool_input_still_parses() {
        let event = PostToolUseEvent {
            tool_name: "Grep".to_string(),
            tool_response: serde_json::json!({"type": "text", "text": "hello"}),
            tool_use_id: "tu".to_string(),
            session_id: "s".to_string(),
            tool_input: serde_json::Value::default(),
        };
        assert_eq!(
            normalize(&event).unwrap().payload,
            ToolPayload::Text("hello".to_string())
        );
    }

    #[test]
    fn tool_query_reads_the_first_known_field_that_is_a_string() {
        assert_eq!(
            tool_query(&serde_json::json!({"pattern": "TODO"})),
            Some("TODO".to_string())
        );
        assert_eq!(
            tool_query(&serde_json::json!({"command": "ls -la"})),
            Some("ls -la".to_string())
        );
        assert_eq!(tool_query(&serde_json::json!({"file_path": "a.rs"})), None);
        assert_eq!(tool_query(&serde_json::Value::Null), None);
    }

    #[test]
    fn tool_input_paths_collects_every_key_containing_path() {
        let paths = tool_input_paths(&serde_json::json!({
            "file_path": "src/main.rs",
            "notebook_path": ".env",
            "pattern": "TODO",
        }));
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&"src/main.rs".to_string()));
        assert!(paths.contains(&".env".to_string()));
    }

    #[test]
    fn tool_input_paths_is_empty_for_a_non_object_input() {
        assert!(tool_input_paths(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn the_default_hook_response_is_a_no_op() {
        assert_eq!(hook_response(None), serde_json::json!({}));
    }

    #[test]
    fn an_emitted_response_carries_updated_tool_output() {
        let response = hook_response(Some("reduced text"));
        assert_eq!(
            response["hookSpecificOutput"]["updatedToolOutput"],
            serde_json::json!("reduced text")
        );
    }
}
