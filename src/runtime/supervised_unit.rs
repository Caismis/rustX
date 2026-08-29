//! The shared structural ownership core of rustX-owned supervised process
//! units.
//!
//! Every real production subprocess hierarchy — the M5 short-lived Bash
//! supervisor unit and the M7 long-lived interactive MCP stdio supervisor
//! unit — is composed from exactly this core, so both domains share the
//! same physical ownership guarantees:
//!
//! - one kernel-mediated terminal proof: the group-scoped wait
//!   (`waitid(Id::PGid)` returning `ECHILD`), with the platform-specific
//!   wait adapter in [`crate::runtime::process_wait`];
//! - on Linux, an inherited seccomp filter rejects `setsid(2)`/`setpgid(2)`
//!   with `EPERM`, so no owned descendant can escape the unit's
//!   process-group/session; macOS has the process-group lifecycle but no
//!   equivalent seccomp restriction, so the unit owns only the processes
//!   that remain in its process group and a descendant that deliberately
//!   leaves that group is outside the ownership domain (not tracked,
//!   contained, reaped, or waited for);
//! - on Linux, the runtime child-subreaper prerequisite is installed before
//!   the unit spawns (`crate::runtime::process_supervision`) and re-established
//!   inside each supervisor process so orphaned descendants reparent into
//!   the unit's reaping domain; macOS has no equivalent orphan-adoption
//!   primitive;
//! - the single-reaper anchor discipline: the inner supervisor pid is the
//!   structural ownership anchor with exactly one reaping owner, and
//!   fallback containment signals are issued only while the anchor is
//!   retained;
//! - `TERM` -> grace -> `KILL` against the inner leader's own process
//!   group, whose numeric id is the inner's pid — provably allocated while
//!   signaling is legal;
//! - Linux catastrophic fallback containment (adopted-anchor retention with
//!   `WNOWAIT`, one anchored `SIGKILL`, group-scoped `ECHILD` release) when
//!   the inner supervisor or control chain fails; macOS reports an
//!   unavailable anchor as unproven instead of claiming that proof;
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
/// Owner -> outer: the owner accepted and retained the outer control
/// connection. This is the runtime->outer startup gate: before it arrives
/// the outer owns nothing and may not create any part of the unit
/// hierarchy, so a control-setup failure at the owner can only ever find a
/// gated outer with no inner and no server tree.
pub(crate) const MSG_OWNER_ATTACHED: u8 = 0x13;

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

/// The bounded macOS window in which the group-absence probe must reach
/// `ESRCH` after the fallback containment signal.
#[cfg(target_os = "macos")]
pub(crate) const GROUP_ABSENCE_TIMEOUT: Duration = Duration::from_secs(2);

/// The deadline after which a missing terminal-ack frame is ignored (the
/// owner may have disappeared; terminality was already proven).
pub(crate) const TERMINAL_ACK_TIMEOUT: Duration = Duration::from_secs(2);

/// Enables the supervisor's orphan-reaping capability where the platform
/// provides one. Linux uses `PR_SET_CHILD_SUBREAPER`; macOS has no equivalent
/// process-wide primitive, so its normal lifecycle relies on the direct shell
/// parent waiting for its background jobs and on process-group signaling.
#[allow(unsafe_code)]
#[allow(clippy::unnecessary_wraps)]
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
    #[cfg(target_os = "macos")]
    {
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err("the supervisor unit requires Linux or macOS process supervision".to_owned())
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

/// Installs the fixed-membership restriction on Linux: `PR_SET_NO_NEW_PRIVS`
/// plus a `seccomp` filter that rejects `setpgid(2)` and `setsid(2)` with
/// `EPERM`. The filter is inherited by the owned child and every descendant
/// across `fork`/`exec`; with `no_new_privs` set, a descendant can only stack
/// more restrictive filters, never remove this one. The macOS implementation
/// is an explicit successful no-op because macOS has no equivalent primitive;
/// its normal process-group path remains usable but does not claim immutable
/// membership.
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
#[cfg(target_os = "macos")]
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn enforce_fixed_group_membership() -> Result<(), String> {
    // macOS has no seccomp equivalent in the current dependency/runtime
    // boundary. The supervisor still creates a dedicated session/process
    // group and uses group-scoped waits, while the shell command wrapper keeps
    // ordinary background jobs attached to the shell's wait lifecycle.
    Ok(())
}

#[cfg(not(any(
    all(
        target_os = "linux",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "riscv64"
        )
    ),
    target_os = "macos"
)))]
pub(crate) fn enforce_fixed_group_membership() -> Result<(), String> {
    Err("supervised lifecycle requires Linux or macOS process supervision".to_owned())
}

