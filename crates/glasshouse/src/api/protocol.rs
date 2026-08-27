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
    /// Start a new session under an installed harness.
    SpawnSession {
        /// A harness identifier, e.g. `claude-code`.
        harness: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Send one line of text to a live session, as Glasshouse rather than as
    /// the user.
    SendMessage { session: String, text: String },
    /// Interrupt a live session, as Glasshouse rather than as the user.
    Interrupt { session: String },
    /// Search this project's durable memory.
    QueryMemory {
        query: String,
        #[serde(default)]
        history: bool,
        #[serde(default = "default_memory_limit")]
        limit: usize,
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
