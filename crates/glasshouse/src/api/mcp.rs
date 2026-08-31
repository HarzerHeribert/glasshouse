//! The MCP door — Phase 43. Glasshouse's control operations as tools an
//! orchestrator harness can call.
//!
//! # A transport, not a second API
//!
//! `glasshouse mcp serve` speaks the Model Context Protocol over stdio:
//! JSON-RPC 2.0, one message per line, `initialize` → `tools/list` →
//! `tools/call`. Every tool it offers is a thin adapter onto one
//! [`Request`] variant, answered by the same [`ServerContext::handle`] the
//! Unix socket door answers with. That is the design ruling recorded in
//! `docs/product/design-decisions.md` under *"Phase 43: the MCP surface is a
//! transport over the existing API door"*, and it is what this module
//! inherits rather than re-implements:
//!
//! - **Project scope** (capability map line 1702). The server binds to the
//!   [`Runtime`] it was started in and offers no tool argument that names a
//!   project, a path, a database, or a socket. A session identifier from
//!   another project's database is refused by `SessionApi::resolve`, exactly
//!   as it is refused on the socket, because the request reaches that seam
//!   through the same `dispatch`. This file opens no store of its own —
//!   `tests/mcp_project_scope.rs` greps it to make sure that stays true.
//! - **Dangerous operations are explicit** (line 1703). Spawning a session,
//!   sending it a message, and interrupting it are three separately named
//!   tools whose descriptions say what they do to a process, never one
//!   `glasshouse_control` with an `action` field. A harness's own permission
//!   controls can therefore allow the five read-only tools and ask about the
//!   three that are not; the MCP tool annotations (`readOnlyHint` and its
//!   siblings) say the same thing in the form a harness reads mechanically.
//! - **The caller is a program.** Every message and interrupt this door
//!   delivers is recorded with `MessageOrigin::Machine`, and no tool accepts
//!   an `origin` argument that could say otherwise: the field exists on the
//!   wire for `glasshouse api send`, which knows a person ran it, and an MCP
//!   client is never that.
//!
//! # Hand-rolled on `serde_json`, deliberately
//!
//! The handshake and the two tool methods are a few hundred lines; a
//! dependency that pulled an async runtime into a binary that has none is
//! the thing this project has refused every time. What is implemented is the
//! 2025-06-18 revision's stdio transport: newline-delimited frames, no
//! embedded newlines, protocol on stdout and nothing else on it, diagnostics
//! on stderr. JSON-RPC batches — removed in that revision — are refused as
//! an invalid request rather than half-supported. Where the specification
//! leaves a choice, the conservative reading is taken and stated at the site.
//!
//! # What happens when the client goes away
//!
//! EOF on stdin ends the read loop and the server returns cleanly. Nothing
//! here interrupts, stops, closes, or marks the sessions it spawned on the
//! way out: Glasshouse orchestrates real harnesses, and a client
//! disconnecting is not an instruction to reap the workers it started — an
//! orchestrator that wants a worker stopped calls `glasshouse_interrupt_session`
//! while it is still connected.
//!
//! That is a statement about what this module does, not a promise about
//! what the harness experiences, and the two differ. The sessions' pseudo-
//! terminals are held by this process, so when it exits the kernel closes
//! them and each harness receives `SIGHUP` on its controlling terminal.
//! Measured on macOS with a shell harness: one that handles the hangup kept
//! running, reparented to init, and saw EOF on its stdin; one that had only
//! just been spawned died before it ran a line. A harness that takes the
//! default action on `SIGHUP` — most do — ends with the server. This is the
//! same fate a `glasshouse api serve` that is killed hands its sessions, and
//! nothing here can promise more than "not killed by Glasshouse".
//!
//! Nothing from a session's output or a memory's body is ever written to
//! stderr or to a log line by this module; those travel only inside a tool
//! result.

use std::io::{BufRead, BufWriter, Write};

use glasshouse::Runtime;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::protocol::default_recent_output_bytes;
use glasshouse::guardrails::{
    AssumptionState, BlastRadius, ChangeFactors, EvidenceSource, GuardrailOverride,
    GuardrailResponse, PromotionKind, Uncertainty,
};

use super::protocol::{
    Request, RequestOrigin, Response, default_assumptions_limit, default_memory_limit,
};
use super::unix::ServerContext;

