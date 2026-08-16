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

#[cfg(all(test, target_os = "macos"))]
#[allow(unsafe_code)] // test-local fork/pipe/_exit shims, mirroring the seccomp regressions
mod macos_waitid_tests {
    //! Focused coverage of the macOS `waitid` adapter's `siginfo_t` ->
    //! [`WaitStatus`] translation, against real child state changes (never a
    //! fabricated status). The adapter exists only because `nix` does not
    //! expose `waitid` on Apple targets, so its mapping is what these tests
    //! pin: `CLD_EXITED` -> `Exited`, `CLD_KILLED` -> `Signaled`, and the
    //! `WNOHANG` zero-pid case -> `StillAlive`.
    //!
    //! Ordering is a pipe handshake, never a sleep: the child blocks on a
    //! pipe until the parent releases it, so the parent can observe a
    //! deterministic `StillAlive` state before forcing the transition.

    use super::{Id, waitid};
    use nix::errno::Errno;
    use nix::sys::signal::Signal;
    use nix::sys::wait::{WaitPidFlag, WaitStatus};
    use nix::unistd::Pid;

    /// Forks a child that blocks reading one byte from a pipe, then returns
    /// the child pid and the parent's write end. The child performs no work
    /// until the parent releases it, so the parent can deterministically
    /// observe `StillAlive` before forcing an exit or a signal.
    fn fork_blocked_child() -> (Pid, i32) {
        let mut fds = [0i32; 2];
        // SAFETY: a scalar pipe allocation; the two ends are closed
        // deterministically on each side of the fork below.
        assert_eq!(
            unsafe { libc::pipe(fds.as_mut_ptr()) },
            0,
            "pipe: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: the child blocks on the pipe, never returns into the
        // multi-threaded test process, and is reaped by the parent via the
        // adapter under test.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed: {}", std::io::Error::last_os_error());
        if pid == 0 {
            // SAFETY: the child only reads from its pipe end and then exits.
            unsafe {
                libc::close(fds[1]);
                let mut byte = 0u8;
                let _ = libc::read(fds[0], (&raw mut byte).cast::<libc::c_void>(), 1);
                libc::_exit(7);
            }
        }
        // SAFETY: the parent closes its read end; it owns only the write end.
        unsafe { libc::close(fds[0]) };
        (Pid::from_raw(pid), fds[1])
    }

    /// Polls `waitid` until it stops reporting `StillAlive`, with a strict
    /// deadlock guard (a liveness guard, never a correctness assertion).
    fn wait_for_change(pid: Pid) -> WaitStatus {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let status = waitid(Id::Pid(pid), WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED)
                .expect("waitid observes the child");
            if !matches!(status, WaitStatus::StillAlive) {
                return status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the child never reached the expected state"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn exited_child_is_translated_with_its_exit_code() {
        let (pid, write_end) = fork_blocked_child();
        // The child is blocked on the pipe: WNOHANG deterministically reports
        // no state change, which the adapter must translate to `StillAlive`.
        let status = waitid(Id::Pid(pid), WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED)
            .expect("waitid observes the blocked child");
        assert_eq!(status, WaitStatus::StillAlive);
        // Release the child: it exits with code 7 and the adapter translates
        // the resulting `CLD_EXITED` siginfo.
        // SAFETY: a single scalar write to the pipe owned by this test; the
        // pointer targets a live `u8` for the duration of the call.
        assert_eq!(
            unsafe { libc::write(write_end, std::ptr::from_ref(&1u8).cast(), 1) },
            1
        );
        let status = wait_for_change(pid);
        assert_eq!(status, WaitStatus::Exited(pid, 7));
        // The child is now gone: a further pid-scoped wait reports ECHILD,
        // which the adapter must propagate rather than fabricate a status.
        assert_eq!(
            waitid(Id::Pid(pid), WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED),
            Err(Errno::ECHILD)
        );
        // SAFETY: close the pipe's write end now that the child was reaped.
        unsafe { libc::close(write_end) };
    }

    #[test]
    fn killed_child_is_translated_with_its_signal() {
        let (pid, write_end) = fork_blocked_child();
        let status = waitid(Id::Pid(pid), WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED)
            .expect("waitid observes the blocked child");
        assert_eq!(status, WaitStatus::StillAlive);
        nix::sys::signal::kill(pid, Signal::SIGKILL).expect("kill the blocked child");
        let status = wait_for_change(pid);
        assert_eq!(status, WaitStatus::Signaled(pid, Signal::SIGKILL, false));
        // SAFETY: close the pipe's write end now that the child was reaped.
        unsafe { libc::close(write_end) };
    }
}
