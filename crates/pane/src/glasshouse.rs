//! The three seams between `pane` and Glasshouse -- memory and checkpoints
//! (map line 2446), the harness hook protocol (2447), and which entitlement
//! served a request and what it cost (2451) -- and the one mechanism behind
//! all of them: find the `glasshouse` binary, run it, parse what it says, and
//! fall back to a working local default when it is not there.
//!
//! **"Reachable" is decided in exactly one place**, [`Glasshouse::run`]: the
//! executable was found, it was spawned, and it exited 0. Every seam below
//! calls it and takes the same fallback path when it returns `None` -- never
//! a version probe, never a handshake, and never a second definition of
//! reachable that could drift from this one.
//!
//! Like `ruler::meter`, this process never links against `glasshouse`; it
//! shells out to the binary, the same protocol boundary every other native
//! harness crosses (`crates/pane/src/lib.rs`'s own doc comment). Map line
//! 2440 is why: `pane` gains no compile-time dependency on the `glasshouse`
//! crate.
//!
//! Two gaps are established at the packet level, not discovered here, and
//! are recorded rather than worked around. The gateway adds no
//! `x-glasshouse-*` response header, so nothing identifying a request
//! reaches `pane` on the HTTP response -- [`served_by`] builds only the
//! ledger half of map line 2451. And the `routing-cost` readout carries no
//! `cost_micro_usd`, so cost is reported in tokens only.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contract::{ServedBy, SessionId};

/// How this module reaches Glasshouse. `None` means every seam takes its
/// local-default path unconditionally -- no command is ever attempted.
#[derive(Debug, Clone)]
pub enum Glasshouse {
    None,
    /// Shells out to this executable. `PathBuf::from("glasshouse")` lets the
    /// OS resolve it from `PATH` (`Command::new` maps to `execvp` on a
    /// bare name); a test passes its own fake script's path instead, so no
    /// production code here performs a `PATH` lookup of its own.
    Command {
        glasshouse: PathBuf,
    },
}

impl Glasshouse {
    /// The one definition of reachable: found, spawned, and exited 0.
    /// `stdin`, when given, is written and then dropped -- closing that end
    /// of the pipe -- so a child reading until EOF gets exactly one message
    /// and returns; a child that ignores stdin is unaffected.
    pub(crate) fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> Option<Vec<u8>> {
        let Glasshouse::Command { glasshouse } = self else {
            return None;
        };

        let mut command = Command::new(glasshouse);
        command.args(args);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::null());
        command.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });

        let mut child = command.spawn().ok()?;
        if let Some(bytes) = stdin {
            child.stdin.take()?.write_all(bytes).ok()?;
        }
        let output = child.wait_with_output().ok()?;

        if output.status.success() {
            Some(output.stdout)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------
// 2446 -- memory and checkpoints
// ---------------------------------------------------------------------

/// One note in the local fallback store.
///
/// **A plain table of notes is right; authority classes and decay are not.**
/// This exists only for the case where Glasshouse is not installed, and its
/// whole job is to hold strings until Glasshouse is reachable again --
/// reimplementing ranking or decay here is a second Phase 21 and does not
/// belong in this module.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Note {
    pub text: String,
}

/// A JSON-Lines table of [`Note`]s under a directory the caller owns.
pub struct LocalMemory {
    path: PathBuf,
}

impl LocalMemory {
    /// `dir` is pane's own state directory; this owns one file inside it.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            path: dir.into().join("notes.jsonl"),
        }
    }

    pub fn add(&self, text: impl Into<String>) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let note = Note { text: text.into() };
        let mut line = serde_json::to_string(&note).unwrap_or_default();
        line.push('\n');
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())
    }

    fn all(&self) -> Vec<Note> {
        let Ok(contents) = fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        contents
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Notes whose text contains `query`, case-insensitively.
    pub fn search(&self, query: &str) -> Vec<Note> {
        let query = query.to_lowercase();
        self.all()
            .into_iter()
            .filter(|note| note.text.to_lowercase().contains(&query))
            .collect()
    }

    /// The most recently added note, standing in for a checkpoint when
    /// Glasshouse cannot supply one.
    pub fn latest(&self) -> Option<String> {
        self.all().into_iter().next_back().map(|note| note.text)
    }
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[serde(default)]
    result: Option<Value>,
}

/// Calls one MCP tool by spawning `glasshouse mcp serve` and speaking one
/// line of JSON-RPC 2.0 over its stdio. `None` on any failure to reach it,
/// launch it, or parse its answer -- indistinguishable from unreachable by
/// design, since every caller here falls back identically either way.
fn call_tool(glasshouse: &Glasshouse, name: &str, arguments: Value) -> Option<Value> {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let mut line = serde_json::to_vec(&request).ok()?;
    line.push(b'\n');

    let stdout = glasshouse.run(&["mcp", "serve"], Some(&line))?;
    let text = String::from_utf8_lossy(&stdout);
    let response_line = text.lines().next_back()?;
    let response: JsonRpcResponse = serde_json::from_str(response_line).ok()?;
    response.result
}