/// The protocol revision this server implements.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Revisions a client may name that this server also answers: the tool
/// methods and annotations used here exist unchanged in each of them, so a
/// client on an older revision is answered on its own revision rather than
/// told to upgrade. Anything else is answered with [`PROTOCOL_VERSION`], as
/// the specification's version negotiation says a server should.
const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// JSON-RPC 2.0's reserved error codes, the four this server can produce.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// What a client is told at `initialize`, in the field the specification
/// sets aside for guidance the model reads. Line 1703 in prose: which tools
/// change a session's state, so a model that is asked before using them
/// knows why it is being asked.
const INSTRUCTIONS: &str = "Every tool acts on the one Glasshouse project this server was started in; \
     there is no argument for naming another. Three tools change a session's state and say so: \
     glasshouse_spawn_session starts a harness process, glasshouse_send_message injects input into \
     a running harness, glasshouse_interrupt_session interrupts one. Four tools write to this \
     project's assumption ledger and nothing else: glasshouse_preflight (which may also take a \
     checkpoint), glasshouse_record_assumption, glasshouse_update_assumption and \
     glasshouse_promote_assumption. Before a substantial change — a migration, a destructive \
     operation, a security or data-integrity change, an unfamiliar integration, a broad refactor \
     — call glasshouse_preflight and state what you know about the change; then record the few \
     critical assumptions it asks for, with their evidence. Glasshouse never infers an assumption \
     from your output: it records only what you state. The remaining six tools only read.";

/// Serve MCP on this process's stdin and stdout until stdin reaches EOF.
///
/// Bound to `runtime` — the project the process was started in — for the
/// whole of its life; see the module doc for why there is no way to say
/// otherwise. Returns when the client closes stdin, or with an error when
/// stdout can no longer be written, which is the same event seen from the
/// other side.
pub fn serve(runtime: &Runtime) -> anyhow::Result<()> {
    let context = ServerContext::open(runtime)?;
    // Stderr, never stdout: stdout is the protocol channel and a client
    // reading it expects frames and nothing else. The project root is the
    // one fact a person debugging a harness registration needs — "which
    // project did this server bind to" — and it is the harness's own
    // process log this lands in, on the same machine, for the same user.
    eprintln!(
        "glasshouse: MCP server for project {} on stdio",
        runtime.project().root().display()
    );

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    serve_frames(&context, stdin.lock(), &mut output)
}

/// The read loop, over any pair of streams so a test can drive it without a
/// process. Every stdout frame is flushed the moment it is written: a client
/// waits on each reply, and a reply held in a buffer is a deadlock with a
/// timer on it.
fn serve_frames(
    context: &ServerContext,
    input: impl BufRead,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    for line in input.lines() {
        let line = match line {
            Ok(line) => line,
            // Bytes that are not UTF-8 are a frame this server cannot read,
            // which is a parse error like any other: answered, and the loop
            // goes on. `read_line` has consumed the bytes by the time it
            // reports this, so the next frame starts clean.
            Err(err) if err.kind() == std::io::ErrorKind::InvalidData => {
                write_frame(
                    output,
                    &error_reply(Value::Null, PARSE_ERROR, "frame is not valid UTF-8"),
                )?;
                continue;
            }
            Err(err) => return Err(err.into()),
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_frame(context, &line) {
            write_frame(output, &reply)?;
        }
    }
    Ok(())
}

fn write_frame(output: &mut impl Write, frame: &Value) -> anyhow::Result<()> {
    // `to_string` never emits a raw newline — every one inside a string is
    // escaped — so one frame is one line, which is the whole of the stdio
    // transport's framing rule.
    let mut payload = serde_json::to_string(frame)?;
    payload.push('\n');
    output.write_all(payload.as_bytes())?;
    output.flush()?;
    Ok(())
}

/// Answer one line. `None` is the reply to a notification, which is to say
/// no reply at all — the specification forbids one.
fn handle_frame(context: &ServerContext, line: &str) -> Option<Value> {
    match parse_frame(line) {
        Frame::Silent => None,
        Frame::Refused(reply) => Some(reply),
        Frame::Request { id, method, params } => {
            Some(match dispatch_method(context, &method, params) {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err(RpcError { code, message }) => error_reply(id, code, message),
            })
        }
    }
}

/// What one line turned out to be, decided before anything is dispatched.
enum Frame {
    /// A request to answer.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// A notification, or a response to a request this server never sent:
    /// nothing to answer, by the protocol's own rule.
    Silent,
    /// Malformed, with the reply that says so.
    Refused(Value),
}

/// Parse one line into a [`Frame`]. Pure, so the protocol's edge cases are
/// testable without a project behind them.
fn parse_frame(line: &str) -> Frame {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(err) => {
            return Frame::Refused(error_reply(
                Value::Null,
                PARSE_ERROR,
                format!("parse error: {err}"),
            ));
        }
    };
    let message = match message {
        Value::Object(message) => message,
        // Batches were removed from the protocol in 2025-06-18, and a server
        // that half-supported them would answer one frame with several. A
        // client that sends one is on a revision this server did not agree
        // to, and is told so.
        Value::Array(_) => {
            return Frame::Refused(error_reply(
                Value::Null,
                INVALID_REQUEST,
                "JSON-RPC batches are not supported; send one message per line",
            ));
        }
        _ => {
            return Frame::Refused(error_reply(
                Value::Null,
                INVALID_REQUEST,
                "a JSON-RPC message is an object",
            ));
        }
    };

    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str);

    // A message with no id is a notification, and a notification is never
    // answered, whatever it says — `notifications/initialized`,
    // `notifications/cancelled`, and anything this server has never heard of
    // alike. That is the one branch that must come before the `jsonrpc`
    // check: an unanswerable message is unanswerable even when malformed.
    let Some(id) = id else {
        if method.is_some() {
            return Frame::Silent;
        }
        return Frame::Refused(error_reply(
            Value::Null,
            INVALID_REQUEST,
            "a message with neither `id` nor `method` is not a request",
        ));
    };
    // The specification forbids a null id on a request outright, and a
    // string or number is what one is allowed to be.
    if !(id.is_string() || id.is_number()) {
        return Frame::Refused(error_reply(
            Value::Null,
            INVALID_REQUEST,
            "a request id must be a string or a number",
        ));
    }
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Frame::Refused(error_reply(
            id,
            INVALID_REQUEST,
            "a request must carry `\"jsonrpc\": \"2.0\"`",
        ));
    }
    let Some(method) = method else {
        // An id and no method is a response — to a request this server never
        // sends. There is nobody to deliver it to, and nothing to say back.
        if message.contains_key("result") || message.contains_key("error") {
            return Frame::Silent;
        }
        return Frame::Refused(error_reply(
            id,
            INVALID_REQUEST,
            "a request must carry a `method`",
        ));
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    Frame::Request {
        id,
        method: method.to_owned(),
        params,
    }
}

