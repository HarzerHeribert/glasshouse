//! `commands::checkpoint` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use std::process::ExitCode;

use glasshouse::Runtime;
use glasshouse::checkpoint::{
    Checkpoint, CheckpointReason, CheckpointStore, Handoff, ProjectCheckpoints, Stored,
};
use glasshouse::cli::CheckpointCommand;
use glasshouse::session::ProjectSessions;

/// `glasshouse checkpoint …`.
///
/// # What Glasshouse supplies, and what it refuses to
///
/// The session, the harness, the timestamp and the Git position are read
/// straight off the project and the repository. The objective, the state, the
/// decisions and the next actions are **arguments**, because they are things
/// only whoever did the work knows. Glasshouse could have filled them from a
/// session's terminal output and it deliberately does not: a checkpoint whose
/// objective was guessed from scrollback would be a confident fiction, and
/// this project already refuses to read state out of terminal output
/// everywhere else.
pub(crate) fn checkpoint_command(
    runtime: &Runtime,
    command: &CheckpointCommand,
) -> anyhow::Result<ExitCode> {
    let checkpoints = ProjectCheckpoints::open(runtime)?;
    let store = checkpoints.store();

    match command {
        CheckpointCommand::Save {
            objective,
            state,
            session,
            decisions,
            failed_approaches,
            files,
            tests,
            next_actions,
        } => {
            let sessions = ProjectSessions::open(runtime)?;
            let Some(record) =
                crate::commands::resume::active_session(&sessions, session.as_deref())?
            else {
                eprintln!(
                    "glasshouse: this project has no recorded sessions to check point. \
                     Start one with `glasshouse launch`."
                );
                return Ok(ExitCode::FAILURE);
            };

            let stored = store.save(Checkpoint::capture(
                &record.id,
                &record.harness,
                CheckpointReason::Manual,
                store.now(),
                runtime.project().root(),
                Handoff {
                    objective: objective.clone(),
                    implementation_state: state.clone(),
                    decisions: decisions.clone(),
                    memory: crate::commands::resume::binding_memory_lines(runtime),
                    failed_approaches: failed_approaches.clone(),
                    files: files.clone(),
                    test_state: tests.clone(),
                    next_actions: next_actions.clone(),
                },
            ))?;

            println!("checkpoint {}", stored.id.short());
            println!(
                "session    {}",
                crate::commands::shared::short_id(&record.id)
            );
            match &stored.checkpoint.git {
                Some(git) => match &git.branch {
                    Some(branch) => println!("git        {branch} at {}", git.commit),
                    None => println!("git        detached at {}", git.commit),
                },
                // Said out loud rather than left blank: "when available" is a
                // real condition, and a silent omission reads as a bug.
                None => println!("git        no repository position available"),
            }
            if stored.checkpoint.trimmed {
                println!(
                    "note       trimmed to fit {} bytes; the session has more",
                    glasshouse::checkpoint::MAX_BYTES
                );
            }
            println!(
                "\nStart a session anywhere from it with:\n  glasshouse launch <harness> \
                 --from-checkpoint {}",
                stored.id.short()
            );
        }
        CheckpointCommand::List => {
            print!("{}", checkpoint_listing(&store)?);
        }
        CheckpointCommand::Show {
            checkpoint,
            document,
        } => {
            let Some(stored) = resolve_checkpoint(&store, checkpoint.as_deref())? else {
                eprintln!("glasshouse: this project has no checkpoints yet.");
                return Ok(ExitCode::FAILURE);
            };
            if *document {
                println!("{}", stored.checkpoint.render());
            } else {
                print!("{}", stored.checkpoint.bootstrap_prompt());
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The checkpoint a command means: the one named, or the most recent.
pub(crate) fn resolve_checkpoint(
    store: &CheckpointStore<'_>,
    named: Option<&str>,
) -> anyhow::Result<Option<Stored>> {
    match named {
        Some("latest") | None => Ok(store.latest()?),
        Some(named) => {
            let id = store.resolve_id(named)?;
            Ok(Some(store.get(&id)?.ok_or_else(|| {
                anyhow::anyhow!("checkpoint `{id}` is not in this project")
            })?))
        }
    }
}

/// The `glasshouse checkpoint list` listing.
fn checkpoint_listing(store: &CheckpointStore<'_>) -> anyhow::Result<String> {
    use std::fmt::Write as _;

    let stored = store.list()?;
    if stored.is_empty() {
        return Ok("No checkpoints recorded for this project.\n\
                   Take one with `glasshouse checkpoint save --objective ... \
                   --state ...`.\n"
            .to_owned());
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}",
        checkpoint_row(
            "CHECKPOINT",
            "SESSION",
            "HARNESS",
            "WHY",
            "TAKEN",
            "OBJECTIVE"
        )
    );
    for entry in &stored {
        let _ = writeln!(
            out,
            "{}",
            checkpoint_row(
                &entry.id.short(),
                &crate::commands::shared::short_id(&entry.checkpoint.session),
                &entry.checkpoint.harness,
                entry.checkpoint.reason.as_str(),
                &crate::commands::shared::format_age(entry.checkpoint.created_at),
                &crate::commands::shared::one_line(&entry.checkpoint.handoff.objective),
            )
        );
    }
    Ok(out)
}

/// One line of the checkpoint listing, header included.
///
/// The header and the rows go through one function so their columns cannot
/// drift apart, exactly as [`session_row`] does — the usual way a hand-aligned
/// table stops lining up is somebody widening a column in one of two format
/// strings.
fn checkpoint_row(
    checkpoint: &str,
    session: &str,
    harness: &str,
    reason: &str,
    taken: &str,
    objective: &str,
) -> String {
    format!(
        "{checkpoint:<12}  {session:<12}  {harness:<14}  {reason:<13}  {taken:<10}  {objective}"
    )
}
