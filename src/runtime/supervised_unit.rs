//! The shared structural ownership core of rustX-owned supervised process
//! units.
//!
//! Every real production subprocess hierarchy — the M5 short-lived Bash
//! supervisor unit and the M7 long-lived interactive MCP stdio supervisor
//! unit — is composed from exactly this core, so both domains share the
//! same physical ownership guarantees:
//!
//! - one kernel-mediated terminal proof: the group-scoped wait
//!   (`waitid(Id::PGid)` returning `ECHILD`) is complete only because
//!   membership is immutable for unit descendants;
//! - the fixed-membership restriction: an inherited seccomp filter rejects
//!   `setsid(2)`/`setpgid(2)` with `EPERM`, so no owned descendant can
//!   escape the unit's process group/session;
//! - the runtime child-subreaper prerequisite, installed before the unit
//!   spawns (`crate::runtime::process_supervision`) and re-established
//!   inside each supervisor process so orphaned descendants reparent into
//!   the unit's reaping domain;
//! - the single-reaper anchor discipline: the inner supervisor pid is the
//!   structural ownership anchor with exactly one reaping owner, and
//!   fallback containment signals are issued only while the anchor is
//!   retained;
//! - `TERM` -> grace -> `KILL` against the inner leader's own process
//!   group, whose numeric id is the inner's pid — provably allocated while
//!   signaling is legal;
//! - catastrophic fallback containment (adopted-anchor retention with
//!   `WNOWAIT`, one anchored `SIGKILL`, group-scoped `ECHILD` release) when
//!   the inner supervisor or control chain fails;
//! - one frame protocol (`[u32 LE length][kind][payload]`) for all
//!   supervisor control traffic, separate from the unit's business I/O.
//!
//! The two domains differ only in business I/O and life span: the Bash
//! unit's control channel is its inherited socket pair on fd 0 while the
//! interactive unit's control channels are private Unix sockets; the Bash
//! unit executes `/bin/bash -c <command>` while the interactive unit
//! executes a long-lived server with real stdin/stdout protocol streams.

use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

/// The nix Unix stream used by the supervisor control channels. The
/// `socket` feature of `nix` is not enabled for the whole crate; the
/// interactive supervisor unit binds/accepts with `std::os::unix::net`
/// and converts, so this module only needs the stream type alias.
#[cfg(unix)]
pub(crate) type ControlStream = std::os::unix::net::UnixStream;

/// The outer supervisor role name.
pub(crate) const ROLE_OUTER: &str = "outer";
/// The inner supervisor role name.
pub(crate) const ROLE_INNER: &str = "inner";

/// The shell's canonical exit status frame kind:
/// `{ exit_code: i32 LE, signaled: u8, signal: i32 LE }`.
pub(crate) const MSG_SHELL_EXITED: u8 = 0x02;
/// All unit-owned children are reaped (kernel `ECHILD` reached).
pub(crate) const MSG_ALL_CHILDREN_REAPED: u8 = 0x03;
/// A process-control failure; payload is the human-readable message.
pub(crate) const MSG_PROCESS_CONTROL_FAILURE: u8 = 0x04;
/// One attempted group signal for test observability:
/// `{ pgid: i32 LE, signal: i32 LE, emitted: u8 }`.
pub(crate) const MSG_SIGNAL_ATTEMPT: u8 = 0x05;
/// Inner -> owner: setup is complete and the owned child has not yet been
/// spawned; payload is the unit PGID (`i32 LE`).
pub(crate) const MSG_ANCHOR_READY: u8 = 0x06;
/// Inner -> owner: the owned child was successfully spawned in the fixed
/// group.
pub(crate) const MSG_OWNERSHIP_ESTABLISHED: u8 = 0x07;
/// Supervisor -> owner: setup ended before any owned process was spawned.
pub(crate) const MSG_NO_OWNERSHIP: u8 = 0x08;
/// Owner -> supervisor: run the `TERM` -> grace -> `KILL` sequence.
pub(crate) const MSG_TERMINATE: u8 = 0x10;
/// Owner -> inner: the owner retained the anchor identity and authorizes
/// the owned-child spawn.
pub(crate) const MSG_START: u8 = 0x11;
/// Owner -> outer: the authoritative terminal frame was parsed.
pub(crate) const MSG_TERMINAL_ACK: u8 = 0x12;