fn error_reply(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    })
}

/// A JSON-RPC error: the protocol's own answer, as opposed to a tool result
/// with `isError` set, which is the tool's.
struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: INVALID_PARAMS,
            message: message.into(),
        }
    }
}

fn dispatch_method(
    context: &ServerContext,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(initialize_result(&params)),
        "ping" => Ok(json!({})),
        // No cursor: eight tools fit in one page, and a cursor a client
        // passes back is ignored rather than refused, as the specification
        // allows a server without pagination to do.
        "tools/list" => Ok(json!({ "tools": TOOLS.iter().map(tool_json).collect::<Vec<_>>() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("`params.name` must name a tool"))?;
            let arguments = match params.get("arguments") {
                None | Some(Value::Null) => Value::Object(Map::new()),
                Some(arguments @ Value::Object(_)) => arguments.clone(),
                Some(_) => {
                    return Err(RpcError::invalid_params(
                        "`params.arguments` must be an object",
                    ));
                }
            };
            call_tool(context, name, arguments)
        }
        // This server sends no requests of its own, so it has no logging
        // level to set, no resources, no prompts, and no completions. A
        // client that asks for any of them is answered with the code the
        // specification reserves for exactly that.
        _ => Err(RpcError {
            code: METHOD_NOT_FOUND,
            message: format!("method `{method}` is not one this server answers"),
        }),
    }
}