/// Signals one owned process group.
///
/// `ESRCH` is the one terminal no-op: it means no process in the target
/// group exists, so there is nothing to signal. Every other failure —
/// including `EPERM`, which means the signal operation was not authorized
/// for at least some target processes — is an explicit containment failure
/// and must never be converted into a success or a terminal result.
pub(crate) fn signal_group(pgid: i32, signal: Signal) -> Result<(), String> {
    classify_signal_result(killpg(Pid::from_raw(pgid), signal))
}

/// Maps one raw group-signal result to the shared containment contract.
///
/// Separated from [`signal_group`] so the mapping — and in particular the
/// rule that `EPERM` is never a success — is deterministically testable
/// without constructing an OS `killpg` result.
fn classify_signal_result(result: nix::Result<()>) -> Result<(), String> {
    match result {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(format!("cannot signal the owned process group: {error}")),
    }
}

/// The outcome of the one fallback containment signal (`SIGKILL` to the
/// retained owned group).
///
/// A group-signal `EPERM` proves only that the requested signal operation
/// was not authorized according to the platform's signal permission rules.
/// It does **not** prove group absence, containment, or zombie-only
/// membership, so it is never modelled as `Contained` and never as a
/// terminal result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContainmentOutcome {
    /// At least one live group member was signaled, or the group is already
    /// gone (`ESRCH`).
    Contained,
    /// The signal operation did not establish containment: `EPERM` (not
    /// authorized, or — on macOS — an ambiguity such as a zombie-only group
    /// that the kernel also reports as `EPERM`) and every other error.
    /// Never a success and never a terminal result; the caller must prove
    /// the group's absence independently before terminality.
    Unproven(String),
}

/// Issues the one fallback containment `SIGKILL` and classifies the result.
///
/// `Ok` (the signal was issued) and `ESRCH` (the group was already absent at
/// the signal operation) are the only [`ContainmentOutcome::Contained`]
/// results. `EPERM` and every other error are
/// [`ContainmentOutcome::Unproven`]: the signal operation did not establish
/// containment, and the caller must never convert that into a terminal
/// result.
pub(crate) fn contain_group(pgid: i32) -> ContainmentOutcome {
    classify_containment_result(killpg(Pid::from_raw(pgid), Signal::SIGKILL))
}

/// Maps one raw containment-signal result to [`ContainmentOutcome`].
///
/// Separated from [`contain_group`] so the `EPERM`-is-unproven
/// classification is deterministically testable without constructing an OS
/// `killpg` result.
fn classify_containment_result(result: nix::Result<()>) -> ContainmentOutcome {
    match result {
        Ok(()) | Err(Errno::ESRCH) => ContainmentOutcome::Contained,
        Err(error) => {
            ContainmentOutcome::Unproven(format!("cannot contain the owned process group: {error}"))
        }
    }
}

