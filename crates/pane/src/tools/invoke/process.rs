//! Observe exit without reaping, so the owned process-group id cannot be
//! recycled before remaining descendants are killed.
use std::io;
use std::process::{Child, ExitStatus};

pub(super) fn try_complete(child: &mut Child) -> io::Result<Option<ExitStatus>> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        if !exited(child.id())? {
            return Ok(None);
        }
        // WNOWAIT kept the leader waitable and reserves its pid/group id.
        // Its recorded exit status is unchanged by signalling the descendants.
        super::kill_group(child.id());
        child.wait().map(Some)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    child.try_wait()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn exited(pid: u32) -> io::Result<bool> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    observed(result, info.si_signo != 0)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn observed(result: i32, exited: bool) -> io::Result<bool> {
    if result == 0 {
        return Ok(exited);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::Interrupted {
        Ok(false)
    } else {
        Err(error)
    }
}