/// The `initialize` result. The client's own revision is echoed when it is
/// one this server answers ([`SUPPORTED_VERSIONS`]); otherwise the server's,
/// and the client decides whether to go on.
fn initialize_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = match requested {
        Some(version) if SUPPORTED_VERSIONS.contains(&version) => version,
        _ => PROTOCOL_VERSION,
    };
    json!({
        "protocolVersion": version,
        // `listChanged: false` — the tool set is fixed at compile time and
        // this server never sends the notification that says otherwise.
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "glasshouse",
            "title": "Glasshouse",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

/// One tool: a name, what a harness shows for it, how a harness may gate it,
/// the shape of its arguments, and the one [`Request`] it becomes.
struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    annotations: Annotations,
    input_schema: fn() -> Value,
    /// Arguments → the request. A failure here is the caller's — a missing
    /// or unknown argument — and is answered as `-32602`, never as a tool
    /// result, because nothing was called.
    build: fn(Value) -> Result<Request, serde_json::Error>,
}

/// The MCP tool annotations, all four stated on every tool rather than left
/// to their defaults, because the defaults (`destructiveHint: true`,
/// `openWorldHint: true`) are the cautious ones and a harness reading them
/// deserves to know which tools they were actually decided for.
struct Annotations {
    read_only: bool,
    destructive: bool,
    idempotent: bool,
}

/// Reads this project's state and changes nothing: the annotation a harness
/// can allow without asking.
const READ_ONLY: Annotations = Annotations {
    read_only: true,
    destructive: false,
    idempotent: true,
};

/// The thirteen tools, and the whole of what this door can do.
///
/// Every entry maps onto exactly one [`Request`] variant, and the request's
/// own handler decides everything after that — bounds, scope, and errors —
/// which is why the descriptions below say what the operation *is* and not
/// how it is checked. A fourteenth tool would be a fourteenth entry here and
/// nothing else; a tool that reached past `ServerContext::handle` cannot be
/// written in this file without the source-scanning test noticing.
///
/// The five guardrail tools (Phase 21K) are the design ruling's *"MCP
/// twins"* of the five assumption requests: same fields, same handlers, and
/// `deny_unknown_fields` on every argument type so a `reasoning` field is
/// a `-32602` rather than something silently dropped.
const TOOLS: &[Tool] = &[
    Tool {
        name: "glasshouse_list_sessions",
        title: "List sessions",
        description: "List every session in this project, most recently active first, with its \
                      harness, role, lifecycle state, and recorded route. Read-only.",
        annotations: READ_ONLY,
        input_schema: || object_schema(json!({}), &[]),
        build: |arguments| {
            let NoArguments {} = serde_json::from_value(arguments)?;
            Ok(Request::ListSessions)
        },
    },
    Tool {
        name: "glasshouse_session_status",
        title: "Session status",
        description: "The lifecycle state of one session in this project (starting, running, idle, \
                      waiting_for_user, stopped, failed, or closed). Read-only.",
        annotations: READ_ONLY,
        input_schema: || object_schema(json!({ "session": SESSION_PROPERTY }), &["session"]),
        build: |arguments| {
            let SessionArguments { session } = serde_json::from_value(arguments)?;
            Ok(Request::SessionState { session })
        },
    },
    Tool {
        name: "glasshouse_spawn_session",
        title: "Spawn session",
        description: "STARTS A PROCESS: launches a new session under an installed harness in this \
                      project, optionally delivering a task as its first message. The session \
                      keeps running after this call returns. Not read-only.",
        annotations: Annotations {
            read_only: false,
            // Additive: it creates a session and touches no existing one.
            destructive: false,
            idempotent: false,
        },
        input_schema: || {
            object_schema(
                json!({
                    "harness": {
                        "type": "string",
                        "description": "The harness to start, by its Glasshouse identifier, e.g. `claude-code`.",
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Extra command-line arguments for the harness.",
                    },
                    "role": {
                        "type": "string",
                        "enum": ["worker", "orchestrator", "normal"],
                        "description": "How the session is tagged. Absent means `worker`.",
                    },
                    "task": {
                        "type": "string",
                        "description": "A task delivered to the session as its first message, the instant it is live.",
                    },
                    "guardrail": {
                        "type": "string",
                        "enum": GuardrailOverride::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>(),
                        "description": "A per-task assumption-guardrail override for the new session: `force` gates every substantial change, `skip` waives the gate (recorded as waived_by_user), `lower` keeps it advisory. Absent means the configured mode applies.",
                    },
                }),
                &["harness"],
            )
        },
        build: |arguments| {
            let SpawnArguments {
                harness,
                args,
                role,
                task,
                guardrail,
            } = serde_json::from_value(arguments)?;
            Ok(Request::SpawnSession {
                harness,
                args,
                role,
                task,
                guardrail,
                // The MCP tool does not offer a presentation backend yet:
                // Phase 17's expose-and-focus is a person's workflow first
                // (line 1895), and the tool's schema grows only once use
                // asks for it.
                presentation: None,
            })
        },
    },
    Tool {
        name: "glasshouse_send_message",
        title: "Send message",
        description: "INJECTS INPUT INTO A RUNNING HARNESS: delivers one line of text to a live \
                      session in this project as if typed at its terminal, recorded as a \
                      machine-originated message. Not read-only.",
        annotations: Annotations {
            read_only: false,
            // What a harness does with a line it is handed is not bounded
            // by anything here, so the cautious answer is the honest one.
            destructive: true,
            idempotent: false,
        },
        input_schema: || {
            object_schema(
                json!({
                    "session": SESSION_PROPERTY,
                    "text": {
                        "type": "string",
                        "description": "The line to deliver. Data, never a command: it is not interpreted on its way to the session.",
                    },
                }),
                &["session", "text"],
            )
        },
        build: |arguments| {
            let SendArguments { session, text } = serde_json::from_value(arguments)?;
            Ok(Request::SendMessage {
                session,
                text,
                // Ruling 4: an MCP caller is a program. Stated here rather
                // than left to the wire default so that a future change to
                // that default cannot change what this door records.
                origin: RequestOrigin::Machine,
            })
        },
    },
    Tool {
        name: "glasshouse_interrupt_session",
        title: "Interrupt session",
        description: "INTERRUPTS A RUNNING HARNESS: sends a real interrupt (Ctrl-C) to a live \
                      session's terminal in this project, stopping whatever it is doing. Not \
                      read-only.",
        annotations: Annotations {
            read_only: false,
            destructive: true,
            // A second interrupt is not a no-op: many harnesses exit on it.
            idempotent: false,
        },
        input_schema: || object_schema(json!({ "session": SESSION_PROPERTY }), &["session"]),
        build: |arguments| {
            let SessionArguments { session } = serde_json::from_value(arguments)?;
            Ok(Request::Interrupt {
                session,
                origin: RequestOrigin::Machine,
            })
        },
    },
    Tool {
        name: "glasshouse_recent_output",
        title: "Recent output",
        description: "The tail of a live session's terminal output in this project — what the \
                      worker is doing right now. Bounded server-side; a session no process is \
                      running is an error, not an empty string. Read-only.",
        annotations: READ_ONLY,
        input_schema: || {
            object_schema(
                json!({
                    "session": SESSION_PROPERTY,
                    "max_bytes": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "At most this many bytes of the tail. Capped server-side regardless.",
                    },
                }),
                &["session"],
            )
        },
        build: |arguments| {
            let RecentOutputArguments { session, max_bytes } = serde_json::from_value(arguments)?;
            Ok(Request::RecentOutput { session, max_bytes })
        },
    },
    Tool {
        name: "glasshouse_search_memory",
        title: "Search memory",
        description: "Search this project's durable memory and return the best-ranked entries. \
                      Read-only.",
        annotations: READ_ONLY,
        input_schema: || {
            object_schema(
                json!({
                    "query": { "type": "string", "description": "What to search for." },
                    "history": {
                        "type": "boolean",
                        "description": "Include superseded memories as well as active ones.",
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "At most this many results. Capped server-side regardless.",
                    },
                }),
                &["query"],
            )
        },
        build: |arguments| {
            let SearchArguments {
                query,
                history,
                limit,
            } = serde_json::from_value(arguments)?;
            Ok(Request::QueryMemory {
                query,
                history,
                limit,
            })
        },
    },
    Tool {
        name: "glasshouse_get_checkpoint",
        title: "Get checkpoint",
        description: "Retrieve one of this project's session checkpoints — the most recent by \
                      default — as a bootstrap prompt or the full handoff document. Read-only.",
        annotations: READ_ONLY,
        input_schema: || {
            object_schema(
                json!({
                    "checkpoint": {
                        "type": "string",
                        "description": "A checkpoint id or unambiguous prefix. Absent or `latest` means the most recent.",
                    },
                    "document": {
                        "type": "boolean",
                        "description": "Return the rendered handoff document rather than the terser bootstrap prompt.",
                    },
                }),
                &[],
            )
        },
        build: |arguments| {
            let CheckpointArguments {
                checkpoint,
                document,
            } = serde_json::from_value(arguments)?;
            Ok(Request::GetCheckpoint {
                checkpoint,
                document,
            })
        },
    },
    Tool {
        name: "glasshouse_preflight",
        title: "Assumption preflight",
        description: "Ask the guardrail about a change you intend to make, BEFORE making it. \
                      State what you know: the footprint (files touched) and subsystems, reversibility, blast \
                      radius, whether it is a migration, destructive, security- or \
                      data-integrity-relevant, an unfamiliar integration, an architectural \
                      change or a broad refactor, the evidence class its premise rests on, and a \
                      coarse budget (with `spent` when re-evaluating). Answers a risk class, the \
                      factor that decided it, a verdict (proceed | advisory | gated), at most \
                      three critical-assumption prompts, guidance, and the seven explicit \
                      responses. Trivial, local, reversible edits answer `proceed` with no \
                      prompts. With a session, the gate is recorded on that session's ledger \
                      and a substantial change takes a checkpoint first. Writes to the ledger \
                      only.",
        annotations: Annotations {
            read_only: false,
            // Additive: a gate row, possibly a checkpoint; nothing existing
            // is changed.
            destructive: false,
            idempotent: false,
        },
        input_schema: || {
            object_schema(
                json!({
                    "session": SESSION_PROPERTY,
                    "change": change_schema(),
                }),
                &["change"],
            )
        },
        build: |arguments| {
            let PreflightArguments { session, change } = serde_json::from_value(arguments)?;
            Ok(Request::Preflight { session, change })
        },
    },
    Tool {
        name: "glasshouse_record_assumption",
        title: "Record assumption",
        description: "Record one critical assumption a change rests on, in six fields and no \
                      more: a one-sentence claim, its current evidence, the evidence's source \
                      class, the uncertainty, the affected scope, and the cheapest useful \
                      verification step. Recorded as `proposed`. Glasshouse never infers an \
                      assumption from your output — record only what you actually assume, and \
                      never your reasoning. Writes to the ledger only.",
        annotations: Annotations {
            read_only: false,
            destructive: false,
            idempotent: false,
        },
        input_schema: || {
            object_schema(
                json!({
                    "session": SESSION_PROPERTY,
                    "claim": {
                        "type": "string",
                        "description": "The premise, in one sentence (at most 280 characters).",
                    },
                    "evidence": {
                        "type": "string",
                        "description": "What currently supports it.",
                    },
                    "evidence_source": {
                        "type": "string",
                        "enum": EvidenceSource::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>(),
                        "description": "What class of evidence that is. `inference` means unverified.",
                    },
                    "uncertainty": {
                        "type": "string",
                        "enum": Uncertainty::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>(),
                    },
                    "affected": {
                        "type": "string",
                        "description": "The affected scope: what the change depends on it for, and what is wrong if it is false.",
                    },
                    "verification": {
                        "type": "string",
                        "description": "The cheapest useful step that would confirm or falsify it.",
                    },
                }),
                &[
                    "claim",
                    "evidence",
                    "evidence_source",
                    "uncertainty",
                    "affected",
                    "verification",
                ],
            )
        },
        build: |arguments| {
            let RecordAssumptionArguments {
                session,
                claim,
                evidence,
                evidence_source,
                uncertainty,
                affected,
                verification,
            } = serde_json::from_value(arguments)?;
            Ok(Request::RecordAssumption {
                session,
                claim,
                evidence,
                evidence_source,
                uncertainty,
                affected,
                verification,
                origin: RequestOrigin::Machine,
            })
        },
    },
    Tool {
        name: "glasshouse_update_assumption",
        title: "Update assumption",
        description: "Append a transition to an assumption: move it to proposed | probing | \
                      supported | refuted | unresolved (waived_by_user is a person's decision and \
                      is refused here), or leave the state and record a response (inspect | \
                      continue | verify | checkpoint | handoff | re-plan | stop) or a note. With \
                      `state: refuted` and `record_failed_approach: true`, one failed-attempt \
                      memory is written with provenance naming the assumption, so the approach \
                      is not repeated. Transitions only ever append; nothing is edited. Writes to \
                      the ledger (and, when asked, one memory) only.",
        annotations: Annotations {
            read_only: false,
            destructive: false,
            idempotent: false,
        },
        input_schema: || {
            object_schema(
                json!({
                    "assumption": {
                        "type": "string",
                        "description": "An assumption id as glasshouse_list_assumptions lists it, or an unambiguous leading part of one.",
                    },
                    "state": {
                        "type": "string",
                        "enum": ["proposed", "probing", "supported", "refuted", "unresolved"],
                        "description": "The new state. Absent re-states the current one.",
                    },
                    "note": {
                        "type": "string",
                        "description": "What was learned, in a sentence.",
                    },
                    "response": {
                        "type": "string",
                        "enum": GuardrailResponse::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>(),
                        "description": "The explicit response chosen to a guardrail event, when this transition is one.",
                    },
                    "record_failed_approach": {
                        "type": "boolean",
                        "description": "With `state: refuted`: write one failed-attempt memory naming this assumption.",
                    },
                }),
                &["assumption"],
            )
        },
        build: |arguments| {
            let UpdateAssumptionArguments {
                assumption,
                state,
                note,
                response,
                record_failed_approach,
            } = serde_json::from_value(arguments)?;
            Ok(Request::UpdateAssumption {
                assumption,
                state,
                note,
                response,
                record_failed_approach,
                origin: RequestOrigin::Machine,
            })
        },
    },
    Tool {
        name: "glasshouse_list_assumptions",
        title: "List assumptions",
        description: "This project's recorded assumptions with their current states, newest \
                      first, and the counts per state — for one session when given, with that \
                      session's gates, overrides and budget events. Read-only.",
        annotations: READ_ONLY,
        input_schema: || {
            object_schema(
                json!({
                    "session": SESSION_PROPERTY,
                    "limit": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "At most this many assumptions. Capped server-side regardless.",
                    },
                }),
                &[],
            )
        },
        build: |arguments| {
            let ListAssumptionsArguments { session, limit } = serde_json::from_value(arguments)?;
            Ok(Request::ListAssumptions { session, limit })
        },
    },
    Tool {
        name: "glasshouse_promote_assumption",
        title: "Promote assumption",
        description: "Promote a SUPPORTED assumption into this project's durable memory as a \
                      decision, a constraint or a finding — and as nothing else. Any other state \
                      is refused: a task assumption stays apart from project decisions until it \
                      has been supported and somebody explicitly promotes it. Writes one memory \
                      and one ledger transition.",
        annotations: Annotations {
            read_only: false,
            destructive: false,
            idempotent: false,
        },
        input_schema: || {
            object_schema(
                json!({
                    "assumption": {
                        "type": "string",
                        "description": "An assumption id, or an unambiguous leading part of one.",
                    },
                    "kind": {
                        "type": "string",
                        "enum": PromotionKind::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>(),
                    },
                    "note": {
                        "type": "string",
                        "description": "Why it is worth keeping, in a sentence.",
                    },
                }),
                &["assumption", "kind"],
            )
        },
        build: |arguments| {
            let PromoteAssumptionArguments {
                assumption,
                kind,
                note,
            } = serde_json::from_value(arguments)?;
            Ok(Request::PromoteAssumption {
                assumption,
                kind,
                note,
                origin: RequestOrigin::Machine,
            })
        },
    },
];

