//! Process control.
//!
//! Terminating a process is one of the few things that genuinely differs
//! between platforms: Unix has signals and process groups, Windows has neither.
//! The difference is confined to this file so callers only ever see
//! [`ProcessSignal`].

use portable_pty::{Child, ChildKiller, MasterPty};

/// What to do to a running process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSignal {
    /// Ask the process to shut down and let it clean up.
    ///
    /// On Unix this is `SIGTERM` to the process group. Windows has no
    /// equivalent request, so it is the same forceful termination as
    /// [`ProcessSignal::Kill`]; the difference is documented rather than
    /// pretended away.
    Terminate,
    /// End the process immediately without giving it a chance to clean up.
    Kill,
}

#[derive(Debug, thiserror::Error)]
pub enum SignalError {
    #[error("the process has already exited")]
    AlreadyExited,
    #[error("could not signal the process: {0}")]
    Os(#[from] std::io::Error),
}

/// Deliver `signal` to the child process and, where the platform supports it,
/// everything it started.
pub(crate) fn signal_process(
    signal: ProcessSignal,
    child: &(dyn Child + Send + Sync),
    master: &(dyn MasterPty + Send),
    killer: &mut (dyn ChildKiller + Send + Sync),
) -> Result<(), SignalError> {
    #[cfg(unix)]
    {
        // The killer is only needed where signals are unavailable.
        let _ = killer;

        // Prefer the pty's session leader: it is the process group the terminal
        // actually drives, so signalling it reaches the harness and every tool
        // it spawned rather than only the top-level process.
        let group = master
            .process_group_leader()
            .or_else(|| child.process_id().map(|pid| pid as libc::pid_t));

        let Some(group) = group else {
            return Err(SignalError::AlreadyExited);
        };

        let sig = match signal {
            ProcessSignal::Terminate => libc::SIGTERM,
            ProcessSignal::Kill => libc::SIGKILL,
        };

        // SAFETY: `kill` with a negative pid targets the process group with
        // that id. The value came from the pty session leader or the child's
        // own pid, both of which identify this child's group.
        let result = unsafe { libc::kill(-group, sig) };
        if result != 0 {
            let err = std::io::Error::last_os_error();
            // The group is gone: the process exited between the liveness check
            // and the signal. That is a race, not a failure.
            if err.raw_os_error() == Some(libc::ESRCH) {
                return Err(SignalError::AlreadyExited);
            }
            return Err(SignalError::Os(err));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = (signal, child, master);
        killer.kill().map_err(SignalError::Os)
    }
}