/// The text of every `{"type":"text","text":...}` content block in an MCP
/// tool result. Empty when `result` carries no such block -- never an error.
fn text_content(result: &Value) -> Vec<String> {
    result
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Searches memory via `glasshouse_search_memory` when reachable, and the
/// local store when not.
pub fn search_memory(glasshouse: &Glasshouse, local: &LocalMemory, query: &str) -> Vec<String> {
    match call_tool(
        glasshouse,
        "glasshouse_search_memory",
        serde_json::json!({ "query": query }),
    ) {
        Some(result) => text_content(&result),
        None => local
            .search(query)
            .into_iter()
            .map(|note| note.text)
            .collect(),
    }
}

/// Reads the checkpoint via `glasshouse_get_checkpoint` when reachable, and
/// the local store's latest note when not.
pub fn checkpoint(glasshouse: &Glasshouse, local: &LocalMemory) -> Option<String> {
    match call_tool(
        glasshouse,
        "glasshouse_get_checkpoint",
        serde_json::json!({}),
    ) {
        Some(result) => text_content(&result).into_iter().next(),
        None => local.latest(),
    }
}

// ---------------------------------------------------------------------
// 2447 -- the harness hook protocol
// ---------------------------------------------------------------------

/// A session lifecycle event, spelled the way Glasshouse's own
/// `session::lifecycle::event_for` spells it -- PascalCase, fixed by the
/// producer. `SessionEnd` maps to nothing there and is deliberately not a
/// variant here: inventing a spelling for it would be a fifth event Glasshouse
/// never asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    SessionStart,
    UserPromptSubmit,
    PermissionRequest,
    Stop,
    StopFailure,
}

impl LifecycleEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            LifecycleEvent::SessionStart => "SessionStart",
            LifecycleEvent::UserPromptSubmit => "UserPromptSubmit",
            LifecycleEvent::PermissionRequest => "PermissionRequest",
            LifecycleEvent::Stop => "Stop",
            LifecycleEvent::StopFailure => "StopFailure",
        }
    }
}

/// Runs `glasshouse hook --session <id> --event <name>`. Unreachable is
/// silent and never propagates: a harness that fails a turn because a hook
/// could not be delivered is worse than one that runs without telemetry.
pub fn emit_lifecycle(glasshouse: &Glasshouse, session: &SessionId, event: LifecycleEvent) {
    let _ = glasshouse.run(
        &[
            "hook",
            "--session",
            session.as_str(),
            "--event",
            event.as_str(),
        ],
        None,
    );
}

/// Runs `glasshouse context-firewall hook --session <id>`, writing the raw
/// tool-result event to its stdin. **This is the other subcommand from
/// [`emit_lifecycle`]**: `PostToolUse` is not in Claude Code's
/// `REPORTED_EVENTS`, so a tool-result event sent to plain `hook` reaches no
/// consumer at all. Unreachable is silent, exactly as above.
pub fn emit_tool_result(glasshouse: &Glasshouse, session: &SessionId, payload: &str) {
    let _ = glasshouse.run(
        &["context-firewall", "hook", "--session", session.as_str()],
        Some(payload.as_bytes()),
    );
}

// ---------------------------------------------------------------------
// 2451 -- which entitlement served each request, and what it cost
// ---------------------------------------------------------------------

/// One row of `glasshouse routing-cost --json`, the same producer and wire
/// shape `ruler::meter::Meter` already reads. Only the columns this module
/// needs are declared; every other key is ignored by `serde_json` without
/// any attribute here.
#[derive(Debug, Deserialize)]
struct ObservationRow {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    route: Option<String>,
    #[serde(default)]
    quota_context: Option<String>,
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cached_input_tokens: Option<u64>,
}

/// Fills a [`ServedBy`] from `glasshouse routing-cost --json --since
/// <since>`. **The row used is the last model observation printed** (excluding local
/// context-firewall bookkeeping): rows arrive ascending
/// by `observed_at`, and the last row at or after `since` is the one closest
/// to the request this call is answering for.
///
/// Absent is not zero here either: no meter, a launch failure, a non-zero
/// exit, or an empty window all produce [`ServedBy::default`], whose
/// `is_known` is `false` -- never a `ServedBy` with token fields defaulted
/// to zero.
pub fn served_by(glasshouse: &Glasshouse, since: SystemTime) -> ServedBy {
    let since_secs = unix_secs(since).to_string();
    let Some(stdout) = glasshouse.run(&["routing-cost", "--json", "--since", &since_secs], None)
    else {
        return ServedBy::default();
    };

    let text = String::from_utf8_lossy(&stdout);
    let row = text
        .lines()
        .filter_map(|line| serde_json::from_str::<ObservationRow>(line.trim()).ok())
        // Tool/firewall bookkeeping is not a model request or an entitlement.
        .rfind(|row| {
            !(row.provider.as_deref() == Some("glasshouse")
                && row.model.as_deref() == Some("context-firewall"))
        });

    match row {
        Some(row) => ServedBy {
            provider: row.provider,
            model: row.model,
            route: row.route,
            quota_context: row.quota_context,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cached_input_tokens: row.cached_input_tokens,
        },
        None => ServedBy::default(),
    }
}

fn unix_secs(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}