/// The inner supervisor's exit status for a normal completion: it reached
/// the kernel `ECHILD` terminal child state (or no owned process tree was
/// ever created), so the outer supervisor only needs to reap it and its
/// child domain is provably empty.
pub(crate) const INNER_EXIT_NORMAL: i32 = 0;
/// The inner supervisor's exit status for an abnormal termination with
/// possibly-live owned work: the outer supervisor must actively contain
/// the unit process group (one structurally-anchored fallback `SIGKILL`)
/// before reaping.
pub(crate) const INNER_EXIT_CONTAINMENT: i32 = 42;

/// The internal poll cadence of the supervisor loops (an implementation
/// detail of the grace period and the wait loops — never a test
/// synchronization mechanism).
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The `TERM` -> `KILL` grace period, kept in sync with
/// `crate::tools::limits::BASH_TERM_GRACE`.
pub(crate) const TERM_GRACE: Duration = Duration::from_secs(2);

/// The deadline after which a missing terminal-ack frame is ignored (the
/// owner may have disappeared; terminality was already proven).
pub(crate) const TERMINAL_ACK_TIMEOUT: Duration = Duration::from_secs(2);

/// `PR_SET_CHILD_SUBREAPER`: orphaned descendants of the owned child
/// reparent into this process's child domain instead of being rediscovered
/// from `/proc`.
///
/// This is one of the narrowly scoped production OS shims shared by both
/// supervisor units: subreaper setup, SIGTERM handler installation,
/// `PR_SET_NO_NEW_PRIVS`, and seccomp filter installation. Linux-only: the
/// lifecycle contract is claimed only where the kernel provides the
/// subreaper mechanism.
#[allow(unsafe_code)]
pub(crate) fn become_child_subreaper() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: prctl with PR_SET_CHILD_SUBREAPER and a literal 1 is a
        // single scalar syscall with no pointer arguments.
        let result = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("the supervisor unit requires Linux (PR_SET_CHILD_SUBREAPER)".to_owned())
    }
}

/// The no-op `SIGTERM` handler of the inner supervisor: the unit group
/// `TERM` must not kill the inner supervisor while the owned child and its
/// descendants handle it.
///
/// A **caught** handler (not `SIG_IGN`) is required: `exec` resets caught
/// dispositions to the default, so the owned child starts with a default
/// `SIGTERM` disposition and its own `trap`/signal handlers stay effective.
/// An ignored signal would be inherited by the child.
extern "C" fn ignore_sigterm(_signal: libc::c_int) {}