/// The schema of `glasshouse_preflight`'s `change` argument — one property
/// per field of `guardrails::ChangeFactors`, and `additionalProperties:
/// false` so that a `reasoning` or a `transcript` is refused here as it is
/// by the type.
fn change_schema() -> Value {
    let budget = json!({
        "type": "object",
        "properties": {
            "footprint": { "type": "integer", "minimum": 0, "description": "How many files." },
            "tool_rounds": { "type": "integer", "minimum": 0 },
            "elapsed_minutes": { "type": "integer", "minimum": 0 },
        },
        "additionalProperties": false,
    });
    json!({
        "type": "object",
        "description": "What you know about the intended change. Every field optional; an absent flag means `false`, an absent footprint means one file.",
        "properties": {
            "description": { "type": "string", "description": "One line, for a person to read. Never classified." },
            "footprint": { "type": "integer", "minimum": 0, "description": "How many files the change touches." },
            "subsystems": { "type": "array", "items": { "type": "string" } },
            "reversible": { "type": "boolean", "description": "Whether it can be undone easily. Default true." },
            "blast_radius": {
                "type": "string",
                "enum": BlastRadius::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>(),
            },
            "premise_evidence": {
                "type": "string",
                "enum": EvidenceSource::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>(),
                "description": "What class of evidence the change's premise rests on. `inference` marks it weakly evidenced.",
            },
            "security": { "type": "boolean" },
            "data_integrity": { "type": "boolean" },
            "migration": { "type": "boolean" },
            "destructive": { "type": "boolean" },
            "unfamiliar_integration": { "type": "boolean" },
            "architecture": { "type": "boolean" },
            "broad_refactor": { "type": "boolean" },
            "budget": budget,
            "spent": budget,
        },
        "additionalProperties": false,
    })
}

