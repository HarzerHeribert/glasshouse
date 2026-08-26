//! Process control.
//!
//! Terminating a process is one of the few things that genuinely differs
//! between platforms: Unix has signals and process groups, Windows has
//! neither natively — `JobHandle` is what gives Windows an equivalent to
//! "kill the whole tree". The difference is confined to this file so callers
//! only ever see [`ProcessSignal`].

#[cfg(unix)]
use portable_pty::Child;
#[cfg(windows)]
use portable_pty::ChildKiller;

#[cfg(windows)]
use std::io;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};

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

/// Deliver `signal` to the child process and everything it started.
#[cfg(unix)]
pub(crate) fn signal_process(
    signal: ProcessSignal,
    child: &(dyn Child + Send + Sync),
) -> Result<(), SignalError> {
    // Earlier versions of this function preferred
    // `MasterPty::process_group_leader()`, which portable-pty implements as
    // `tcgetpgrp(master_fd)` — the terminal's *current foreground* process
    // group. That is not reliably the harness's group: a shell that does its
    // own job control (e.g. `bash -c 'set -m; sleep 30; ...'`) hands the
    // terminal's foreground group to whatever job it just started, so
    // signalling that group can hit only a subprocess and leave the session
    // leader running untouched. Use the child's own pid instead — see the
    // SAFETY note on the `kill` call below for why that is always the right
    // group id to use.
    let Some(group) = child.process_id().map(|pid| pid as libc::pid_t) else {
        return Err(SignalError::AlreadyExited);
    };

    let sig = match signal {
        ProcessSignal::Terminate => libc::SIGTERM,
        ProcessSignal::Kill => libc::SIGKILL,
    };

    // SAFETY: `kill` with a negative pid targets the process group with that
    // id. portable-pty's Unix spawn path runs `setsid()` in a `pre_exec`
    // hook before exec (`portable_pty::unix`, `PtyFd::spawn_command`), which
    // makes the child both a session leader and the leader of its own
    // process group. A process group leader's pid and its process group id
    // are the same number by definition, so the child's own pid is exactly
    // the group id that reaches it and everything it has spawned.
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

/// Deliver `signal` to the child process on Windows.
///
/// Windows has no signal-to-a-group primitive, so "Terminate" and "Kill" are
/// the same forceful action here — see [`ProcessSignal::Terminate`].
#[cfg(windows)]
pub(crate) fn signal_process(
    _signal: ProcessSignal,
    job: Option<&JobHandle>,
    killer: &mut (dyn ChildKiller + Send + Sync),
) -> Result<(), SignalError> {
    // Prefer the job object: it reaches every process ever assigned to it,
    // not just the direct child. Glasshouse's harnesses are npm-installed
    // `.cmd` shims, launched as `cmd.exe /C harness.cmd ...`, so the direct
    // child is `cmd.exe` and the real long-running process (`node.exe`) is a
    // grandchild — only the job-object path can reach it. See [`JobHandle`].
    if let Some(job) = job {
        if job.terminate().is_ok() {
            return Ok(());
        }
        // TerminateJobObject itself failed (distinct from AssignProcessToJobObject
        // having failed back at spawn time, which would have left `job` as
        // `None`). Fall through to the direct killer as a last resort.
    }

    // portable-pty 0.9.0's `WinChildKiller::kill` has its result inverted: it
    // calls `TerminateProcess`, which returns *non-zero on success*, but
    // then does `if res != 0 { Err(err) } else { Ok(()) }` — so a successful
    // kill is reported as `Err` and a failed one as `Ok`
    // (`portable_pty::win::WinChildKiller::kill`, portable-pty 0.9.0). There
    // is no way to distinguish a real failure from the inversion from out
    // here, so the result is treated as advisory only: call it, and report
    // success regardless. Remove this once the upstream bug is fixed and the
    // dependency is bumped past the fix.
    let _ = killer.kill();
    Ok(())
}

/// A Windows Job Object that makes a whole process tree killable as a unit.
///
/// portable-pty's Windows kill paths (`TerminateProcess` via
/// `WinChild`/`WinChildKiller`) only ever reach the one process handle they
/// hold. Glasshouse's harnesses are npm-installed `.cmd` shims, so the
/// process Glasshouse actually spawns is `cmd.exe /C harness.cmd ...`: the
/// direct child is `cmd.exe`, and the real long-running process (`node.exe`)
/// is a grandchild that would otherwise be orphaned and keep running after
/// `cmd.exe` dies. Putting the direct child in a job object created with
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` fixes that: [`JobHandle::terminate`]
/// reaches everything the job has ever contained, and even if nobody calls
/// `terminate`, dropping this handle does the same thing as a safety net.
#[cfg(windows)]
pub(crate) struct JobHandle(HANDLE);

// SAFETY: a Windows HANDLE is an opaque kernel object reference with no
// thread affinity, so moving or sharing this wrapper across threads is
// sound. Nothing in this module duplicates the underlying handle outside of
// `assign`, so shared access is always to the single owning value.
#[cfg(windows)]
unsafe impl Send for JobHandle {}
#[cfg(windows)]
unsafe impl Sync for JobHandle {}

#[cfg(windows)]
impl JobHandle {
    /// Create a new job object with `KILL_ON_JOB_CLOSE` set and assign
    /// `process` to it.
    ///
    /// This can legitimately fail — for example, `process` may already be in
    /// a job that disallows nested jobs on older Windows releases — and
    /// callers must treat that as "the job-object trick is unavailable for
    /// this child", not as a reason to fail the spawn: the existing
    /// direct-kill path still works, it simply cannot reach grandchildren.
    pub(crate) fn assign(process: HANDLE) -> io::Result<Self> {
        // SAFETY: a null security-attributes pointer and a null name is the
        // documented way to create an anonymous job object with default
        // security, owned solely by this process.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // Wrap immediately so an early return below still closes the handle
        // via `JobHandle`'s `Drop`.
        let job = JobHandle(handle);

        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        // SAFETY: `info` is a plain, fixed-layout struct; its address and
        // exact size are what `SetInformationJobObject` expects for the
        // `JobObjectExtendedLimitInformation` class.
        let configured = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: `job.0` was just created above and `process` is a live
        // process handle supplied by the caller (portable-pty's own child
        // handle, via `Child::as_raw_handle`).
        let assigned = unsafe { AssignProcessToJobObject(job.0, process) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(job)
    }

    /// Immediately terminate every process currently assigned to this job.
    pub(crate) fn terminate(&self) -> io::Result<()> {
        // SAFETY: `self.0` is a valid job handle for the lifetime of `self`.
        let ok = unsafe { TerminateJobObject(self.0, 1) };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
impl Drop for JobHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a valid, uniquely-owned handle, so closing it
        // is always sound. Because the job was created with
        // `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, closing the last (only)
        // handle to it also terminates anything still assigned to it — the
        // safety net that fires even when `terminate` was never called.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
