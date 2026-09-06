//! The append-only rollout file: one JSON object per line, one file per
//! session. [`contract::RolloutKind`]'s doc comment is why one file rather
//! than two -- 61C's `"turn"` lines and 61E's `"cell"` lines share it so that
//! append order is the session's order. [`resume`] rebuilds a
//! [`contract::Conversation`] by reading the file, never by replaying it:
//! `runtime-contract.md` §4 fixes that a resumed program must not re-run a
//! side effect, and "a program that deleted a branch would delete it twice"
//! is the reason.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::contract::{Conversation, Message, Role, RolloutKind, SessionId};
use crate::runtime::outcome::CellRecord;

/// The `kind` pane writes for the one line per session that carries the
/// system prompt. `RolloutKind` (`contract.rs`, frozen) only names the two
/// kinds 61C and 61E's runtime both need to recognise by name across the
/// shared file; this kind is 61C's own, chosen and documented here, and a
/// reader that does not know it skips it exactly like any other unknown
/// `kind` -- so nothing about the shared format depends on this name.
/// The system prompt's line kind, from the frozen vocabulary rather than
/// spelled here: 61E writes into the same file and both halves must agree on
/// the string, which a shared enum guarantees and a local constant does not.
pub const SYSTEM_KIND: &str = RolloutKind::System.as_str();

#[derive(Serialize, Deserialize)]
struct SystemLine {
    kind: String,
    session_id: String,
    text: String,
}

#[derive(Serialize, Deserialize)]
struct TurnLine {
    kind: String,
    session_id: String,
    turn: u64,
    role: String,
    text: String,
    at_millis: u64,
}

/// One cell's line: `runtime-contract.md` §4's own object, with the two
/// fields every line in this shared file carries in front of it.
///
/// **`#[serde(flatten)]` rather than a struct that repeats §4's fields.** The
/// contract's shape is `CellRecord`'s and the runtime owns it; re-spelling it
/// here would be a second place for it to drift, and the drift would be
/// invisible because both halves would still serialise.
#[derive(Serialize)]
struct CellLine<'a> {
    kind: &'static str,
    session_id: &'a str,
    #[serde(flatten)]
    record: &'a CellRecord,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// An open rollout file. Every write appends one JSON line and flushes; no
/// code path in this type seeks or truncates.
pub struct Rollout {
    file: File,
    session_id: SessionId,
    next_turn: u64,
}

impl Rollout {
    /// Opens the rollout file at `path`, creating it if it does not exist.
    /// A newly created file's first line records `system` once, per the
    /// packet: it is never repeated on a later turn.
    pub fn create(path: &Path, session_id: SessionId, system: &str) -> io::Result<Self> {
        let is_new = !path.exists();
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let mut rollout = Self {
            file,
            session_id,
            next_turn: 0,
        };
        if is_new {
            let line = SystemLine {
                kind: SYSTEM_KIND.to_string(),
                session_id: rollout.session_id.as_str().to_string(),
                text: system.to_string(),
            };
            rollout.append_line(&line)?;
        }
        Ok(rollout)
    }

    /// Appends one turn at the next turn number and advances it.
    pub fn record_turn(&mut self, role: Role, text: &str) -> io::Result<()> {
        let line = TurnLine {
            kind: RolloutKind::Turn.as_str().to_string(),
            session_id: self.session_id.as_str().to_string(),
            turn: self.next_turn,
            role: role.as_str().to_string(),
            text: text.to_string(),
            at_millis: now_millis(),
        };
        self.append_line(&line)?;
        self.next_turn += 1;
        Ok(())
    }

    /// Appends one cell's line -- `runtime-contract.md` §4.
    ///
    /// **It does not advance the turn number.** A cell is not a turn: the
    /// user and assistant messages around it write their own `turn` lines and
    /// [`resume`] rebuilds the conversation from those alone, so a cell line
    /// that consumed a turn number would leave a hole in the sequence
    /// `record_turn` maintains.
    pub fn record_cell(&mut self, record: &CellRecord) -> io::Result<()> {
        let session_id = self.session_id.clone();
        let line = CellLine {
            kind: RolloutKind::Cell.as_str(),
            session_id: session_id.as_str(),
            record,
        };
        self.append_line(&line)
    }

    fn append_line<T: Serialize>(&mut self, line: &T) -> io::Result<()> {
        let mut json = serde_json::to_vec(line).map_err(io::Error::from)?;
        json.push(b'\n');
        self.file.write_all(&json)?;
        self.file.flush()
    }
}

/// Rebuilds a [`Conversation`] from the rollout file at `path` alone.
///
/// Reads every line, keeps the ones it recognises (`system` and `"turn"`),
/// and returns their content in file order -- it replays nothing and issues
/// no request. A `kind` this reader does not know (61E's `"cell"` lines, or
/// anything newer) is skipped. Only the *final* line is allowed to fail to
/// parse: an append-only log's last line is the one most likely to be
/// half-written by a process that died mid-append, so it is dropped and the
/// rest of the file is kept; a parse failure earlier in the file is a
/// genuinely corrupt rollout and is an error.
pub fn resume(path: &Path) -> io::Result<Conversation> {
    let file = File::open(path)?;
    let lines: Vec<String> = BufReader::new(file).lines().collect::<io::Result<_>>()?;

    let mut system = String::new();
    let mut messages = Vec::new();
    let last_index = lines.len().saturating_sub(1);

    for (index, raw) in lines.iter().enumerate() {
        let value: serde_json::Value = match serde_json::from_str(raw) {
            Ok(value) => value,
            Err(_) if index == last_index => continue,
            Err(err) => return Err(io::Error::new(io::ErrorKind::InvalidData, err)),
        };
        let kind = value.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        if kind == SYSTEM_KIND {
            if let Some(text) = value.get("text").and_then(|t| t.as_str()) {
                system = text.to_string();
            }
        } else if kind == RolloutKind::Turn.as_str() {
            let role = match value.get("role").and_then(|r| r.as_str()) {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => continue,
            };
            let text = value.get("text").and_then(|t| t.as_str()).unwrap_or("");
            messages.push(Message::text(role, text));
        }
        // Any other kind (e.g. `RolloutKind::Cell`'s `"cell"`) is skipped.
    }

    Ok(Conversation { system, messages })
}