/// The one argument five tools share: a session identifier, as
/// `glasshouse_list_sessions` lists it. There is deliberately no project
/// component to it — which project the session must belong to is this
/// server's business, not the caller's.
const SESSION_PROPERTY: &str = "__session__";

fn object_schema(properties: Value, required: &[&str]) -> Value {
    let mut properties = properties;
    // The session property is written once, here, rather than repeated in
    // five schemas that could drift apart.
    if let Some(map) = properties.as_object_mut() {
        for value in map.values_mut() {
            if value.as_str() == Some(SESSION_PROPERTY) {
                *value = json!({
                    "type": "string",
                    "description": "A session identifier in this project, as glasshouse_list_sessions lists it.",
                });
            }
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        // Mirrors `deny_unknown_fields` on the argument types below: an
        // argument this schema does not name is refused, not ignored, so a
        // caller that passes `origin` or `project` finds out.
        "additionalProperties": false,
    })
}

fn tool_json(tool: &Tool) -> Value {
    json!({
        "name": tool.name,
        "title": tool.title,
        "description": tool.description,
        "inputSchema": (tool.input_schema)(),
        "annotations": {
            "title": tool.title,
            "readOnlyHint": tool.annotations.read_only,
            "destructiveHint": tool.annotations.destructive,
            "idempotentHint": tool.annotations.idempotent,
            // Nothing here reaches beyond this machine and this project.
            "openWorldHint": false,
        },
    })
}