#[allow(unsafe_code)]
pub(crate) fn ignore_group_term() -> Result<(), String> {
    // SAFETY: installing a no-op handler with no pointer payload is a
    // single scalar libc call; the handler never dereferences anything.
    let handler = ignore_sigterm as extern "C" fn(libc::c_int) as libc::sighandler_t;
    let result = unsafe { libc::signal(libc::SIGTERM, handler) };
    if result == libc::SIG_ERR {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

/// The `AUDIT_ARCH` constant of the compiled architecture, used by the
/// seccomp filter to reject syscalls from an unexpected ABI before any
/// other check.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const AUDIT_ARCH: u32 = 0xC000_003E; // AUDIT_ARCH_X86_64
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const AUDIT_ARCH: u32 = 0xC000_00B7; // AUDIT_ARCH_AARCH64
#[cfg(all(target_os = "linux", target_arch = "riscv64"))]
const AUDIT_ARCH: u32 = 0xC000_00F3; // AUDIT_ARCH_RISCV64

/// The `sock_filter` BPF program of the fixed-membership restriction:
///
/// ```text
/// 0: load seccomp_data.arch
/// 1: if arch == the compiled AUDIT_ARCH -> 2, else -> last (kill, fail closed)
/// 2: load seccomp_data.nr
/// 3 (x86-64 only): if the x32 bit is set -> EPERM
/// next: if nr == setpgid -> EPERM
/// next: if nr == setsid -> EPERM
/// next: allow
/// next: return EPERM
/// last: kill (foreign audit architecture)
/// ```
///
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn membership_restriction_program() -> [libc::sock_filter; 9] {
    use libc::{BPF_ABS, BPF_JEQ, BPF_JMP, BPF_JSET, BPF_K, BPF_LD, BPF_RET, BPF_W};
    [
        libc::sock_filter {
            code: (BPF_LD | BPF_W | BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 4,
        },
        libc::sock_filter {
            code: (BPF_JMP | BPF_JEQ | BPF_K) as u16,
            jt: 0,
            jf: 6,
            k: AUDIT_ARCH,
        },
        libc::sock_filter {
            code: (BPF_LD | BPF_W | BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 0,
        },
        libc::sock_filter {
            code: (BPF_JMP | BPF_JSET | BPF_K) as u16,
            jt: 3,
            jf: 0,
            k: 0x4000_0000, // __X32_SYSCALL_BIT
        },
        libc::sock_filter {
            code: (BPF_JMP | BPF_JEQ | BPF_K) as u16,
            jt: 2,
            jf: 0,
            k: libc::SYS_setpgid as u32,
        },
        libc::sock_filter {
            code: (BPF_JMP | BPF_JEQ | BPF_K) as u16,
            jt: 1,
            jf: 0,
            k: libc::SYS_setsid as u32,
        },
        libc::sock_filter {
            code: (BPF_RET | BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: 0x7FFF_0000, // SECCOMP_RET_ALLOW
        },
        libc::sock_filter {
            code: (BPF_RET | BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: 0x0005_0000 | libc::EPERM as u32, // SECCOMP_RET_ERRNO | EPERM
        },
        libc::sock_filter {
            code: (BPF_RET | BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: 0x8000_0000, // SECCOMP_RET_KILL_PROCESS
        },
    ]
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "aarch64", target_arch = "riscv64")
))]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn membership_restriction_program() -> [libc::sock_filter; 8] {
    use libc::{BPF_ABS, BPF_JEQ, BPF_JMP, BPF_K, BPF_LD, BPF_RET, BPF_W};
    [
        libc::sock_filter {
            code: (BPF_LD | BPF_W | BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 4,
        },
        libc::sock_filter {
            code: (BPF_JMP | BPF_JEQ | BPF_K) as u16,
            jt: 0,
            jf: 5,
            k: AUDIT_ARCH,
        },
        libc::sock_filter {
            code: (BPF_LD | BPF_W | BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 0,
        },
        libc::sock_filter {
            code: (BPF_JMP | BPF_JEQ | BPF_K) as u16,
            jt: 2,
            jf: 0,
            k: libc::SYS_setpgid as u32,
        },
        libc::sock_filter {
            code: (BPF_JMP | BPF_JEQ | BPF_K) as u16,
            jt: 1,
            jf: 0,
            k: libc::SYS_setsid as u32,
        },
        libc::sock_filter {
            code: (BPF_RET | BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: 0x7FFF_0000,
        },
        libc::sock_filter {
            code: (BPF_RET | BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: 0x0005_0000 | libc::EPERM as u32,
        },
        libc::sock_filter {
            code: (BPF_RET | BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: 0x8000_0000,
        },
    ]
}

/// Installs the fixed-membership restriction: `PR_SET_NO_NEW_PRIVS` plus a
/// `seccomp` filter that rejects `setpgid(2)` and `setsid(2)` with
/// `EPERM`. The filter is inherited by the owned child and every
/// descendant across `fork`/`exec`; with `no_new_privs` set, a descendant
/// can only stack *more* restrictive filters, never remove this one, and
/// no privilege gain can bypass it. An install failure is a pre-ownership
/// setup failure: no owned process tree exists yet.
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )
))]
#[allow(unsafe_code)]
pub(crate) fn enforce_fixed_group_membership() -> Result<(), String> {
    // SAFETY: prctl with PR_SET_NO_NEW_PRIVS and a literal 1 is a single
    // scalar syscall with no pointer arguments; it is required to install a
    // seccomp filter without CAP_SYS_ADMIN, is inherited across fork/exec,
    // and can never be cleared by a descendant.
    let result = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if result != 0 {
        return Err(format!(
            "cannot set PR_SET_NO_NEW_PRIVS: {}",
            std::io::Error::last_os_error()
        ));
    }
    let program = membership_restriction_program();
    let fprog = libc::sock_fprog {
        len: u16::try_from(program.len()).expect("the membership filter fits u16"),
        filter: program.as_ptr().cast_mut(),
    };
    // SAFETY: seccomp(SECCOMP_SET_MODE_FILTER, 0, &fprog) copies the BPF
    // program into the kernel during the call — the pointer is not
    // retained. The stack-local program lives for the whole call.
    let result =
        unsafe { libc::syscall(libc::SYS_seccomp, libc::SECCOMP_SET_MODE_FILTER, 0, &fprog) };
    if result != 0 {
        return Err(format!(
            "cannot install the membership seccomp filter: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(all(
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )
)))]
pub(crate) fn enforce_fixed_group_membership() -> Result<(), String> {
    Err("supervised lifecycle requires Linux on x86_64, aarch64, or riscv64".to_owned())
}

/// Signals one owned process group. `ESRCH` is a terminal no-op; any other
/// failure is explicit.
pub(crate) fn signal_group(pgid: i32, signal: Signal) -> Result<(), String> {
    match killpg(Pid::from_raw(pgid), signal) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(format!("cannot signal the owned process group: {error}")),
    }
}

/// Incremental length-prefixed control-frame reader shared by the
/// supervisor units: `[u32 LE length][kind][payload]`.
#[derive(Debug, Default)]
pub(crate) struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    /// Creates an empty frame reader.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Feeds newly read bytes and returns the first complete frame, if one
    /// arrived. Partial frames stay buffered. Note that the returned frame
    /// is already consumed: callers must handle it and then drain the rest
    /// with repeated [`FrameReader::pop`] calls.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Option<(u8, Vec<u8>)> {
        self.buf.extend_from_slice(bytes);
        self.pop()
    }

    /// Parses the first complete frame out of the buffer. Call after
    /// [`FrameReader::push`] fed new bytes once; drain with repeated calls.
    pub(crate) fn pop(&mut self) -> Option<(u8, Vec<u8>)> {
        if self.buf.len() < 4 {
            return None;
        }
        let len = u32::from_le_bytes(self.buf[..4].try_into().expect("four bytes")) as usize;
        if self.buf.len() < 4 + len {
            return None;
        }
        let kind = self.buf[4];
        let payload = self.buf[5..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Some((kind, payload))
    }
}

/// Writes one length-prefixed control frame: `[u32 LE length][kind][payload]`.
pub(crate) fn write_frame(
    stream: &mut ControlStream,
    kind: u8,
    payload: &[u8],
) -> Result<(), String> {
    let mut frame = Vec::with_capacity(4 + 1 + payload.len());
    let frame_len = u32::try_from(1 + payload.len())
        .map_err(|_| "the control frame is too large".to_owned())?;
    frame.extend_from_slice(&frame_len.to_le_bytes());
    frame.push(kind);
    frame.extend_from_slice(payload);
    nix::unistd::write(stream, &frame)
        .map_err(|error| format!("cannot write the control frame: {error}"))?;
    Ok(())
}

/// The explicit outcome of catastrophic emergency containment.
///
/// `Ok(())` alone would be ambiguous (contained-and-terminal vs. no anchor
/// vs. normal path already completed), so the result distinguishes the
/// terminal proof from the unavailable-anchor state:
///
/// - [`EmergencyContainment::TerminalProven`]: the anchor was retained,
///   the fallback signal was issued while retained, the group-scoped wait
///   reached `ECHILD`, and the anchor was released — the owned unit group
///   is terminal.
/// - [`EmergencyContainment::AnchorUnavailable`]: the anchor is not a
///   waitable child of rustX without a prior authoritative terminal event.
///   This is **not** a terminal proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmergencyContainment {
    /// The anchor was retained, the fallback signal was issued while that
    /// identity was retained, the group-scoped wait reached `ECHILD`, and
    /// the anchor was then released.
    TerminalProven,
    /// The unit anchor is unavailable without a prior authoritative
    /// terminal proof. Never a terminal result.
    AnchorUnavailable,
}

/// Catastrophic fallback after the unit's outer supervisor has been reaped.
///
/// rustX is a subreaper, so the dead outer's unit descendants are now rustX
/// children. The inner leader is retained with `WNOWAIT` before its numeric
/// identity is used for `killpg`; this is the same ABA-proof anchor used by
/// the normal outer path. Only after the group-scoped child wait reaches
/// `ECHILD` is the anchor identity released and terminality proven.
///
/// The anchor is matched only by pid; the unit group only by its retained
/// pgid. No broad wait (`waitpid(-1)`, `waitid(P_ALL)`) exists here, so
/// unrelated adopted children are never consumed.
#[cfg(target_os = "linux")]
pub(crate) fn emergency_contain_group(
    pgid: i32,
    anchor_unavailable: bool,
) -> Result<EmergencyContainment, String> {
    use nix::errno::Errno;
    use nix::sys::signal::Signal;
    use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
    #[cfg(not(test))]
    let _ = anchor_unavailable;

    #[cfg(test)]
    if anchor_unavailable {
        // The regression seam: the anchor reads as not waitable without a
        // prior authoritative terminal event. The semantic state is what
        // matters — never an actual pid reuse.
        return Ok(EmergencyContainment::AnchorUnavailable);
    }
    let anchor = Pid::from_raw(pgid);
    loop {
        // Anchor retention: observe the adopted inner leader's terminal
        // state without consuming its identity (`WNOWAIT`). The numeric
        // group id stays provably allocated while this returns Exited or
        // Signaled.
        match waitid(
            Id::Pid(anchor),
            WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT,
        ) {
            Ok(WaitStatus::StillAlive) => std::thread::sleep(POLL_INTERVAL),
            Ok(WaitStatus::Exited(..) | WaitStatus::Signaled(..)) => break,
            Ok(_) | Err(Errno::EINTR) => {}
            Err(Errno::ECHILD) => {
                // The anchor is not a waitable child of rustX: the owned
                // group may still exist. ECHILD from the anchor wait is
                // never a terminal process-group proof, and the cached
                // numeric pgid is never signaled after anchor loss.
                return Ok(EmergencyContainment::AnchorUnavailable);
            }
            Err(error) => {
                return Err(format!("cannot retain the lost unit anchor: {error}"));
            }
        }
    }

    signal_group(pgid, Signal::SIGKILL)?;
    loop {
        // The group-scoped terminal proof: no adopted child of rustX
        // remains in the unit group. The anchor itself is released (reaped)
        // by this same wait, strictly after the fallback signal.
        match waitid(
            Id::PGid(anchor),
            WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED,
        ) {
            Ok(WaitStatus::StillAlive) => std::thread::sleep(POLL_INTERVAL),
            Ok(_) | Err(Errno::EINTR) => {}
            Err(Errno::ECHILD) => return Ok(EmergencyContainment::TerminalProven),
            Err(error) => {
                return Err(format!(
                    "cannot prove the lost unit group terminal: {error}"
                ));
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn emergency_contain_group(
    _pgid: i32,
    _anchor_unavailable: bool,
) -> Result<EmergencyContainment, String> {
    Err("fallback containment requires Linux PR_SET_CHILD_SUBREAPER".to_owned())
}

#[cfg(all(
    test,
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )
))]
mod seccomp_tests {
    use super::*;
    use nix::sys::wait::{WaitStatus, waitpid};
    use nix::unistd::{getpgrp, getsid};

    #[derive(Clone, Copy)]
    enum Call {
        Setsid,
        Setpgid,
    }

    #[allow(unsafe_code)]
    fn assert_filtered(call: Call, x32: bool) {
        // SAFETY: the throwaway child installs the real production filter,
        // performs one scalar syscall, and exits without returning through
        // the multi-threaded test process.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed: {}", std::io::Error::last_os_error());
        if pid == 0 {
            let before_pgrp = getpgrp();
            let before_sid = getsid(None).ok();
            let installed = enforce_fixed_group_membership().is_ok();
            let native = match call {
                Call::Setsid => libc::SYS_setsid,
                Call::Setpgid => libc::SYS_setpgid,
            };
            let number = if x32 { native | 0x4000_0000 } else { native };
            // SAFETY: both tested syscalls take only scalar arguments.
            let result = unsafe {
                match call {
                    Call::Setsid => libc::syscall(number),
                    Call::Setpgid => libc::syscall(number, 0, 0),
                }
            };
            let denied =
                result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
            let unchanged = getpgrp() == before_pgrp && getsid(None).ok() == before_sid;
            // SAFETY: terminate the isolated child immediately; the status
            // encodes installation, deterministic EPERM, and no mutation.
            unsafe { libc::_exit(i32::from(!(installed && denied && unchanged))) };
        }
        let status = waitpid(Pid::from_raw(pid), None).expect("reap seccomp test child");
        assert_eq!(status, WaitStatus::Exited(Pid::from_raw(pid), 0));
    }

    #[test]
    fn native_setsid_is_denied_by_the_installed_filter() {
        assert_filtered(Call::Setsid, false);
    }

    #[test]
    fn native_setpgid_is_denied_by_the_installed_filter() {
        assert_filtered(Call::Setpgid, false);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn x32_setsid_is_denied_by_the_filter_with_eperm() {
        assert_filtered(Call::Setsid, true);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn x32_setpgid_is_denied_by_the_filter_with_eperm() {
        assert_filtered(Call::Setpgid, true);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn x86_64_program_has_fail_closed_x32_and_membership_branches() {
        let program = membership_restriction_program();
        assert_eq!(program.len(), 9);
        assert_eq!(program[0].k, 4);
        assert_eq!(program[1].k, AUDIT_ARCH);
        assert_eq!(program[2].k, 0);
        assert_eq!(program[3].k, 0x4000_0000);
        assert_eq!(program[4].k, u32::try_from(libc::SYS_setpgid).unwrap());
        assert_eq!(program[5].k, u32::try_from(libc::SYS_setsid).unwrap());
        assert_eq!(program[6].k, 0x7FFF_0000);
        assert_eq!(program[7].k, 0x0005_0000 | libc::EPERM as u32);
        assert_eq!(program[8].k, 0x8000_0000);
    }
}
