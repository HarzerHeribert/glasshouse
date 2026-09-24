//! A shell command handed back to its caller while it still runs.
//!
//! **The invariant: a handed-over child is the same child.** The foreground
//! call admitted, gated, confined and spawned it; [`Running`] carries the
//! process, the group it leads and both pipe readers to whoever finishes the
//! wait, and [`Running::finish`] is the wait [`super::spawn_confined`] would
//! otherwise have done itself -- the same poll, the same group kill.

use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Duration;

use super::{CANCEL_POLL, ConfinedChild, ToolError, collect, kill_and_reap, process};

/// A shell child still running when its caller's bound passed, handed over
/// whole: the process, the group it leads and both pipe readers.
///
/// **Nothing about the child changes at the hand-over** -- the same sandbox,
/// the same process group, the same kill. Whoever takes it finishes the wait
/// with [`Running::finish`], which is the wait [`super::spawn_confined`] would
/// otherwise have done itself.
pub(crate) struct Running {
    pub(super) child: ConfinedChild,
    pub(super) stdout: JoinHandle<Vec<u8>>,
    pub(super) stderr: JoinHandle<Vec<u8>>,
}

impl Running {
    /// Waits the child out and reads both pipes, or kills its whole group
    /// the moment `stopped` answers true. The exit code is `None` for a
    /// child killed by a signal, as it is for a call that waited in place.
    pub(crate) fn finish(
        mut self,
        stopped: &dyn Fn() -> bool,
    ) -> Result<(Option<i32>, String, String), ToolError> {
        let status = match wait_child(&mut self.child, stopped, None) {
            Waited::Exited(status) => status,
            Waited::Cancelled | Waited::Due => return Err(cancelled_bash()),
            Waited::Failed(error) => {
                return Err(ToolError::Spawn {
                    tool: "bash".to_string(),
                    program: PathBuf::from("bash"),
                    error: error.to_string(),
                });
            }
        };
        if !drained(&self.stdout, &self.stderr, stopped) {
            return Err(cancelled_bash());
        }
        Ok((status.code(), collect(self.stdout), collect(self.stderr)))
    }
}

fn cancelled_bash() -> ToolError {
    ToolError::Cancelled {
        tool: "bash".to_string(),
    }
}

/// How long a caller lets one shell command run in place before it takes the
/// running child back as [`Running`] and the call returns.
///
/// The call's own result then carries no output and no exit code; the caller
/// finds the child in `handed` and owns it from there. Only a shell command
/// is ever handed over: every other tool is a bounded read that finishes in
/// place.
pub(crate) struct Yielding {
    pub(crate) after: Duration,
    pub(crate) handed: std::cell::RefCell<Option<Running>>,
}

impl Yielding {
    pub(crate) fn after(after: Duration) -> Self {
        Self {
            after,
            handed: std::cell::RefCell::new(None),
        }
    }
}

/// How one wait on a child ended.
pub(super) enum Waited {
    Exited(std::process::ExitStatus),
    Cancelled,
    Failed(std::io::Error),
    /// The caller's bound passed with the child still running.
    Due,
}

/// Polls `child` until it exits, `stopped` answers (the whole group is
/// killed), the wait itself fails (killed too), or `due` passes with the
/// child still running -- which leaves it untouched for the caller.
pub(super) fn wait_child(
    child: &mut ConfinedChild,
    stopped: &dyn Fn() -> bool,
    due: Option<std::time::Instant>,
) -> Waited {
    loop {
        if stopped() {
            kill_and_reap(child);
            return Waited::Cancelled;
        }
        match process::try_complete(child) {
            Ok(Some(status)) => return Waited::Exited(status),
            Ok(None) => {
                if due.is_some_and(|due| std::time::Instant::now() >= due) {
                    return Waited::Due;
                }
                std::thread::sleep(CANCEL_POLL);
            }
            Err(error) => {
                kill_and_reap(child);
                return Waited::Failed(error);
            }
        }
    }
}

/// Waits for both pipe readers, cancellably: a process that escaped the
/// group must not hold the caller through a pipe it inherited. `false` when
/// `stopped` answered first.
pub(super) fn drained(
    stdout: &JoinHandle<Vec<u8>>,
    stderr: &JoinHandle<Vec<u8>>,
    stopped: &dyn Fn() -> bool,
) -> bool {
    while !stdout.is_finished() || !stderr.is_finished() {
        if stopped() {
            return false;
        }
        std::thread::sleep(CANCEL_POLL);
    }
    true
}