/// Run one tool. An unknown name or an argument shape the tool does not
/// accept is a protocol error (`-32602`) — nothing ran. A request the
/// handler refused is a tool result with `isError: true` and the handler's
/// own message as its text — something ran, and said no.
fn call_tool(context: &ServerContext, name: &str, arguments: Value) -> Result<Value, RpcError> {
    let tool = TOOLS.iter().find(|tool| tool.name == name).ok_or_else(|| {
        RpcError::invalid_params(format!(
            "unknown tool `{name}`; tools/list names the {} this server offers",
            TOOLS.len()
        ))
    })?;
    let request = (tool.build)(arguments).map_err(|err| {
        RpcError::invalid_params(format!("invalid arguments for `{name}`: {err}"))
    })?;
    Ok(tool_result(context.handle(request)))
}

/// A [`Response`] as a tool result: the handler's JSON, verbatim, as text
/// content. Passed through unchanged on purpose — every guarantee the
/// handlers make about what they return (no credentials, bounded output,
/// a refusal that names no path) is kept by not touching it.
fn tool_result(response: Response) -> Value {
    match response {
        Response::Ok { result } => json!({
            "content": [{ "type": "text", "text": result.to_string() }],
            "isError": false,
        }),
        Response::Error { message } => json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    }
}

// The argument shapes. Each is `deny_unknown_fields`, so an argument a tool
// does not name — an `origin`, a `project`, a `socket` — is a `-32602`
// rather than something silently dropped on the floor; the schemas above
// say `additionalProperties: false` for the same reason.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArguments {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionArguments {
    session: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnArguments {
    harness: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    task: Option<String>,
    #[serde(default)]
    guardrail: Option<GuardrailOverride>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArguments {
    session: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecentOutputArguments {
    session: String,
    #[serde(default = "default_recent_output_bytes")]
    max_bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArguments {
    query: String,
    #[serde(default)]
    history: bool,
    #[serde(default = "default_memory_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointArguments {
    #[serde(default)]
    checkpoint: Option<String>,
    #[serde(default)]
    document: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreflightArguments {
    #[serde(default)]
    session: Option<String>,
    /// Required, unlike the wire request's own `change`: a tool call that
    /// states nothing about the change would be asking the gate to guess,
    /// and an empty object is the honest way to say "one local reversible
    /// edit".
    change: ChangeFactors,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordAssumptionArguments {
    #[serde(default)]
    session: Option<String>,
    claim: String,
    evidence: String,
    evidence_source: EvidenceSource,
    uncertainty: Uncertainty,
    affected: String,
    verification: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateAssumptionArguments {
    assumption: String,
    #[serde(default)]
    state: Option<AssumptionState>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    response: Option<GuardrailResponse>,
    #[serde(default)]
    record_failed_approach: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListAssumptionsArguments {
    #[serde(default)]
    session: Option<String>,
    #[serde(default = "default_assumptions_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromoteAssumptionArguments {
    assumption: String,
    kind: PromotionKind,
    #[serde(default)]
    note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool's schema and argument type agree on which arguments exist
    /// and which are required — the schema is what a harness's model reads,
    /// and the type is what actually decides, so a drift between them is a
    /// tool that refuses what it advertised.
    #[test]
    fn every_tools_schema_names_exactly_the_arguments_its_type_accepts() {
        for tool in TOOLS {
            let schema = (tool.input_schema)();
            let properties = schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("{}: schema has no properties object", tool.name));
            let required: Vec<&str> = schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect();

            // Every required argument present, with a plausible value for
            // its type, must build; that proves the required list is not
            // wider than the type.
            let mut full = Map::new();
            for (name, property) in properties {
                full.insert(name.clone(), sample_for(property));
            }
            assert!(
                (tool.build)(Value::Object(full.clone())).is_ok(),
                "{}: every advertised argument together must build a request",
                tool.name
            );

            // Each required argument removed in turn must fail; each
            // optional one removed must still build. That proves the
            // required list is not narrower than the type either.
            for name in properties.keys() {
                let mut without = full.clone();
                without.remove(name);
                let built = (tool.build)(Value::Object(without)).is_ok();
                if required.contains(&name.as_str()) {
                    assert!(!built, "{}: `{name}` is advertised as required", tool.name);
                } else {
                    assert!(built, "{}: `{name}` is advertised as optional", tool.name);
                }
            }

            // An argument the schema does not name is refused.
            let mut extra = full;
            extra.insert("project".to_owned(), json!("elsewhere"));
            assert!(
                (tool.build)(Value::Object(extra)).is_err(),
                "{}: an argument the schema does not name must be refused",
                tool.name
            );
            assert_eq!(
                schema["additionalProperties"],
                json!(false),
                "{}",
                tool.name
            );
        }
    }

    fn sample_for(property: &Value) -> Value {
        match property["type"].as_str() {
            Some("string") => property["enum"]
                .as_array()
                .and_then(|options| options.first().cloned())
                .unwrap_or_else(|| json!("sample")),
            Some("integer") => json!(1),
            Some("boolean") => json!(true),
            Some("array") => json!([]),
            // An empty object is the one sample every object-typed argument
            // accepts: `ChangeFactors` defaults every field.
            Some("object") => json!({}),
            other => panic!("unexpected property type {other:?}"),
        }
    }

    #[test]
    fn a_send_message_is_always_a_machine_origin_request() {
        let tool = TOOLS
            .iter()
            .find(|tool| tool.name == "glasshouse_send_message")
            .unwrap();
        let request = (tool.build)(json!({ "session": "s", "text": "hello" })).unwrap();
        match request {
            Request::SendMessage { origin, .. } => assert_eq!(origin, RequestOrigin::Machine),
            other => panic!("expected SendMessage, got {other:?}"),
        }
        assert!(
            (tool.build)(json!({ "session": "s", "text": "hello", "origin": "user" })).is_err(),
            "the origin is not the caller's to state"
        );
    }

    #[test]
    fn a_frame_that_is_not_a_request_is_answered_or_ignored_as_the_protocol_says() {
        fn refused_with(line: &str) -> i64 {
            match parse_frame(line) {
                Frame::Refused(reply) => reply["error"]["code"].as_i64().unwrap(),
                Frame::Silent => panic!("{line}: ignored, expected a refusal"),
                Frame::Request { .. } => panic!("{line}: accepted, expected a refusal"),
            }
        }
        assert_eq!(refused_with("not json"), PARSE_ERROR);
        assert_eq!(refused_with("[]"), INVALID_REQUEST);
        assert_eq!(refused_with("42"), INVALID_REQUEST);
        assert_eq!(
            refused_with(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#),
            INVALID_REQUEST
        );
        assert_eq!(refused_with(r#"{"id":1,"method":"ping"}"#), INVALID_REQUEST);
        assert_eq!(refused_with(r#"{"jsonrpc":"2.0","id":1}"#), INVALID_REQUEST);

        assert!(matches!(
            parse_frame(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Frame::Silent
        ));
        assert!(matches!(
            parse_frame(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            Frame::Silent
        ));
        match parse_frame(r#"{"jsonrpc":"2.0","id":"a","method":"ping"}"#) {
            Frame::Request { id, method, params } => {
                assert_eq!(id, json!("a"));
                assert_eq!(method, "ping");
                assert_eq!(params, Value::Null);
            }
            _ => panic!("a well-formed request must be accepted"),
        }
    }
}
