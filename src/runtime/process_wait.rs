//! The small platform adapter for the `waitid` primitive used by the
//! supervised process units.
//!
//! `nix` exposes its `waitid` wrapper on Linux but not on Apple targets even
//! though macOS provides the same POSIX primitive. Keep the Linux path on
//! the established `nix` implementation and translate macOS `siginfo_t`
//! results into the same `WaitStatus` vocabulary used by the supervisors.

#[cfg(target_os = "macos")]
mod macos {
    use nix::errno::Errno;
    use nix::sys::signal::Signal;
    use nix::sys::wait::{WaitPidFlag, WaitStatus};
    use nix::unistd::Pid;

    /// The process or process-group selector accepted by `waitid(2)`.
    #[derive(Debug, Clone, Copy)]
    pub(crate) enum Id {
        /// Select one process.
        Pid(Pid),
        /// Select children in one process group.
        PGid(Pid),
    }

    /// Waits for a macOS child or child in a process group and translates the
    /// result into the `nix` wait-status type shared by the supervisors.
    #[allow(unsafe_code)]
    pub(crate) fn waitid(id: Id, flags: WaitPidFlag) -> nix::Result<WaitStatus> {
        let (id_type, id_value) = match id {
            Id::Pid(pid) => (
                libc::P_PID,
                libc::id_t::try_from(pid.as_raw()).map_err(|_| Errno::EINVAL)?,
            ),
            Id::PGid(pgid) => (
                libc::P_PGID,
                libc::id_t::try_from(pgid.as_raw()).map_err(|_| Errno::EINVAL)?,
            ),
        };
        // macOS leaves the siginfo payload unspecified for WNOHANG when no
        // child has changed state, so zero it before the libc call.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        Errno::result(unsafe { libc::waitid(id_type, id_value, &raw mut info, flags.bits()) })?;

        let child_pid = info.si_pid;
        if child_pid == 0 {
            return Ok(WaitStatus::StillAlive);
        }
        let child = Pid::from_raw(child_pid);
        let status = info.si_status;
        match info.si_code {
            libc::CLD_EXITED => Ok(WaitStatus::Exited(child, status)),
            libc::CLD_KILLED | libc::CLD_DUMPED => Ok(WaitStatus::Signaled(
                child,
                Signal::try_from(status)?,
                info.si_code == libc::CLD_DUMPED,
            )),
            libc::CLD_STOPPED | libc::CLD_TRAPPED => {
                Ok(WaitStatus::Stopped(child, Signal::try_from(status)?))
            }
            libc::CLD_CONTINUED => Ok(WaitStatus::Continued(child)),
            _ => Err(Errno::EINVAL),
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::{Id, waitid};

#[cfg(not(target_os = "macos"))]
pub(crate) use nix::sys::wait::{Id, waitid};
