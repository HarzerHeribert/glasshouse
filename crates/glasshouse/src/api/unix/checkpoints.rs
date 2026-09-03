use glasshouse::Runtime;
use glasshouse::checkpoint::store::{ProjectCheckpoints, StoreError as CheckpointStoreError};
use glasshouse::checkpoint::{Checkpoint, CheckpointReason, Handoff};
use glasshouse::session::ProjectSessions;

use super::memory::binding_memory_lines;
use crate::api::protocol::Response;

/// Retrieve a checkpoint — box 10, the read half of [`request_checkpoint`].
///
/// Resolution mirrors `main.rs`'s own `resolve_checkpoint` (private to that
/// file, so reproduced here against the same public [`ProjectCheckpoints`]
/// surface rather than duplicated by copy): a named id or unambiguous
/// prefix, `"latest"`, or nothing, all meaning what `glasshouse checkpoint
/// show` already means by them. Scoped to this project the same way every
/// other request this door answers is — the socket was opened for one
/// project's own database file, and a checkpoint's row lives in that file
/// alone (see `database.rs`'s per-project file separation).
pub(super) fn get_checkpoint(
    runtime: &Runtime,
    checkpoint: Option<&str>,
    document: bool,
) -> Response {
    let checkpoints = match ProjectCheckpoints::open(runtime) {
        Ok(checkpoints) => checkpoints,
        Err(err) => return Response::err(err),
    };
    let store = checkpoints.store();

    let resolved = match checkpoint {
        None | Some("latest") => store.latest(),
        Some(named) => match store.resolve_id(named) {
            Ok(id) => store.get(&id),
            Err(err) => return Response::err(checkpoint_store_err(err)),
        },
    };

    match resolved {
        Ok(Some(stored)) => Response::ok(serde_json::json!({
            "checkpoint": stored.id.short(),
            "session": stored.checkpoint.session.as_str(),
            "harness": stored.checkpoint.harness,
            "trimmed": stored.checkpoint.trimmed,
            "document": if document {
                stored.checkpoint.render()
            } else {
                stored.checkpoint.bootstrap_prompt()
            },
        })),
        Ok(None) => Response::err("this project has no checkpoints yet"),
        Err(err) => Response::err(checkpoint_store_err(err)),
    }
}

fn checkpoint_store_err(err: CheckpointStoreError) -> String {
    err.to_string()
}

/// Take a checkpoint — box 11.
///
/// Mirrors `main.rs`'s `CheckpointCommand::Save` arm: the same session
/// resolution (`crate::commands::resume::active_session`, named or the project's most recently
/// active), the same `Checkpoint::capture`, the same store. Duplicated
/// rather than called through because that arm prints to standard output as
/// part of returning an `ExitCode`, which has nothing to do with what this
/// door writes to a socket.
#[allow(clippy::too_many_arguments)]
pub(super) fn request_checkpoint(
    runtime: &Runtime,
    sessions: &ProjectSessions,
    session: Option<&str>,
    objective: String,
    implementation_state: String,
    decisions: Vec<String>,
    failed_approaches: Vec<String>,
    files: Vec<String>,
    test_state: Option<String>,
    next_actions: Vec<String>,
) -> Response {
    let record = match crate::commands::resume::active_session(sessions, session) {
        Ok(Some(record)) => record,
        Ok(None) => {
            return Response::err(
                "this project has no recorded sessions to check point; start one first",
            );
        }
        Err(err) => return Response::err(err),
    };

    let checkpoints = match ProjectCheckpoints::open(runtime) {
        Ok(checkpoints) => checkpoints,
        Err(err) => return Response::err(err),
    };
    let store = checkpoints.store();

    let checkpoint = Checkpoint::capture(
        &record.id,
        &record.harness,
        CheckpointReason::Manual,
        store.now(),
        runtime.project().root(),
        Handoff {
            objective,
            implementation_state,
            decisions,
            memory: binding_memory_lines(runtime),
            failed_approaches,
            files,
            test_state,
            next_actions,
        },
    );

    match store.save(checkpoint) {
        Ok(stored) => Response::ok(serde_json::json!({
            "checkpoint": stored.id.short(),
            "session": record.id.as_str(),
            "trimmed": stored.checkpoint.trimmed,
        })),
        Err(err) => Response::err(err),
    }
}
