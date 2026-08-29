//! The wire shape of the control API — Phase 42.
//!
//! One connection carries exactly one [`Request`] and exactly one
//! [`Response`], each a single line of JSON. A protocol this small has no
//! framing to get wrong: a caller writes one line, reads one line, and closes
//! the connection. Nothing here is transport-specific — [`super::unix`] is
//! the only module that knows this travels over a Unix domain socket.

use serde::{Deserialize, Serialize};

fn default_memory_limit() -> usize {
    20
}

fn default_events_limit() -> usize {
    200
}

/// One control-API call.
///
/// Every variant is answered against the project the door was opened for —
/// see the door's own doc comment for why that is structural rather than a
/// field every request would otherwise have to repeat.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Every session in this project, most recently active first.
    ListSessions,
    /// One session's lifecycle, plus what Glasshouse recorded about its
    /// current route.
    SessionState { session: String },
    /// Start a new session under an installed harness — capability map
    /// Phase 14, boxes 5, 6 and part of box 1.
    SpawnSession {
        /// A harness identifier, e.g. `claude-code`.
        harness: String,
        #[serde(default)]
        args: Vec<String>,
        /// How the spawned session is tagged — `worker`, `orchestrator`, or
        /// `normal`. Absent means `worker`: every session this door spawns
        /// is spawned *by* something other than a person (an orchestrator,
        /// a script — see this module's own doc comment), so a session with
        /// no role stated is a worker by default rather than
        /// indistinguishable from one a person started by hand.
        #[serde(default)]
        role: Option<String>,
        /// A natural-language task delivered to the session as its first
        /// message, the instant it is live — distinct from
        /// [`Request::SendMessage`], which addresses a session that already
        /// exists. Absent means the harness starts with nothing sent to it,
        /// same as before this field existed.
        #[serde(default)]
        task: Option<String>,
    },
    /// Send one line of text to a live session, as Glasshouse rather than as
    /// the user.
    SendMessage { session: String, text: String },
    /// Interrupt a live session, as Glasshouse rather than as the user.
    Interrupt { session: String },
    /// Current resource capacity and quota telemetry for every model
    /// resource Glasshouse can describe — capability map line 1679.
    ///
    /// Read-only, like every other request this door answers: it never
    /// makes a network request of its own. See
    /// `glasshouse::provider::resources::capacity_json`, which this is
    /// answered with directly, for the exact shape.
    ResourceCapacity,
    /// The current routing-model selection and its health — capability map
    /// line 1680.
    ///
    /// Read-only, like every other request this door answers. Answered from
    /// `glasshouse::config::EffectiveConfig::routing_model` and
    /// `::routing_model_resolution` directly: the recorded choice and the
    /// layer it came from, and whether that choice actually resolves or has
    /// degraded to deterministic heuristics with a reason named in the
    /// type's own words. There is no live latency or health probe anywhere
    /// in this project — a degrade to heuristics *is* the health signal this
    /// line asks for; see those functions' own doc comments.
    RoutingModel,
    /// This project's lifecycle events, in Glasshouse's own vocabulary
    /// rather than any one harness's — capability map line 701.
    ///
    /// Read-only, like every other request this door answers. Incremental:
    /// `after` is the log position the caller has already consumed — `0` for
    /// the start of the log, or a prior response's `head` — and only events
    /// strictly newer than it come back, oldest first. `limit` bounds how
    /// many events one call returns; it is capped server-side regardless of
    /// what is asked for, so a caller cannot pull an unbounded response by
    /// naming a large number.
    Events {
        #[serde(default)]
        after: i64,
        #[serde(default = "default_events_limit")]
        limit: usize,
    },
    /// Search this project's durable memory.
    QueryMemory {
        query: String,
        #[serde(default)]
        history: bool,
        #[serde(default = "default_memory_limit")]
        limit: usize,
    },
    /// Retrieve a checkpoint — capability map line 66, "retrieve a completed
    /// worker result or checkpoint." A worker has no other durable "result"
    /// format Glasshouse owns (Phase 19's checkpoints are the shipped
    /// mechanism; see the evidence ledger); this is the read half of
    /// [`Request::TakeCheckpoint`]'s write.
    GetCheckpoint {
        /// A checkpoint id or unambiguous prefix, or absent/`"latest"` for
        /// the project's most recent checkpoint — the same rule
        /// `glasshouse checkpoint show` uses.
        #[serde(default)]
        checkpoint: Option<String>,
        /// The rendered handoff document, rather than the terser bootstrap
        /// prompt — the same distinction `glasshouse checkpoint show
        /// --document` makes.
        #[serde(default)]
        document: bool,
    },
    /// Take a checkpoint for a session.
    TakeCheckpoint {
        /// Named explicitly, or the project's most recently active session —
        /// the same rule `glasshouse checkpoint save` uses outside this door.
        #[serde(default)]
        session: Option<String>,
        objective: String,
        state: String,
        #[serde(default)]
        decisions: Vec<String>,
        #[serde(default)]
        failed_approaches: Vec<String>,
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        test_state: Option<String>,
        #[serde(default)]
        next_actions: Vec<String>,
    },
}

/// One control-API answer.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { result: serde_json::Value },
    Error { message: String },
}

impl Response {
    pub fn ok(result: serde_json::Value) -> Self {
        Response::Ok { result }
    }

    pub fn err(message: impl std::fmt::Display) -> Self {
        Response::Error {
            message: message.to_string(),
        }
    }
}