/// macOS: proves the owned process group is absent by probing `killpg(pgid, 0)`
/// until it reaches `ESRCH`.
///
/// `waitid(Id::PGid) == ECHILD` on macOS only proves the waiting supervisor
/// has no waitable group child left; a descendant reparented to launchd is
/// invisible to that wait. The whole group's absence is instead proven by a
/// `killpg(pgid, 0)` probe reaching `ESRCH`, which reflects every process in
/// the numeric group rather than only the caller's children.
///
/// `ESRCH` is the sole accepted absence proof: it means no process anywhere
/// has that numeric process-group id. `Ok(())` (a live signalable member
/// exists) and `EPERM` (the group is still observable but this caller cannot
/// signal any member — a zombie-only group, or a live member the caller is
/// not authorized to signal; the kernel reports both as `EPERM`) both keep
/// the probe polling and are never a terminal result by themselves. A hard
/// error or a timeout leaves the group's absence unproven.
///
/// The caller must run this only after the retained anchor was reaped: an
/// un-reaped anchor zombie keeps the group observable and the probe would
/// never reach `ESRCH`.
///
/// # Numeric-identity (ABA) note
///
/// The probe runs strictly after the anchor is released, so the numeric
/// group id may in principle be recycled by an unrelated new process group.
/// That cannot make the probe unsound — `ESRCH` still means "no process has
/// this pgid", which implies every process that remained in the owned group
/// is gone. It can only make the probe conservative: a coincidental reuse
/// keeps the group observable and turns an actually-empty owned group into
/// an `Err` (unproven timeout), never into a false terminal result.
#[cfg(target_os = "macos")]
pub(crate) fn prove_group_absent(pgid: i32) -> Result<(), String> {
    let deadline = std::time::Instant::now() + GROUP_ABSENCE_TIMEOUT;
    loop {
        match killpg(Pid::from_raw(pgid), None) {
            Err(Errno::ESRCH) => return Ok(()),
            // `Ok(())` means a live signalable member remains; `EPERM` means
            // the group is still observable but this caller cannot signal any
            // member (a zombie-only group, or a live member it is not
            // authorized to signal). Both keep polling until `ESRCH` or the
            // bound, so neither is ever a terminal result by itself.
            Ok(()) | Err(Errno::EPERM) => {}
            Err(error) => {
                return Err(format!("cannot probe the owned group absence: {error}"));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(
                "the owned process group did not become provably absent after containment"
                    .to_owned(),
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Incremental length-prefixed control-frame reader shared by the
/// supervisor units: `[u32 LE length][kind][payload]`.
///
/// One stream direction has exactly one logical buffered reader, held across
/// every lifecycle phase of that connection. Unix stream reads do not
/// preserve the writer's frame boundaries, so a phase that recognizes its
/// own frame and then drops a gate-local reader would discard valid frames
/// that arrived in the same read. Feeding ([`FrameReader::feed`]) and
/// draining ([`FrameReader::pop`]) are deliberately separate so a frame can
/// never be lost through an ignored return value.
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

    /// Feeds newly read bytes **without** consuming any frame: feeding and
    /// draining are separate steps, so no frame can ever be dropped by
    /// ignoring a return value.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The bytes this reader still owns: the surplus that a lifecycle phase
    /// transition must never discard. Test-only observation of the
    /// ownership invariant itself; the supervisors only ever feed and pop.
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn buffered(&self) -> &[u8] {
        &self.buf
    }

    /// Parses the first complete frame out of the buffer. Call after
    /// [`FrameReader::feed`] fed new bytes; drain with repeated calls.
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
/// On Linux, rustX is a subreaper, so the dead outer's unit descendants are
/// now rustX children. The inner leader is retained with `WNOWAIT` before its
/// numeric identity is used for `killpg`; this is the same ABA-proof anchor
/// used by the normal outer path. Only after the group-scoped child wait
/// reaches `ECHILD` is the anchor identity released and terminality proven.
/// On macOS the inner is not adopted by rustX after outer loss, so this
/// function returns [`EmergencyContainment::AnchorUnavailable`] when that
/// anchor is not waitable.
///
/// The anchor is matched only by pid; the unit group only by its retained
/// pgid. No broad wait (`waitpid(-1)`, `waitid(P_ALL)`) exists here, so
/// unrelated adopted children are never consumed.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn emergency_contain_group(
    pgid: i32,
    anchor_unavailable: bool,
) -> Result<EmergencyContainment, String> {
    use crate::runtime::process_wait::{Id, waitid};
    use nix::errno::Errno;
    use nix::sys::signal::Signal;
    use nix::sys::wait::{WaitPidFlag, WaitStatus};
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

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn emergency_contain_group(
    _pgid: i32,
    _anchor_unavailable: bool,
) -> Result<EmergencyContainment, String> {
    Err("fallback containment requires Linux or macOS process supervision".to_owned())
}

/// Containment of an **adopted orphaned** unit group whose owning process is
/// gone entirely (Issue #145).
///
/// This is the sibling of [`emergency_contain_group`], and the difference is
/// exactly the difference between the two catastrophes:
///
/// ```text
/// emergency_contain_group   the unit's OUTER supervisor was lost, but the
///                           inner supervisor is still the unit's own reaping
///                           owner, so containment first waits for the inner
///                           to reach its own terminal state and only then
///                           issues the anchored fallback signal.
///
/// contain_adopted_group     the whole owning rustX process is gone. Nothing
///                           will ever drive that unit to its terminal state,
///                           so waiting for the inner would wait forever: the
///                           anchor is retained for identity, the group is
///                           killed, and terminality is proven group-scoped.
/// ```
///
/// The pid-reuse guarantee is identical and is what makes signalling a cached
/// numeric pgid legal: `waitid(Pid, WNOWAIT)` must first answer for the
/// anchor. `StillAlive` proves the identity is allocated and adopted by this
/// process; `Exited`/`Signaled` under `WNOWAIT` proves the same because the
/// zombie is deliberately not consumed. `ECHILD` proves the opposite — the
/// anchor is not adoptable here (macOS, or a Linux parent that was not a
/// subreaper when the owner died) — and the cached pgid is then **never**
/// signalled and no terminality is claimed.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn contain_adopted_group(pgid: i32) -> Result<EmergencyContainment, String> {
    use crate::runtime::process_wait::{Id, waitid};
    use nix::errno::Errno;
    use nix::sys::signal::Signal;
    use nix::sys::wait::{WaitPidFlag, WaitStatus};

    let anchor = Pid::from_raw(pgid);
    loop {
        match waitid(
            Id::Pid(anchor),
            WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT,
        ) {
            // Both outcomes prove the numeric identity is still allocated
            // and owned here, which is exactly the precondition for
            // signalling the cached group id.
            Ok(WaitStatus::StillAlive | WaitStatus::Exited(..) | WaitStatus::Signaled(..)) => break,
            Ok(_) | Err(Errno::EINTR) => {}
            Err(Errno::ECHILD) => return Ok(EmergencyContainment::AnchorUnavailable),
            Err(error) => {
                return Err(format!(
                    "cannot retain the adopted nested unit anchor: {error}"
                ));
            }
        }
    }

    signal_group(pgid, Signal::SIGKILL)?;
    loop {
        // The group-scoped terminal proof: no adopted member of the unit
        // group remains. This wait also reaps the anchor itself, strictly
        // after the containment signal.
        match waitid(
            Id::PGid(anchor),
            WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED,
        ) {
            Ok(WaitStatus::StillAlive) => std::thread::sleep(POLL_INTERVAL),
            Ok(_) | Err(Errno::EINTR) => {}
            Err(Errno::ECHILD) => return Ok(EmergencyContainment::TerminalProven),
            Err(error) => {
                return Err(format!(
                    "cannot prove the adopted nested unit group terminal: {error}"
                ));
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn contain_adopted_group(_pgid: i32) -> Result<EmergencyContainment, String> {
    Err("nested unit containment requires Linux or macOS process supervision".to_owned())
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

#[cfg(test)]
mod signal_contract_tests {
    use super::{ContainmentOutcome, classify_containment_result, classify_signal_result};
    use nix::errno::Errno;

    /// A successful signal and an `ESRCH` (no target group) are the only
    /// results that count as successful containment.
    #[test]
    fn success_and_esrch_are_terminal_no_ops() {
        assert_eq!(classify_signal_result(Ok(())), Ok(()));
        assert_eq!(classify_signal_result(Err(Errno::ESRCH)), Ok(()));
    }

    /// `EPERM` means the signal was not authorized for at least some target
    /// processes. It is an explicit containment failure — never a success
    /// and never a terminal result.
    #[test]
    fn eperm_is_never_a_success() {
        let result = classify_signal_result(Err(Errno::EPERM));
        assert!(result.is_err(), "EPERM must not map to success: {result:?}");
        let message = result.expect_err("EPERM is an error");
        assert!(
            message.contains("EPERM"),
            "the failure must name the unauthorized signal: {message}"
        );
    }

    /// Any other signal error is likewise an explicit containment failure.
    #[test]
    fn other_errors_are_explicit_failures() {
        assert!(classify_signal_result(Err(Errno::EINVAL)).is_err());
        assert!(classify_signal_result(Err(Errno::EACCES)).is_err());
    }

    /// A successful containment signal and `ESRCH` (group already gone) are
    /// the two terminal containment outcomes.
    #[test]
    fn containment_success_and_esrch_are_contained() {
        assert_eq!(
            classify_containment_result(Ok(())),
            ContainmentOutcome::Contained
        );
        assert_eq!(
            classify_containment_result(Err(Errno::ESRCH)),
            ContainmentOutcome::Contained
        );
    }

    /// `EPERM` from the fallback containment signal is an explicit unproven
    /// state on every platform: it proves only that the signal operation was
    /// not authorized (on macOS the kernel also reports a zombie-only group
    /// as `EPERM`, so the two cases are indistinguishable). It must never map
    /// to `Contained` and never be a terminal result.
    #[test]
    fn containment_eperm_is_unproven_on_every_platform() {
        let outcome = classify_containment_result(Err(Errno::EPERM));
        match outcome {
            ContainmentOutcome::Unproven(message) => {
                assert!(
                    message.contains("EPERM"),
                    "the unproven state must name the unauthorized signal: {message}"
                );
            }
            other @ ContainmentOutcome::Contained => {
                panic!("EPERM must never be {other:?}")
            }
        }
    }

    /// Any other containment-signal error is an explicit unproven state.
    #[test]
    fn containment_other_errors_are_unproven() {
        assert!(matches!(
            classify_containment_result(Err(Errno::EINVAL)),
            ContainmentOutcome::Unproven(_)
        ));
    }
}
