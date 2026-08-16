//! The per-invocation Bash process supervisor for Linux and macOS.
//!
//! Each Bash invocation owns one small supervisor composed of two
//! processes:
//!
//! ```text
//! rustX
//!   └─ outer supervisor (rustX child; subreaper; reaper of last resort)
//!        └─ inner supervisor (outer child; setsid → invocation session/group leader;
//!                              subreaper; /bin/bash parent; orphan reaper; IPC peer)
//!             └─ /bin/bash -c <command>
//!                  └─ descendants
//! ```
//!
//! The supervisor exists only for the lifetime of one Bash invocation. It
//! is a plain `std` process (no tokio runtime) and owns nothing but process
//! lifecycle: it never touches agent-loop state, tool-registry state,
//! artifacts, provider translation, or model history.
//!
//! # Why two processes
//!
//! The invocation must be **terminable** (`TERM`/`KILL` reach bash and its
//! descendants) and **reapable** (someone must wait for every owned child
//! and reach a kernel-mediated terminal state). The two requirements
//! conflict inside one process: a process inside the killable group dies
//! with it, and a process outside the group cannot be the group's anchor.
//! The outer supervisor therefore survives the group signals and acts as
//! the reaper of last resort, while the inner supervisor anchors the
//! invocation's session/process group, reaps the shell and its orphans
//! during the normal lifecycle, and performs the `TERM` -> grace -> `KILL`
//! sequence on its own group.
//!
//! # Session, group, and ownership
//!
//! The inner supervisor calls `setsid()`: it becomes the leader of a fresh
//! session and of the session's first process group, whose numeric id is
//! the inner supervisor's own pid. `/bin/bash` is spawned without a new
//! process group, so bash and every descendant that stays in the group
//! live in exactly that one invocation-owned group inside the
//! invocation-owned session. Unrelated rustX/sibling processes live in
//! different sessions, so they can never join the invocation's process
//! group (`setpgid` across sessions fails with `EPERM`).
//!
//! # Ownership boundary
//!
//! The Bash invocation's ownership boundary is its dedicated process
//! group. The invocation owns, guarantees termination of, and bases its
//! settlement on exactly the processes that remain in that group. Ordinary
//! shell descendants remain owned while they stay in the group.
//!
//! # Fixed invocation process group
//!
//! **Membership is immutable for Bash descendants.** The inner supervisor
//! installs a narrow inherited seccomp policy (see
//! `enforce_fixed_group_membership` between its own `setsid()` setup and
//! the `/bin/bash` spawn: `setsid(2)` and `setpgid(2)` are rejected with
//! `EPERM` for the inner supervisor, for bash, and for every descendant —
//! `setpgid(2)` and `setsid(2)` are the only syscalls that can change
//! process-group/session membership on Linux, and seccomp filters are
//! inherited across `fork`/`exec` and can only be made more restrictive
//! afterwards. A Bash command therefore cannot create a new session or move
//! itself (or, via a parent-on-child `setpgid`, another process) out of the
//! invocation group. A descendant that tries (for example `setsid sleep
//! 30`, or `setpgid` from any child) fails deterministically with
//! `EPERM`; the utility usually exits non-zero and nothing leaves the
//! invocation group.
//!
//! The syscall numbers come directly from `libc` for the compiled Linux
//! target ABI. On x86-64, x32 syscall execution is rejected explicitly
//! because it shares `AUDIT_ARCH_X86_64` while using a distinct syscall-
//! number namespace. The restriction is installed with
//! `PR_SET_NO_NEW_PRIVS` plus a `seccomp` filter and requires no privileges.
//! A failure to install it is
//! a pre-ownership setup failure: no bash tree is spawned and the
//! invocation settles as an explicit `Failed`.
//!
//! # Reaping domain vs settlement gate
//!
//! Both supervisor processes call `PR_SET_CHILD_SUBREAPER`, so a shell
//! descendant that outlives the shell is reparented into the inner
//! supervisor's child domain (nearest subreaper ancestor), and the outer
//! supervisor inherits the inner's children if the inner dies with them.
//! The reaping domain (every adopted child) is deliberately **wider** than
//! the semantic ownership domain (the invocation group). Settlement uses
//! the kernel's **group-scoped wait** — `waitid` with `Id::PGid` — which
//! matches only children of the waiting process whose process group is the
//! invocation group:
//!
//! - the inner supervisor's group-scoped wait returning `ECHILD` — every
//!   child of the inner that is in the invocation group is reaped. On Linux
//!   that is the complete terminal proof and the inner exits with
//!   `INNER_EXIT_NORMAL`; on macOS it only proves the inner has no waitable
//!   group child left (reparented descendants are invisible), so the inner
//!   exits with `INNER_EXIT_CONTAINMENT` and the outer's fallback
//!   containment signal terminates the retained group;
//! - the outer supervisor's group-scoped wait returning `ECHILD` — no
//!   child of the outer remains in the invocation group (the inner anchor
//!   itself is a member and is released by this same wait, strictly after
//!   any fallback containment signal); it then reports
//!   `MSG_ALL_CHILDREN_REAPED` and exits. On macOS this frame means "the
//!   anchor was released after the group was actively signaled", not "the
//!   group is proven empty".
//!
//! # Single-reaper anchor ownership
//!
//! The inner supervisor pid is the invocation's **structural ownership
//! anchor**: while it remains un-reaped, the numeric invocation group id is
//! provably allocated and fallback containment signals are legal. An
//! anchor therefore has exactly one reaping owner, and generic reaping
//! hygiene never consumes it:
//!
//! - the outer supervisor's dedicated anchor observation is the only code
//!   allowed to **observe** the anchor's terminal state (`waitid` with
//!   `WNOWAIT`: observation only, never consumption);
//! - the outer supervisor's group-scoped gate is the only code allowed to
//!   **reap** the anchor (strictly after any fallback containment signal);
//! - rustX becomes the anchor's reaping owner only when both supervisors
//!   are lost: the adopted anchor is observed with `WNOWAIT` and reaped
//!   through rustX's own group-scoped gate (see the Bash tool);
//! - the outer supervisor therefore has **no generic `waitpid(-1)` loop**:
//!   every child of the outer is either the anchor or an in-group adopted
//!   descendant, so the group-scoped gate reaps the outer's whole child
//!   domain and `waitpid(-1)` could only ever consume the anchor. There is
//!   nothing left for a generic loop to own.
//!
//! An `ECHILD` from the dedicated anchor observation is consequently an
//! **ownership invariant violation**, never a terminal observation: before
//! the intentional release no other code may reap the anchor, so
//! `ECHILD` means the single-owner rule was broken. The outer reports the
//! violation and fails safely — it never derives owned-group terminality
//! from an anchor `ECHILD`, never signals a numeric group id without the
//! retained anchor, and never reports the canonical terminal event.
//!
//! # Wait ownership audit
//!
//! Every wait call site of the supervisor unit and its exact ownership:
//!
//! | Call site | Matches | Observes/consumes | Owner |
//! |---|---|---|---|
//! | outer dedicated anchor wait (`waitid(Pid(inner), WNOWAIT \| WEXITED \| WNOHANG)`) | only the inner anchor | observes only (`WNOWAIT`) | outer dedicated path; `ECHILD` = invariant violation, never terminal |
//! | outer frozen-anchor wait (`waitid(Pid(inner), WUNTRACED \| WNOHANG)`) | only the inner anchor | observes/consumes the stop event | outer dedicated path |
//! | outer group gate (`waitid(PGid(inner), WEXITED \| WNOHANG)`) | every outer child in the invocation group, including the anchor | consumes | outer gate; `ECHILD` = canonical terminal event (the anchor's only reaper release); on macOS this is only a terminal event because the fallback containment signal was already issued while the anchor was retained |
//! | inner reaping hygiene (`waitpid(-1, WNOHANG)`) | every child of the inner (bash and adopted in-group descendants) | consumes | inner supervisor; no child of the inner is ever an anchor, so this never consumes another owner's identity |
//! | inner group gate (`waitid(PGid(self), WEXITED \| WNOHANG)`) | every inner child in the invocation group | consumes | inner supervisor; `ECHILD` = `INNER_EXIT_NORMAL` on Linux, `INNER_EXIT_CONTAINMENT` on macOS |
//!
//! rustX's own waits (the outer's direct reap and the catastrophic
//! adoption path) are documented in the Bash tool.
//!
//! The fixed-membership invariant is what makes `ECHILD` from these
//! group-scoped waits a **complete** terminal proof. `waitid(P_PGID)`
//! alone is only an observation of the waiting process's children: an
//! in-group grandchild hidden behind an ancestor that left the group would
//! be invisible to it. Because no bash descendant can leave the invocation
//! group, every in-group process other than the inner supervisor is a bash
//! descendant; while bash lives, bash itself is a matching child of the
//! inner and blocks the gate, and once bash (or any in-group ancestor)
//! exits, the kernel reparents its in-group children directly into the
//! nearest subreaper's child domain — the inner supervisor while it lives,
//! the outer supervisor after it. There is therefore no state in which an
//! in-group process is not a matching child of the supervisor that owns
//! the gate, and `ECHILD` is exactly the whole-group terminal state. This
//! is the exact kernel-mediated terminal linearization point of the owned
//! process group. It is not a `/proc` scan and not a `killpg(0)` probe (an
//! un-reaped leader zombie keeps the group observable, so probes cannot
//! distinguish live members).
//!
//! # Signal ownership
//!
//! `TERM`/`KILL` are issued by the inner supervisor with `killpg` against
//! **its own process group**, whose numeric id is its own pid — provably
//! allocated exactly while the inner supervisor lives, so no foreign
//! process group can ever receive the invocation's numeric group id while
//! signals remain legal. The inner ignores `SIGTERM` (it must survive the
//! group `TERM` to keep reaping); `SIGKILL` is uncatchable and kills the
//! inner together with the group. The final signal is the last `killpg`;
//! afterwards the anchor is released by the reap and no further signal
//! exists.
//!
//! # Failure containment
//!
//! When the inner supervisor terminates abnormally while owned work may
//! still be alive (signal failure, wait/reap failure, IPC failure, control-
//! channel abandonment), it does **not** simply walk away: it exits with
//! the dedicated `INNER_EXIT_CONTAINMENT` status. The outer supervisor —
//! the final containment and reaping authority — observes that status
//! without releasing the inner's identity (`waitid` with `WNOWAIT` keeps
//! the inner an un-reaped zombie), so the inner pid — the invocation's
//! process-group id — stays provably allocated. The outer then sends the
//! one fallback containment `SIGKILL` to that structurally owned group,
//! and only then releases the anchor through the group-scoped wait. The
//! numeric group id can therefore never be signaled after its allocation
//! has ended, and never without structural ownership proof.
//!
//! The outer also detects a **frozen anchor**: the inner supervisor is
//! never legitimately stopped, so an observed `SIGSTOP` state (an external
//! freeze of the whole unit) is un-wedged by the outer with `SIGKILL`,
//! which keeps a stopped containment chain from stranding the owned group.
//! The only residual state in which terminality cannot be proven from
//! rustX is a supervisor unit frozen at the kernel level beyond the outer's
//! reach; the confirmation watchdog then records explicit failure intent,
//! but canonical settlement still waits for authoritative terminality.
//!
//! # IPC
//!
//! rustX creates one `UnixStream` pair. The child end becomes the outer
//! supervisor's stdin (fd 0) and is inherited by the inner supervisor,
//! which is the only supervisor process that reads it (the outer never
//! reads, so the inner's lifecycle events can never be consumed by it).
//! bash gets a null stdin so it never sees the control channel. Messages
//! are length-prefixed frames: `[u32 LE length][u8 kind][payload]`. The
//! inner supervisor writes the pre-spawn `MSG_ANCHOR_READY` gate,
//! `MSG_OWNERSHIP_ESTABLISHED`, `MSG_SHELL_EXITED`,
//! `MSG_PROCESS_CONTROL_FAILURE`, and `MSG_SIGNAL_ATTEMPT` (test
//! observability); the outer supervisor
//! writes `MSG_ALL_CHILDREN_REAPED` — the single authoritative terminal
//! report — plus `MSG_PROCESS_CONTROL_FAILURE` and `MSG_SIGNAL_ATTEMPT`
//! for its fallback containment; rustX writes `MSG_START`,
//! `MSG_TERMINATE`, and the terminal acknowledgement. A successful Bash
//! spawn is the OS ownership commit. After rustX sends `START`, loss of the
//! commit frame is conservatively treated as possible ownership. There is
//! no generic IPC framework.
//!
//! # Platform behavior
//!
//! Linux uses `PR_SET_CHILD_SUBREAPER` plus the fixed-membership seccomp
//! filter, so group-scoped `ECHILD` is a complete descendant proof even when
//! Bash backgrounds work. macOS has neither primitive: a descendant that
//! outlives the shell is reparented to launchd and becomes invisible to the
//! supervisor's group-scoped wait, so `ECHILD` on macOS never proves the
//! group empty. macOS therefore uses a weaker, honest contract:
//!
//! - Bash is wrapped with an EXIT `wait` as a **best-effort convenience**
//!   so ordinary background jobs finish naturally; it is not an ownership
//!   boundary and the user command may legally replace it;
//! - when the shell is reaped, the inner supervisor escalates to the outer
//!   supervisor's fallback containment (`SIGKILL` to the retained group)
//!   instead of claiming group terminality from `ECHILD`;
//! - the outer reports terminality only after it issued that containment
//!   signal while the anchor was retained and then released the anchor.
//!   This proves the group was actively terminated, not that every member
//!   was reaped — the claim rustX cannot make on macOS;
//! - a containment signal whose result is `EPERM` is never itself a
//!   terminal result. `EPERM` proves only that the signal operation was not
//!   authorized (the kernel also reports a zombie-only group as `EPERM`, so
//!   the two cases are indistinguishable); on Linux it is an explicit
//!   containment failure, while on macOS the `killpg(pgid, 0)` absence probe
//!   after the anchor release — never `EPERM` — is the terminal authority;
//! - a macOS supervisor-loss path that cannot retain a waitable anchor is
//!   reported as unproven rather than converted into terminality.
//!
//! A macOS command that deliberately leaves the group (`setsid`/`setpgid`)
//! before containment is outside rustX's macOS proof; that limitation is
//! documented and is never claimed as contained.

use std::process::{Command, Stdio};

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, read, write};

use crate::runtime::process_wait::{Id, waitid};
#[cfg(target_os = "macos")]
use crate::runtime::supervised_unit::prove_group_absent;
use crate::runtime::supervised_unit::{
    ContainmentOutcome, FrameReader, INNER_EXIT_CONTAINMENT, INNER_EXIT_NORMAL,
    MSG_ALL_CHILDREN_REAPED, MSG_ANCHOR_READY, MSG_NO_OWNERSHIP, MSG_OWNERSHIP_ESTABLISHED,
    MSG_PROCESS_CONTROL_FAILURE, MSG_SHELL_EXITED, MSG_SIGNAL_ATTEMPT, MSG_START, MSG_TERMINAL_ACK,
    MSG_TERMINATE, POLL_INTERVAL, TERM_GRACE, TERMINAL_ACK_TIMEOUT, become_child_subreaper,
    contain_group, enforce_fixed_group_membership, ignore_group_term,
};

/// The outer supervisor role name in `RUSTX_SUPERVISOR_ROLE`.
pub const ROLE_OUTER: &str = crate::runtime::supervised_unit::ROLE_OUTER;

/// The inner supervisor role name in `RUSTX_SUPERVISOR_ROLE`.
pub const ROLE_INNER: &str = crate::runtime::supervised_unit::ROLE_INNER;

/// The environment variable carrying the `/bin/bash -c` command into the
/// supervisor (the command travels in argv today; the environment simply
/// carries it to the inner process that actually spawns bash).
pub const COMMAND_ENV: &str = "RUSTX_SUPERVISOR_COMMAND";

/// The optional test-observability environment variable naming a file the
/// inner supervisor writes its own pid into (the invocation's process-group
/// id). Never set in production.
pub const ANCHOR_PID_FILE_ENV: &str = "RUSTX_SUPERVISOR_ANCHOR_PID_FILE";

/// Test-only injection: the inner supervisor refuses every group signal.
pub const FAIL_SIGNAL_ENV: &str = "RUSTX_TEST_FAIL_SIGNAL";

/// Test-only injection: the inner supervisor fails the shell wait/reap.
pub const FAIL_WAIT_ENV: &str = "RUSTX_TEST_FAIL_WAIT";

/// Test-only injection: the inner supervisor fails the bash spawn.
pub const FAIL_BASH_SPAWN_ENV: &str = "RUSTX_TEST_FAIL_BASH_SPAWN";

/// Test-only injection: the inner supervisor's SIGTERM handler installation
/// fails (a pre-ownership setup failure).
pub const FAIL_SIGTERM_HANDLER_ENV: &str = "RUSTX_TEST_FAIL_SIGTERM_HANDLER";

/// Test-only injection: the invocation ownership anchor reads as lost, so
/// the inner supervisor refuses every group signal.
pub const FORCE_ANCHOR_LOSS_ENV: &str = "RUSTX_TEST_FORCE_ANCHOR_LOSS";

/// Test-only injection: names a directory where the outer supervisor parks
/// deterministically after its first dedicated `StillAlive` observation of
/// the inner anchor and before the next loop phase. The regressions use
/// the `observed`/`proceed` marker files to construct the
/// observed-then-exited anchor interleaving without sleeps. Never set in
/// production.
pub const OUTER_BARRIER_DIR_ENV: &str = "RUSTX_TEST_OUTER_BARRIER_DIR";

/// Test-only injection: the outer supervisor's fallback containment signal
/// fails with an injected permission error (the deterministic stand-in for
/// `killpg` returning `EPERM`). Never set in production.
pub const FAIL_CONTAINMENT_ENV: &str = "RUSTX_TEST_FAIL_CONTAINMENT";

// All protocol constants, exit codes, structural primitives, and the
// `TERM` -> grace -> `KILL` timings come from the shared supervisor-unit
// core (`crate::runtime::supervised_unit`), so the Bash and interactive
// units share one ownership model.

/// Runs the outer supervisor; never returns.
pub fn run_outer_supervisor() -> ! {
    let exit = run_outer();
    std::process::exit(exit);
}

/// Runs the inner supervisor; never returns.
pub fn run_inner_supervisor() -> ! {
    let exit = run_inner();
    std::process::exit(exit);
}

/// The outer supervisor's ownership state of the inner anchor.
///
/// The anchor (`inner_pid`) is the invocation's structural identity: while
/// it remains un-reaped, the numeric invocation process-group id is
/// provably allocated and fallback containment signals are legal. It has
/// exactly one reaping owner (the outer's dedicated anchor path), and the
/// transition into a terminal state happens only through the dedicated
/// observation:
///
/// ```text
/// Running
///     ↓ waitid(Pid(inner), WNOWAIT) terminal observation   (observe only)
/// TerminalRetained
///     ↓ waitid(PGid(inner)) group-scoped gate              (reap/release)
/// Released
/// ```
///
/// An `ECHILD` from the dedicated observation before the intentional
/// release is an ownership invariant violation — never a terminal
/// observation — and moves the anchor to [`InnerAnchor::UnexpectedlyLost`].
/// A fallback containment signal that fails moves the anchor to
/// [`InnerAnchor::ContainmentFailed`]: the anchor is still retained but the
/// owned group was not provably contained, so terminality stays unproven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InnerAnchor {
    /// The anchor is alive (or not yet observed terminal) and retained;
    /// only the dedicated observation path runs.
    Running,
    /// The anchor's terminal state was observed with `WNOWAIT` (identity
    /// not consumed) and any fallback containment signal was already
    /// issued while the identity was retained; the group-scoped gate now
    /// owns the release.
    TerminalRetained,
    /// The anchor is not waitable before its intentional release: an
    /// ownership invariant violation. The outer fails safely: it never
    /// gates on the numeric group id (the release could be an ABA hazard),
    /// never signals, and never reports the canonical terminal event.
    UnexpectedlyLost,
    /// The fallback containment signal failed while the anchor was still
    /// retained. The owned group was not provably contained, so the outer
    /// fails safely and never reports the canonical terminal event.
    ContainmentFailed,
}

/// The outer supervisor: the final containment and reaping authority. It
/// survives the invocation group signals (it is outside the group's
/// session), inherits the inner supervisor's children when the inner dies
/// with them, and is the only process that reports the canonical terminal
/// event ([`MSG_ALL_CHILDREN_REAPED`]).
///
/// The inner supervisor pid is the invocation's structural ownership
/// anchor and has exactly one reaping owner: this loop. The dedicated
/// observation (`waitid` with `WNOWAIT`) matches only the anchor and never
/// consumes its identity, so a fallback containment `SIGKILL` is always
/// issued while the anchor is still provably allocated. The anchor is
/// released only by the group-scoped gate, strictly after any fallback
/// containment signal. There is deliberately **no generic `waitpid(-1)`
/// reaping loop**: every child of this supervisor is either the anchor or
/// an in-group adopted descendant, so the gate reaps the entire child
/// domain, and a generic loop could only ever consume the anchor and lose
/// the abnormal-exit containment decision.
///
/// An unexpected `ECHILD` from the dedicated observation (before the
/// intentional release) is an ownership invariant violation: it is
/// reported and the outer fails safely — it never derives owned-group
/// terminality from an anchor `ECHILD`, never signals a numeric group id
/// without the retained anchor, and never reports the canonical terminal
/// event.
#[allow(clippy::too_many_lines)] // one coherent observe/un-wedge/contain/reap pipeline
fn run_outer() -> i32 {
    let mut stream = ControlStream;
    if let Err(error) = become_child_subreaper() {
        let _ = stream.write_preownership_failure(&format!(
            "cannot become the invocation subreaper: {error}"
        ));
        return 0;
    }
    if let Err(error) = fcntl(std::io::stdin(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK)) {
        let _ = stream.write_preownership_failure(&format!(
            "cannot make the outer control channel non-blocking: {error}"
        ));
        return 0;
    }
    let inner_pid = match Command::new(supervisor_binary())
        .env("RUSTX_SUPERVISOR_ROLE", ROLE_INNER)
        .spawn()
    {
        Ok(child) => child.id(),
        Err(error) => {
            let _ = stream.write_preownership_failure(&format!(
                "cannot spawn the invocation anchor supervisor: {error}"
            ));
            return 0;
        }
    };
    // The inner supervisor's pid is the invocation's process-group id and
    // its structural ownership anchor: while it remains un-reaped, the
    // numeric group id is provably allocated to this invocation and can
    // never name a foreign group.
    let inner_pid =
        i32::try_from(inner_pid).expect("the inner supervisor pid always fits in an i32");
    let mut anchor = InnerAnchor::Running;
    let mut inner_frozen = false;
    let mut anchor_loss_reported = false;
    loop {
        match anchor {
            InnerAnchor::Running => {
                // The dedicated anchor observation: matches only the inner
                // supervisor and — with `WNOWAIT` — never consumes its
                // identity. The inner stays an un-reaped zombie, so a
                // fallback containment signal still has its structural
                // ownership proof and the anchor is released only by the
                // later group-scoped gate.
                match waitid(
                    Id::Pid(Pid::from_raw(inner_pid)),
                    WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT,
                ) {
                    Ok(WaitStatus::StillAlive) => {}
                    Ok(WaitStatus::Stopped(..) | WaitStatus::Continued(_)) => {}
                    // WEXITED-only waiting: ptrace stops never match.
                    #[cfg(target_os = "linux")]
                    Ok(WaitStatus::PtraceEvent(..) | WaitStatus::PtraceSyscall(_)) => {}
                    Ok(WaitStatus::Exited(_, code)) => {
                        anchor = if code == INNER_EXIT_NORMAL {
                            InnerAnchor::TerminalRetained
                        } else {
                            // Abnormal termination with possibly-live owned
                            // work: active containment while the anchor is
                            // still held (it is observed but un-reaped).
                            contain_after_abnormal_exit(&mut stream, inner_pid)
                        };
                    }
                    Ok(WaitStatus::Signaled(..)) => {
                        anchor = contain_after_abnormal_exit(&mut stream, inner_pid);
                    }
                    Err(Errno::EINTR) => {}
                    Err(Errno::ECHILD) => {
                        // The anchor is not a waitable child of this
                        // supervisor before its intentional release. With
                        // one reaping owner that is an ownership invariant
                        // violation — never a terminal observation: the
                        // owned group may still exist. The outer fails
                        // safely and never derives terminality from it.
                        anchor = InnerAnchor::UnexpectedlyLost;
                        if !anchor_loss_reported {
                            anchor_loss_reported = true;
                            let _ = stream.write_failure(
                                "the invocation anchor became unwaitable before its intentional \
                                 release; owned-group terminality can no longer be proven from \
                                 this supervisor",
                            );
                        }
                    }
                    Err(error) => {
                        let _ = stream.write_failure(&format!(
                            "cannot observe the invocation anchor: {error}"
                        ));
                    }
                }
                // The inner is never legitimately stopped: an observed
                // `SIGSTOP` state is an external freeze of the whole
                // containment unit. The outer un-wedges it with `SIGKILL`,
                // so a frozen anchor can never strand the owned group
                // behind a dead control chain; the inner's death then
                // follows the normal abnormal-exit containment path.
                if !inner_frozen {
                    match waitid(
                        Id::Pid(Pid::from_raw(inner_pid)),
                        WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED,
                    ) {
                        Ok(WaitStatus::Stopped(..)) => {
                            inner_frozen = true;
                            match nix::sys::signal::kill(Pid::from_raw(inner_pid), Signal::SIGKILL)
                            {
                                Ok(()) | Err(Errno::ESRCH) => {}
                                Err(error) => {
                                    let _ = stream.write_failure(&format!(
                                        "cannot un-wedge the frozen invocation anchor: {error}"
                                    ));
                                }
                            }
                        }
                        Ok(WaitStatus::StillAlive | _) | Err(Errno::EINTR | Errno::ECHILD) => {}
                        Err(error) => {
                            let _ = stream.write_failure(&format!(
                                "cannot observe the invocation anchor freeze state: {error}"
                            ));
                        }
                    }
                }
            }
            InnerAnchor::TerminalRetained => {
                // The owned-group gate: the kernel-mediated terminal
                // condition of the invocation process group at the outer
                // level. No child of ours remains in the invocation group
                // once this returns ECHILD — the inner anchor itself is a
                // member and is released (reaped) by this group-scoped
                // wait, which happens strictly after any fallback
                // containment signal. With membership immutable for bash
                // descendants, every in-group process is always a matching
                // child here, so ECHILD is exactly the empty invocation
                // group.
                match waitid(
                    Id::PGid(Pid::from_raw(inner_pid)),
                    WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED,
                ) {
                    Ok(WaitStatus::StillAlive | _) | Err(Errno::EINTR) => {}
                    Err(Errno::ECHILD) => {
                        // macOS: `ECHILD` only proves this supervisor has no
                        // waitable group child left; a reparented descendant
                        // is invisible to it. The group's absence is proven
                        // independently (after the anchor was reaped) before
                        // the terminal frame is emitted.
                        #[cfg(target_os = "macos")]
                        if let Err(error) = prove_group_absent(inner_pid) {
                            let _ = stream.write_failure(&error);
                            anchor = InnerAnchor::ContainmentFailed;
                            continue;
                        }
                        if stream.write_frame(MSG_ALL_CHILDREN_REAPED, &[]).is_ok() {
                            await_terminal_ack();
                        }
                        return 0;
                    }
                    Err(error) => {
                        let _ = stream.write_failure(&format!(
                            "cannot observe the owned group terminal state: {error}"
                        ));
                    }
                }
            }
            InnerAnchor::UnexpectedlyLost | InnerAnchor::ContainmentFailed => {
                // Fail-safe: the owned-group terminal state cannot be
                // proven (the anchor was lost before its intentional
                // release, or the fallback containment signal failed). The
                // specific failure was already reported once above. Never
                // signal the unproven numeric id again and never report
                // the canonical terminal event: stay alive and non-terminal
                // rather than fabricate a terminal state.
            }
        }
        // Test-only deterministic barrier: parks this supervisor after it
        // observed the anchor StillAlive at least once, before the next
        // loop phase. The regressions use it to prove that a zombie anchor
        // can never be consumed by any reaping path other than the
        // dedicated one. Never armed in production (the env var is unset).
        if let Ok(dir) = std::env::var(OUTER_BARRIER_DIR_ENV) {
            let observed = std::path::Path::new(&dir).join("observed");
            if !observed.exists() {
                let _ = std::fs::write(&observed, inner_pid.to_string());
                let proceed = std::path::Path::new(&dir).join("proceed");
                while !proceed.exists() {
                    std::thread::sleep(POLL_INTERVAL);
                }
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Keeps the outer alive until rustX acknowledges parsing the authoritative
/// terminal frame. This prevents unread control input from turning close
/// into `ECONNRESET` and obscuring that frame. The deadline is only process
/// cleanup if rustX disappeared; terminality was already proven by ECHILD.
fn await_terminal_ack() {
    let deadline = std::time::Instant::now() + TERMINAL_ACK_TIMEOUT;
    let mut buffered = Vec::with_capacity(64);
    loop {
        let mut chunk = [0u8; 64];
        match read(std::io::stdin(), &mut chunk) {
            Ok(0) => return,
            Ok(count) => buffered.extend_from_slice(&chunk[..count]),
            Err(Errno::EAGAIN) if std::time::Instant::now() < deadline => {
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(error) => {
                if error == Errno::EINTR {
                    continue;
                }
                return;
            }
        }
        while buffered.len() >= 4 {
            let len = u32::from_le_bytes(buffered[..4].try_into().expect("four bytes")) as usize;
            if buffered.len() < 4 + len {
                break;
            }
            let kind = buffered[4];
            buffered.drain(..4 + len);
            if kind == MSG_TERMINAL_ACK {
                return;
            }
        }
    }
}

/// The outer supervisor's active containment: one final `SIGKILL` to the
/// invocation's process group. This is issued only while the structural
/// anchor is held — the inner supervisor is still un-reaped, so the group
/// id is provably allocated to this invocation — and never afterwards.
///
/// The raw result is classified into
/// [`ContainmentOutcome`](crate::runtime::supervised_unit::ContainmentOutcome):
/// `Contained` (`Ok` or `ESRCH`) versus `Unproven` (`EPERM` and every other
/// error).
fn containment_signal(stream: &mut ControlStream, pgid: i32) -> ContainmentOutcome {
    stream
        .write_frame(
            MSG_SIGNAL_ATTEMPT,
            &signal_attempt_payload(pgid, Signal::SIGKILL, true),
        )
        .ok();
    if std::env::var(FAIL_CONTAINMENT_ENV).is_ok() {
        // Test-only seam: the deterministic stand-in for `killpg` returning
        // `EPERM` — the signal operation was attempted but not authorized.
        return ContainmentOutcome::Unproven(
            "injected containment signal failure (killpg EPERM)".to_owned(),
        );
    }
    contain_group(pgid)
}

/// Applies the platform containment policy to an abnormally-exited anchor
/// and returns the resulting anchor state.
///
/// On Linux an `Unproven` fallback signal is a hard containment failure: the
/// unit fails safely and never reports the canonical terminal event. On
/// macOS the fallback `SIGKILL` result is ambiguous on `EPERM` (the kernel
/// reports a zombie-only group and a live member this caller cannot signal
/// with the same `EPERM`), so it is never itself a terminal fact: the
/// `killpg(pgid, 0)` absence probe after the anchor release is the sole
/// macOS terminal authority, and the anchor proceeds to that probe.
fn contain_after_abnormal_exit(stream: &mut ControlStream, pgid: i32) -> InnerAnchor {
    match containment_signal(stream, pgid) {
        ContainmentOutcome::Contained => InnerAnchor::TerminalRetained,
        ContainmentOutcome::Unproven(error) => {
            #[cfg(target_os = "linux")]
            {
                let _ = stream.write_failure(&error);
                InnerAnchor::ContainmentFailed
            }
            #[cfg(target_os = "macos")]
            {
                let _ = error;
                InnerAnchor::TerminalRetained
            }
        }
    }
}

/// The inner supervisor: the invocation's session/group leader, the shell's
/// parent, the reaper of its own child domain, and the IPC peer that
/// performs the `TERM` -> grace -> `KILL` sequence on its own group. Its
/// terminal gate is the group-scoped wait on the invocation process group:
/// with membership immutable for bash descendants, `ECHILD` there means the
/// shell and every owned member are reaped — the invocation group contains
/// only this supervisor itself.
#[allow(clippy::too_many_lines)] // one coherent session/spawn/reap/terminate pipeline
fn run_inner() -> i32 {
    let mut stream = ControlStream;
    let command = match std::env::var(COMMAND_ENV) {
        Ok(command) if !command.is_empty() => command,
        _ => {
            let _ = stream.write_preownership_failure(
                "the bash command is missing from the supervisor environment",
            );
            return INNER_EXIT_NORMAL;
        }
    };
    if let Err(error) = nix::unistd::setsid() {
        let _ = stream
            .write_preownership_failure(&format!("cannot create the invocation session: {error}"));
        return INNER_EXIT_NORMAL;
    }
    if let Err(error) = become_child_subreaper() {
        let _ = stream.write_preownership_failure(&format!(
            "cannot become the invocation subreaper: {error}"
        ));
        return INNER_EXIT_NORMAL;
    }
    // The invocation group TERM targets this process too; it must survive
    // to keep reaping while bash and its descendants handle the TERM. A
    // handler-installation failure is a pre-ownership setup failure: no
    // bash tree exists yet, so an immediate normal exit (with the explicit
    // failure report) is the correct settlement.
    if std::env::var(FAIL_SIGTERM_HANDLER_ENV).is_ok() {
        let _ = stream.write_preownership_failure("injected SIGTERM handler installation failure");
        return INNER_EXIT_NORMAL;
    }
    if let Err(error) = ignore_group_term() {
        let _ = stream.write_preownership_failure(&format!(
            "cannot install the invocation SIGTERM handler: {error}"
        ));
        return INNER_EXIT_NORMAL;
    }
    // The control channel is non-blocking so the loop can poll for the
    // TERMINATE request without blocking the reap loop.
    if let Err(error) = fcntl(std::io::stdin(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK)) {
        let _ = stream.write_preownership_failure(&format!(
            "cannot make the control channel non-blocking: {error}"
        ));
        return INNER_EXIT_NORMAL;
    }
    // The fixed-membership restriction: from this point on, this process,
    // bash, and every descendant are structurally prevented from changing
    // process-group/session membership (setsid/setpgid are rejected). This
    // is what makes the group-scoped terminal wait complete. An install
    // failure is a pre-ownership setup failure: no bash tree exists yet.
    if let Err(error) = enforce_fixed_group_membership() {
        let _ = stream.write_preownership_failure(&format!(
            "cannot install the fixed process-group membership restriction: {error}"
        ));
        return INNER_EXIT_NORMAL;
    }
    if let Ok(path) = std::env::var(ANCHOR_PID_FILE_ENV) {
        let _ = std::fs::write(&path, std::process::id().to_string());
    }
    let self_pid = i32::try_from(std::process::id()).unwrap_or(0);
    if stream
        .write_frame(MSG_ANCHOR_READY, &self_pid.to_le_bytes())
        .is_err()
    {
        return INNER_EXIT_NORMAL;
    }
    // One buffered reader owns the complete rustX -> inner control
    // direction: the pre-ownership `START` gate and the owned control loop
    // below share it. Unix stream reads do not preserve the writer's frame
    // boundaries, so a frame that arrived in the same `read()` as `START`
    // must stay owned by the owned loop instead of being dropped with a
    // gate-local buffer.
    let mut control_reader = FrameReader::new();
    match await_start(&mut control_reader) {
        Ok(true) => {}
        Ok(false) => {
            let _ = stream.write_frame(MSG_NO_OWNERSHIP, &[]);
            return INNER_EXIT_NORMAL;
        }
        Err(error) => {
            let _ = stream.write_preownership_failure(&error);
            return INNER_EXIT_NORMAL;
        }
    }
    if std::env::var(FAIL_BASH_SPAWN_ENV).is_ok() {
        let _ = stream.write_preownership_failure("injected bash spawn failure");
        return INNER_EXIT_NORMAL;
    }
    // macOS does not provide Linux's child-subreaper reparenting. The EXIT
    // `wait` is a best-effort convenience only: it keeps the ordinary
    // background-job domain attached to the shell so those jobs finish
    // naturally. It is NOT an ownership boundary and is NOT the source of
    // terminal correctness — the user command runs in the same shell and may
    // legally replace the trap, so the macOS terminal proof below (escalate
    // to the outer supervisor's fallback containment once the shell is
    // reaped) stays valid regardless. Linux retains the original command
    // bytes and uses subreaper adoption for descendants.
    #[cfg(target_os = "macos")]
    let shell_command = format!("trap 'wait' EXIT\n{command}");
    #[cfg(not(target_os = "macos"))]
    let shell_command = command;
    let fail_wait = std::env::var(FAIL_WAIT_ENV).is_ok();
    let fail_signal = std::env::var(FAIL_SIGNAL_ENV).is_ok();
    let force_anchor_loss = std::env::var(FORCE_ANCHOR_LOSS_ENV).is_ok();
    let bash = match Command::new("/bin/bash")
        .arg("-c")
        .arg(&shell_command)
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = stream.write_preownership_failure(&format!("cannot spawn /bin/bash: {error}"));
            return INNER_EXIT_NORMAL;
        }
    };
    if stream.write_frame(MSG_OWNERSHIP_ESTABLISHED, &[]).is_err() {
        return INNER_EXIT_CONTAINMENT;
    }
    // SAFETY-free pid capture: the pid is a positive `u32` from the kernel;
    // it is only compared against `waitpid` pids of the same conversion.
    let bash_pid = i32::try_from(bash.id()).unwrap_or(0);
    let mut shell_reported = false;
    let mut kill_deadline: Option<std::time::Instant> = None;
    loop {
        // Reaping hygiene of the inner child domain: matches every child
        // of this process — the shell and owned in-group group members
        // adopted through subreaper reparenting — and consumes them so no
        // zombie is ever left behind. This is deliberately NOT the
        // settlement gate (the group-scoped wait below decides settlement)
        // and it can never consume another owner's identity: no child of
        // the inner supervisor is ever an ownership anchor (the
        // invocation's anchor is this process's own pid, owned by the
        // outer supervisor's dedicated path).
        match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(pid, code)) => {
                if pid.as_raw() == bash_pid && !shell_reported {
                    shell_reported = true;
                    if fail_wait {
                        let _ = stream.write_failure("injected bash wait failure");
                        return INNER_EXIT_CONTAINMENT;
                    }
                    let mut payload = Vec::with_capacity(9);
                    payload.extend_from_slice(&code.to_le_bytes());
                    payload.push(0);
                    payload.extend_from_slice(&0i32.to_le_bytes());
                    if stream.write_frame(MSG_SHELL_EXITED, &payload).is_err() {
                        return INNER_EXIT_CONTAINMENT;
                    }
                }
            }
            Ok(WaitStatus::Signaled(pid, sig, _)) => {
                if pid.as_raw() == bash_pid && !shell_reported {
                    shell_reported = true;
                    if fail_wait {
                        let _ = stream.write_failure("injected bash wait failure");
                        return INNER_EXIT_CONTAINMENT;
                    }
                    let mut payload = Vec::with_capacity(9);
                    payload.extend_from_slice(&0i32.to_le_bytes());
                    payload.push(1);
                    payload.extend_from_slice(&(sig as i32).to_le_bytes());
                    if stream.write_frame(MSG_SHELL_EXITED, &payload).is_err() {
                        return INNER_EXIT_CONTAINMENT;
                    }
                }
            }
            Ok(WaitStatus::StillAlive | _) | Err(Errno::EINTR | Errno::ECHILD) => {
                // Nothing dead to reap (ECHILD), or nothing exited yet; the
                // group gate below decides settlement.
            }
            Err(error) => {
                let _ =
                    stream.write_failure(&format!("cannot wait for the owned children: {error}"));
                return INNER_EXIT_CONTAINMENT;
            }
        }
        // The owned-group gate: the kernel-mediated terminal condition of
        // the invocation process group. Because membership is immutable for
        // bash descendants (setsid/setpgid are rejected by the inherited
        // filter), every in-group process other than this supervisor is a
        // bash descendant that can never leave the group: while bash lives,
        // bash itself is a matching child and blocks the gate, and when an
        // in-group ancestor exits, the kernel reparents its in-group
        // children directly into this supervisor's child domain. ECHILD
        // therefore means no process other than this supervisor remains in
        // the invocation group — the complete terminal state. This is the
        // exact structural point; it is not a probe, a scan, or a timing
        // observation.
        //
        // On macOS this proof does not hold: without a child-subreaper, a
        // descendant that outlives the shell is reparented to launchd and
        // becomes invisible to this supervisor's group-scoped wait, so
        // `ECHILD` only means "no child of this supervisor remains in the
        // group". macOS therefore escalates to the outer supervisor's
        // fallback containment (a `SIGKILL` to the retained group) instead
        // of claiming the group is empty.
        match waitid(
            Id::PGid(Pid::from_raw(self_pid)),
            WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED,
        ) {
            Ok(WaitStatus::StillAlive | _) | Err(Errno::EINTR) => {}
            Err(Errno::ECHILD) => {
                #[cfg(target_os = "macos")]
                return INNER_EXIT_CONTAINMENT;
                #[cfg(not(target_os = "macos"))]
                return INNER_EXIT_NORMAL;
            }
            Err(error) => {
                let _ = stream.write_failure(&format!(
                    "cannot observe the owned group terminal state: {error}"
                ));
                return INNER_EXIT_CONTAINMENT;
            }
        }
        // Control frames from rustX (TERMINATE). Frames that were already
        // read — a `TERMINATE` that shared its `read()` with the `START`
        // gate frame in particular — are drained before another read is
        // issued, so an authoritative cancellation request is acted on even
        // if rustX never writes another byte. The channel is non-blocking;
        // the poll cadence keeps this loop deterministic.
        let mut chunk = [0u8; 256];
        loop {
            if let Err(error) = handle_frames(
                &mut control_reader,
                &mut stream,
                self_pid,
                fail_signal,
                force_anchor_loss,
                &mut kill_deadline,
            ) {
                let _ = stream.write_failure(&error);
                return INNER_EXIT_CONTAINMENT;
            }
            match read(std::io::stdin(), &mut chunk) {
                Ok(0) => {
                    // rustX closed the control channel: the invocation is
                    // abandoned and nobody reads our reports anymore. Owned
                    // work may still be alive, so the exit signals the outer
                    // supervisor to fail-safe-contain the invocation.
                    return INNER_EXIT_CONTAINMENT;
                }
                Ok(control_read) => control_reader.feed(&chunk[..control_read]),
                // non-blocking; EWOULDBLOCK == EAGAIN on Linux
                Err(Errno::EAGAIN) => break,
                Err(error) => {
                    let _ =
                        stream.write_failure(&format!("cannot read the control channel: {error}"));
                    return INNER_EXIT_CONTAINMENT;
                }
            }
        }
        // The grace period after TERM: if the owned tree has not reached
        // its terminal child set by the deadline, KILL the invocation group
        // (including this process; the outer supervisor reaps everything).
        if kill_deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            stream
                .write_frame(
                    MSG_SIGNAL_ATTEMPT,
                    &signal_attempt_payload(self_pid, Signal::SIGKILL, true),
                )
                .ok();
            let _ = killpg(Pid::from_raw(self_pid), Signal::SIGKILL);
            return INNER_EXIT_CONTAINMENT;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Drains every complete control frame already buffered in the owned
/// control direction's reader and handles it.
///
/// This is the single interpretation path for owned control frames: a frame
/// carried over from the `START` gate's `read()` is handled here exactly
/// like a freshly read one, so terminate handling is never duplicated.
/// Partial frames stay buffered in the reader for the next read.
fn handle_frames(
    reader: &mut FrameReader,
    stream: &mut ControlStream,
    self_pid: i32,
    fail_signal: bool,
    force_anchor_loss: bool,
    kill_deadline: &mut Option<std::time::Instant>,
) -> Result<(), String> {
    while let Some((kind, _payload)) = reader.pop() {
        match kind {
            MSG_TERMINATE => {
                if fail_signal {
                    // The injected signaling failure: the TERM cannot be
                    // delivered, so the termination contract cannot be
                    // established. The failure must not leave owned work
                    // running: the error escalates containment to the
                    // outer supervisor (via INNER_EXIT_CONTAINMENT).
                    stream.write_frame(
                        MSG_SIGNAL_ATTEMPT,
                        &signal_attempt_payload(self_pid, Signal::SIGTERM, false),
                    )?;
                    return Err("injected signaling failure".to_owned());
                }
                if force_anchor_loss {
                    // The ownership anchor reads as lost: the inner
                    // supervisor refuses to signal a numeric group id it
                    // can no longer prove is its own. The outer supervisor
                    // remains the containment authority: it structurally
                    // owns the un-reaped inner anchor and may perform the
                    // final fallback signal with that proof.
                    stream.write_frame(
                        MSG_SIGNAL_ATTEMPT,
                        &signal_attempt_payload(self_pid, Signal::SIGTERM, false),
                    )?;
                    return Err(
                        "cannot prove the invocation process group is still owned; signaling is forbidden"
                            .to_owned(),
                    );
                }
                stream.write_frame(
                    MSG_SIGNAL_ATTEMPT,
                    &signal_attempt_payload(self_pid, Signal::SIGTERM, true),
                )?;
                match killpg(Pid::from_raw(self_pid), Signal::SIGTERM) {
                    Ok(()) | Err(Errno::ESRCH) => {}
                    Err(error) => {
                        return Err(format!(
                            "cannot send SIGTERM to the owned process group: {error}"
                        ));
                    }
                }
                // The grace period: the owned tree either reaches its
                // terminal child set (ECHILD in the main loop) or is KILLed
                // at the deadline.
                if kill_deadline.is_none() {
                    *kill_deadline = Some(std::time::Instant::now() + TERM_GRACE);
                }
            }
            other => return Err(format!("unknown control message kind {other:#04x}")),
        }
    }
    Ok(())
}

/// Waits at the pre-ownership gate for rustX to acknowledge that it has
/// retained the invocation anchor. Bash cannot exist before `MSG_START`.
///
/// The reader belongs to the caller for the whole rustX -> inner control
/// direction: already-buffered frames are drained before another `read()`
/// is issued, partial frames stay buffered across reads, and the gate
/// consumes **exactly** its own gate frame. Any valid frame that followed
/// it in the same `read()` — an authoritative `MSG_TERMINATE` in
/// particular — remains owned by the owned control loop that runs next.
fn await_start(reader: &mut FrameReader) -> Result<bool, String> {
    loop {
        if let Some((kind, _payload)) = reader.pop() {
            return match kind {
                MSG_START => Ok(true),
                MSG_TERMINATE => Ok(false),
                other => Err(format!(
                    "unexpected pre-ownership control message {other:#04x}"
                )),
            };
        }
        let mut chunk = [0u8; 32];
        match read(std::io::stdin(), &mut chunk) {
            Ok(0) => return Ok(false),
            Ok(count) => reader.feed(&chunk[..count]),
            Err(Errno::EAGAIN) => std::thread::sleep(POLL_INTERVAL),
            Err(Errno::EINTR) => {}
            Err(error) => return Err(format!("cannot read the ownership start gate: {error}")),
        }
    }
}

/// The supervisor's control stream: fd 0 (stdin), the inherited socket end.
struct ControlStream;

impl ControlStream {
    /// Writes one length-prefixed frame: `[u32 LE length][kind][payload]`.
    #[allow(clippy::unused_self)] // the handle exists to be explicit about the control stream
    fn write_frame(&mut self, kind: u8, payload: &[u8]) -> Result<(), String> {
        let mut frame = Vec::with_capacity(4 + 1 + payload.len());
        let frame_len = u32::try_from(1 + payload.len())
            .map_err(|_| "the control frame is too large".to_owned())?;
        frame.extend_from_slice(&frame_len.to_le_bytes());
        frame.push(kind);
        frame.extend_from_slice(payload);
        write(std::io::stdin(), &frame)
            .map_err(|error| format!("cannot write to the control channel: {error}"))?;
        Ok(())
    }

    /// Reports a process-control failure frame.
    fn write_failure(&mut self, message: &str) -> Result<(), String> {
        self.write_frame(MSG_PROCESS_CONTROL_FAILURE, message.as_bytes())
    }

    fn write_preownership_failure(&mut self, message: &str) -> Result<(), String> {
        self.write_failure(message)?;
        self.write_frame(MSG_NO_OWNERSHIP, &[])
    }
}

/// The `SIGNAL_ATTEMPT` payload for one attempted group signal.
fn signal_attempt_payload(pgid: i32, sig: Signal, emitted: bool) -> Vec<u8> {
    let mut payload = Vec::with_capacity(9);
    payload.extend_from_slice(&pgid.to_le_bytes());
    payload.extend_from_slice(&(sig as i32).to_le_bytes());
    payload.push(u8::from(emitted));
    payload
}

/// The binary to re-execute for the inner supervisor: this process itself.
fn supervisor_binary() -> std::path::PathBuf {
    std::env::current_exe().expect("current executable")
}

// Structural primitives (`become_child_subreaper`, `ignore_group_term`,
// `enforce_fixed_group_membership`) come from the shared supervisor-unit core.

#[cfg(all(
    test,
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )
))]
mod anchor_reaping_tests {
    //! Supervisor ownership regressions: the single-reaper anchor
    //! discipline and the control-frame ownership of the `START` gate.
    //!
    //! These run the real supervisor binary as a subprocess (the same
    //! binary `run_bash_unix` spawns) and drive the exact interleavings
    //! through deterministic barriers and protocol frames — never through
    //! sleeps:
    //!
    //! - the mandatory race regression
    //!   ([`anchor_observed_alive_is_never_consumed_before_its_terminal_observation`]):
    //!   the outer observes the anchor `StillAlive`, the anchor then exits
    //!   abnormally and sits as an un-reaped zombie, and only then does the
    //!   next loop phase run. Before the single-reaper fix the generic
    //!   `waitpid(-1)` hygiene consumed the zombie and the fallback
    //!   containment decision was lost (the outer then waited on a live
    //!   group member forever and never reported the terminal event). With
    //!   single-reaper ownership the zombie survives until the dedicated
    //!   observation, the fallback `SIGKILL` is issued while the anchor is
    //!   retained, and the group-scoped gate releases the anchor.
    //! - the coalesced `START` gate regression
    //!   ([`start_and_terminate_in_one_read_never_loses_the_terminate`]):
    //!   `MSG_START` and `MSG_TERMINATE` are written to the inner supervisor
    //!   as one batch and nothing is written afterwards, so the whole
    //!   invocation must settle from control input that was already sent
    //!   before the gate ran. This is the end-to-end settlement proof of the
    //!   scenario — spawn commit, `TERM` sequence, reaping, empty process
    //!   group. The exact phase-transition proof is deliberately not its
    //!   job: a stream preserves byte order, not write boundaries, so the
    //!   peer's `read()` batching cannot be asserted. That proof is the
    //!   deterministic in-process regression in
    //!   [`super::start_gate_frame_ownership_tests`], which preloads one
    //!   [`FrameReader`] with both frames and needs no kernel at all.

    use super::*;
    use crate::runtime::process_runner::supervisor_binary;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// The strict deadline for every supervisor observation of a test
    /// (a deadlock guard, never a synchronization mechanism).
    const DEADLINE: Duration = Duration::from_secs(15);

    /// Spawns the real outer supervisor with the test-only anchor barrier
    /// armed and returns the child and the rustX-side control stream.
    fn spawn_outer(
        command: &str,
        barrier_dir: &Path,
        anchor_pid_file: &Path,
    ) -> (std::process::Child, UnixStream) {
        let (stream_a, stream_b) = UnixStream::pair().expect("control socket pair");
        let child = std::process::Command::new(supervisor_binary())
            .env("RUSTX_SUPERVISOR_ROLE", ROLE_OUTER)
            .env(COMMAND_ENV, command)
            .env(ANCHOR_PID_FILE_ENV, anchor_pid_file)
            .env(OUTER_BARRIER_DIR_ENV, barrier_dir)
            // The injected wait failure turns the shell's exit into an
            // abnormal inner completion (`INNER_EXIT_CONTAINMENT`) with
            // possibly-live owned work — the exact abnormal-exit topology
            // the race regression must pin.
            .env(FAIL_WAIT_ENV, "1")
            .stdin(Stdio::from(std::os::unix::io::OwnedFd::from(stream_b)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the outer supervisor");
        (child, stream_a)
    }

    /// Spawns the real supervisor for one ordinary invocation: no injected
    /// failure and no barrier, so the only thing under test is the control
    /// protocol itself.
    fn spawn_invocation(
        command: &str,
        anchor_pid_file: &Path,
    ) -> (std::process::Child, UnixStream) {
        let (stream_a, stream_b) = UnixStream::pair().expect("control socket pair");
        let child = std::process::Command::new(supervisor_binary())
            .env("RUSTX_SUPERVISOR_ROLE", ROLE_OUTER)
            .env(COMMAND_ENV, command)
            .env(ANCHOR_PID_FILE_ENV, anchor_pid_file)
            .stdin(Stdio::from(std::os::unix::io::OwnedFd::from(stream_b)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the outer supervisor");
        (child, stream_a)
    }

    /// Contains the invocation and fails.
    ///
    /// A regression that loses the trailing control frame would otherwise
    /// leave a live owned Bash tree behind, so every failure path of the
    /// coalesced-gate regression goes through here: the invocation group is
    /// killed, the supervisor child is killed and reaped, and only then
    /// does the test panic.
    fn contain_and_fail(outer: &mut std::process::Child, pgid: Option<i32>, message: &str) -> ! {
        if let Some(pgid) = pgid {
            let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
        }
        let _ = outer.kill();
        let _ = outer.wait();
        panic!("{message}");
    }

    /// Reads the next complete control frame, containing the invocation
    /// instead of hanging or leaking it when the deadlock guard expires.
    ///
    /// The reader is owned by the caller across calls for exactly the
    /// reason the supervisor owns one per control direction: frames the
    /// peer wrote separately can arrive in a single `read()`, and the
    /// surplus must survive until the next call.
    fn next_frame(
        stream: &mut UnixStream,
        reader: &mut FrameReader,
        outer: &mut std::process::Child,
        pgid: Option<i32>,
        deadline: Instant,
        description: &str,
    ) -> (u8, Vec<u8>) {
        loop {
            if let Some(frame) = reader.pop() {
                return frame;
            }
            let mut chunk = [0u8; 64];
            match stream.read(&mut chunk) {
                Ok(0) => contain_and_fail(
                    outer,
                    pgid,
                    &format!("{description}: the control channel closed"),
                ),
                Ok(count) => reader.feed(&chunk[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        contain_and_fail(outer, pgid, description);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => contain_and_fail(
                    outer,
                    pgid,
                    &format!("{description}: control channel read failed: {error}"),
                ),
            }
        }
    }

    /// Polls for a file with a strict deadlock guard.
    fn wait_for_file(path: &Path) {
        let deadline = Instant::now() + DEADLINE;
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "{} never appeared",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Reads the pid out of a fixture pid file (with a deadlock guard).
    fn read_pid(path: &Path) -> i32 {
        wait_for_file(path);
        std::fs::read_to_string(path)
            .expect("pid file")
            .trim()
            .parse()
            .expect("pid")
    }

    /// The `/proc/<pid>/stat` state character; test-only fixture
    /// inspection — `/proc` is never the production ownership authority.
    fn proc_state(pid: i32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let close = stat.rfind(')')?;
        stat[close + 2..].chars().next()
    }

    /// Polls until the process is an un-reaped zombie (it exited and
    /// nobody consumed it yet). This is the exact state the race
    /// regression must pin before the next reaping phase runs.
    fn wait_for_unreaped_zombie(pid: i32) {
        let deadline = Instant::now() + DEADLINE;
        while proc_state(pid) != Some('Z') {
            assert!(
                Instant::now() < deadline,
                "process {pid} never became an un-reaped zombie"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Polls until the process is fully gone (reaped).
    fn wait_for_reaped(pid: i32) {
        let deadline = Instant::now() + DEADLINE;
        while proc_state(pid).is_some() {
            assert!(Instant::now() < deadline, "process {pid} was never reaped");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Whether any process remains in the numeric process group.
    fn group_alive(pgid: i32) -> bool {
        match killpg(Pid::from_raw(pgid), None) {
            Ok(()) | Err(Errno::EPERM) => true,
            Err(_) => false,
        }
    }

    /// Polls until the numeric process group is provably gone.
    fn wait_for_group_gone(pgid: i32) {
        let deadline = Instant::now() + DEADLINE;
        while group_alive(pgid) {
            assert!(
                Instant::now() < deadline,
                "process group {pgid} never became terminal"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Reads complete length-prefixed control frames with a strict
    /// deadline: `(kind, payload)`. `None` is a closed channel.
    fn read_frame(stream: &mut UnixStream, deadline: Instant) -> Option<(u8, Vec<u8>)> {
        let mut buf = Vec::new();
        loop {
            let mut chunk = [0u8; 64];
            match stream.read(&mut chunk) {
                Ok(0) => return None,
                Ok(count) => buf.extend_from_slice(&chunk[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("control channel read failed: {error}"),
            }
            if buf.len() >= 4 {
                let len = u32::from_le_bytes(buf[..4].try_into().expect("four bytes")) as usize;
                if buf.len() >= 4 + len {
                    let kind = buf[4];
                    let payload = buf[5..4 + len].to_vec();
                    return Some((kind, payload));
                }
            }
            assert!(
                Instant::now() < deadline,
                "deadline waiting for a control frame"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Sends the rustX-side START acknowledgement (authorizes the Bash
    /// spawn) over the control channel.
    fn send_start(stream: &mut UnixStream) {
        stream.write_all(&[1, 0, 0, 0, MSG_START]).expect("START");
    }

    /// Sends the terminal acknowledgement so the outer supervisor can exit.
    fn send_terminal_ack(stream: &mut UnixStream) {
        stream
            .write_all(&[1, 0, 0, 0, MSG_TERMINAL_ACK])
            .expect("ack");
    }

    /// The mandatory race regression:
    ///
    /// ```text
    /// outer dedicated observation: inner == StillAlive   (barrier parks here)
    /// inner exits abnormally (containment) and sits as an un-reaped zombie
    /// the next loop phase runs                             (barrier released)
    /// ```
    ///
    /// Single-reaper ownership requires the zombie anchor to survive into
    /// the dedicated observation, the fallback `SIGKILL` to be issued
    /// while the anchor is retained, and the group-scoped gate to release
    /// the anchor afterwards. Before the fix, a generic `waitpid(-1)`
    /// hygiene loop consumed the zombie in the released phase, the next
    /// dedicated observation saw `ECHILD`, and the fallback containment
    /// decision was lost — the outer then waited on the still-live group
    /// member forever and never reported the terminal event. This test
    /// therefore fails deterministically on the pre-fix code and passes
    /// only with the single-reaper design.
    #[test]
    fn anchor_observed_alive_is_never_consumed_before_its_terminal_observation() {
        let dir = tempfile::tempdir().expect("temp dir");
        let barrier_dir = dir.path().join("barrier");
        std::fs::create_dir_all(&barrier_dir).expect("barrier dir");
        let anchor_pid_file = dir.path().join("anchor.pid");
        let sleep_pid_file = dir.path().join("sleep.pid");
        // The fixture owns live work (a background `sleep 30`) when the
        // inner exits abnormally: the injected wait failure fires on the
        // shell's exit and escalates containment, exactly like a real
        // abnormal inner completion with possibly-live owned work.
        let command = format!(
            "sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
            sleep_pid_file.display()
        );
        let (mut outer, mut stream) = spawn_outer(&command, &barrier_dir, &anchor_pid_file);
        nix::fcntl::fcntl(
            &stream,
            nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
        )
        .expect("nonblocking control stream");
        let deadline = Instant::now() + DEADLINE;

        // 1. The outer provably observed the anchor StillAlive and parked
        //    at the barrier: the exact dedicated-observation point.
        wait_for_file(&barrier_dir.join("observed"));
        let inner_pid = read_pid(&anchor_pid_file);

        // 2. Authorize the Bash spawn: the inner (which is gated on
        //    `START` before spawning bash) then starts the fixture and the
        //    injected wait failure turns the shell's exit into
        //    `INNER_EXIT_CONTAINMENT` — while the outer stays parked at
        //    the barrier.
        send_start(&mut stream);
        let sleep_pid = read_pid(&sleep_pid_file);
        wait_for_unreaped_zombie(inner_pid);
        assert!(
            group_alive(inner_pid),
            "the owned group must still be live when the anchor sits as a zombie"
        );
        assert!(
            proc_state(sleep_pid).is_some(),
            "the owned descendant must still exist when the anchor sits as a zombie"
        );

        // 3. The next loop phase runs: the anchor may only be observed and
        //    later released by the dedicated path. The mandatory proof:
        //    the fallback containment signal is issued while the anchor is
        //    retained, and the canonical terminal frame still arrives.
        let _ = std::fs::write(barrier_dir.join("proceed"), "go");
        let mut saw_containment_signal = false;
        let mut saw_terminal_frame = false;
        while !saw_terminal_frame {
            let (kind, payload) = read_frame(&mut stream, deadline).unwrap_or_else(|| {
                panic!(
                    "the outer supervisor never reported the terminal event; \
                     the anchor (pid {inner_pid}) was consumed before its \
                     terminal observation and the fallback containment \
                     decision was lost"
                )
            });
            match kind {
                MSG_SIGNAL_ATTEMPT => {
                    assert!(payload.len() >= 9, "malformed signal-attempt frame");
                    let pgid = i32::from_le_bytes(payload[0..4].try_into().expect("four bytes"));
                    let signal = i32::from_le_bytes(payload[4..8].try_into().expect("four bytes"));
                    let emitted = payload[8] != 0;
                    assert_eq!(
                        pgid, inner_pid,
                        "the fallback signal targets the anchor pgid"
                    );
                    assert_eq!(signal, libc::SIGKILL);
                    assert!(emitted, "the fallback containment must reach the kernel");
                    saw_containment_signal = true;
                }
                MSG_PROCESS_CONTROL_FAILURE | MSG_ANCHOR_READY => {
                    // The inner's injected wait-failure report and its
                    // pre-start gate report; both buffered before the
                    // containment sequence and expected.
                }
                MSG_ALL_CHILDREN_REAPED => saw_terminal_frame = true,
                other => panic!("unexpected control frame kind {other:#04x}"),
            }
        }
        assert!(
            saw_containment_signal,
            "the abnormal anchor exit must trigger exactly the fallback containment signal"
        );

        // 4. Acknowledge; the outer exits normally and everything it owned
        //    was reaped by its gate: the anchor and the descendant are
        //    provably gone and the numeric group no longer exists.
        send_terminal_ack(&mut stream);
        let status = outer.wait().expect("outer supervisor wait");
        assert!(
            status.success(),
            "the outer must exit 0 after the terminal event"
        );
        wait_for_reaped(inner_pid);
        wait_for_reaped(sleep_pid);
        wait_for_group_gone(inner_pid);
    }

    /// A failed fallback containment signal is never a terminal result.
    ///
    /// The injected seam models `killpg` returning `EPERM`: the signal was
    /// attempted (the observable `MSG_SIGNAL_ATTEMPT` carries `emitted`)
    /// but not authorized. The outer supervisor must report the explicit
    /// containment failure and must never emit the canonical
    /// `MSG_ALL_CHILDREN_REAPED` terminal frame while the owned group may
    /// still be live.
    #[test]
    #[allow(clippy::too_many_lines)] // one coherent deterministic fixture
    fn failed_containment_signal_never_reports_terminal() {
        let dir = tempfile::tempdir().expect("temp dir");
        let barrier_dir = dir.path().join("barrier");
        std::fs::create_dir_all(&barrier_dir).expect("barrier dir");
        let anchor_pid_file = dir.path().join("anchor.pid");
        let sleep_pid_file = dir.path().join("sleep.pid");
        let command = format!(
            "sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
            sleep_pid_file.display()
        );
        let (stream_a, stream_b) = UnixStream::pair().expect("control socket pair");
        let mut outer = std::process::Command::new(supervisor_binary())
            .env("RUSTX_SUPERVISOR_ROLE", ROLE_OUTER)
            .env(COMMAND_ENV, &command)
            .env(ANCHOR_PID_FILE_ENV, &anchor_pid_file)
            .env(OUTER_BARRIER_DIR_ENV, &barrier_dir)
            // The injected wait failure turns the shell's exit into an
            // abnormal inner completion (`INNER_EXIT_CONTAINMENT`) with
            // possibly-live owned work.
            .env(FAIL_WAIT_ENV, "1")
            // The injected containment failure is the deterministic stand-in
            // for `killpg` returning `EPERM`.
            .env(FAIL_CONTAINMENT_ENV, "1")
            .stdin(Stdio::from(std::os::unix::io::OwnedFd::from(stream_b)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the outer supervisor");
        let mut stream = stream_a;
        nix::fcntl::fcntl(
            &stream,
            nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
        )
        .expect("nonblocking control stream");

        // 1. The outer provably observed the anchor StillAlive and parked
        //    at the barrier.
        wait_for_file(&barrier_dir.join("observed"));
        let inner_pid = read_pid(&anchor_pid_file);

        // 2. Authorize the Bash spawn; the injected wait failure turns the
        //    shell exit into `INNER_EXIT_CONTAINMENT` while the outer stays
        //    parked, leaving the owned descendant live.
        send_start(&mut stream);
        let sleep_pid = read_pid(&sleep_pid_file);
        wait_for_unreaped_zombie(inner_pid);
        assert!(
            group_alive(inner_pid),
            "the owned group must still be live while the anchor sits as a zombie"
        );
        assert!(
            proc_state(sleep_pid).is_some(),
            "the owned descendant must still exist when containment is attempted"
        );

        // 3. The next loop phase runs: the outer observes the abnormal
        //    inner exit and attempts the fallback containment, which the
        //    injected seam fails with the EPERM stand-in.
        let _ = std::fs::write(barrier_dir.join("proceed"), "go");
        let deadline = Instant::now() + DEADLINE;
        let mut reader = FrameReader::new();
        let mut saw_signal_attempt = false;
        let mut saw_containment_failure = false;
        while !saw_containment_failure {
            let (kind, payload) = next_frame(
                &mut stream,
                &mut reader,
                &mut outer,
                Some(inner_pid),
                deadline,
                "the outer supervisor never reported the containment failure",
            );
            match kind {
                MSG_SIGNAL_ATTEMPT => {
                    assert!(payload.len() >= 9, "malformed signal-attempt frame");
                    let signal = i32::from_le_bytes(payload[4..8].try_into().expect("four bytes"));
                    let emitted = payload[8] != 0;
                    assert_eq!(signal, libc::SIGKILL, "the containment signal is SIGKILL");
                    assert!(emitted, "the containment attempt must reach the kernel");
                    saw_signal_attempt = true;
                }
                MSG_PROCESS_CONTROL_FAILURE => {
                    let message = String::from_utf8_lossy(&payload).into_owned();
                    if message.contains("containment") {
                        saw_containment_failure = true;
                    }
                }
                MSG_ANCHOR_READY | MSG_OWNERSHIP_ESTABLISHED => {}
                MSG_ALL_CHILDREN_REAPED => {
                    panic!("a failed containment signal must never produce the terminal frame")
                }
                other => panic!("unexpected control frame kind {other:#04x}"),
            }
        }
        assert!(
            saw_signal_attempt,
            "the containment attempt must be observable before its failure"
        );

        // 4. The owned group is still live (the containment signal failed),
        //    and no terminal frame may arrive: a bounded-absence proof with
        //    the same persistent reader so surplus bytes are never dropped.
        let absence_deadline = Instant::now() + Duration::from_secs(1);
        loop {
            while let Some((kind, _payload)) = reader.pop() {
                assert_ne!(
                    kind, MSG_ALL_CHILDREN_REAPED,
                    "a failed containment signal must never emit the terminal frame"
                );
            }
            let mut chunk = [0u8; 64];
            match stream.read(&mut chunk) {
                Ok(0) => panic!("the outer supervisor exited without a terminal frame"),
                Ok(count) => reader.feed(&chunk[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("control channel read failed: {error}"),
            }
            if Instant::now() >= absence_deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        // 5. Test-side cleanup: the containment correctly failed, so the
        //    test terminates the still-live owned group and reaps the stuck
        //    outer. The adopted descendants are zombies of the now-dead
        //    outer and are reparented to init (this test process is not a
        //    subreaper and is never their reaper); they are dead, never live
        //    work.
        let _ = killpg(Pid::from_raw(inner_pid), Signal::SIGKILL);
        let _ = outer.kill();
        let _ = outer.wait();
    }

    /// The end-to-end regression of the coalesced `START` gate scenario.
    ///
    /// rustX's `START` acknowledgement and a cancellation request that
    /// became observable immediately afterwards are written to the inner
    /// supervisor as one batch, after its own `MSG_ANCHOR_READY`:
    ///
    /// ```text
    /// inner -> rustX:  MSG_ANCHOR_READY
    /// rustX -> inner:  MSG_START || MSG_TERMINATE      (one write, then silence)
    /// ```
    ///
    /// In practice the inner supervisor's 32-byte gate read returns both
    /// frames, which is what exercised the pre-fix gate-local buffer. A
    /// stream preserves byte order and not write boundaries, though, so
    /// this test does **not** prove that the peer saw both frames in one
    /// `read()`; that exact phase-transition proof belongs to the
    /// deterministic preloaded-reader regression in
    /// [`super::start_gate_frame_ownership_tests`].
    ///
    /// What this test proves is the real settlement of the scenario: no
    /// control byte is written after the batch, so nothing but control
    /// input that was already sent before the gate ran can drive the
    /// invocation. `MSG_OWNERSHIP_ESTABLISHED` proves the owned Bash spawn
    /// committed, the `SIGTERM` attempt proves the terminate request was
    /// interpreted by the owned loop, and `MSG_ALL_CHILDREN_REAPED` plus
    /// the reaped anchor and the empty process group prove the whole owned
    /// tree physically settled.
    #[test]
    fn start_and_terminate_in_one_read_never_loses_the_terminate() {
        let dir = tempfile::tempdir().expect("temp dir");
        let anchor_pid_file = dir.path().join("anchor.pid");
        // `exec` replaces the shell, so the invocation group holds exactly
        // the inner supervisor and one long-lived owned process: the
        // fixture can never settle by finishing on its own within any test
        // deadline, so only the invocation's own TERM sequence can settle
        // it.
        let (mut outer, mut stream) = spawn_invocation("exec sleep 600", &anchor_pid_file);
        nix::fcntl::fcntl(
            &stream,
            nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
        )
        .expect("nonblocking control stream");
        let deadline = Instant::now() + DEADLINE;
        let mut reader = FrameReader::new();

        // 1. The synchronization point — never a sleep: the inner
        //    supervisor's own `MSG_ANCHOR_READY`. Once it is read, the
        //    inner provably completed its session/subreaper/seccomp setup
        //    and is waiting at the `START` gate.
        let (kind, payload) = next_frame(
            &mut stream,
            &mut reader,
            &mut outer,
            None,
            deadline,
            "the inner supervisor must announce the invocation anchor",
        );
        assert_eq!(
            kind, MSG_ANCHOR_READY,
            "the anchor announcement is the first frame of the invocation"
        );
        let inner_pid = i32::from_le_bytes(payload[..4].try_into().expect("four bytes"));
        assert_eq!(
            inner_pid,
            read_pid(&anchor_pid_file),
            "the announced anchor is the inner supervisor's own pid"
        );

        // 2. One `write_all` of two complete frames — exactly like a
        //    cancellation that becomes observable right after the START
        //    acknowledgement was written. Byte order is guaranteed; the
        //    peer's read batching is not asserted here.
        let mut batch = Vec::with_capacity(10);
        batch.extend_from_slice(&[1, 0, 0, 0, MSG_START]);
        batch.extend_from_slice(&[1, 0, 0, 0, MSG_TERMINATE]);
        stream
            .write_all(&batch)
            .expect("the coalesced START/TERMINATE batch");

        // 3. Not one further control byte is written. `MSG_OWNERSHIP_ESTABLISHED`
        //    proves START was consumed and the Bash spawn (the OS ownership
        //    commit) happened; the `SIGTERM` attempt against the invocation
        //    group is written only by the owned loop's terminate path, so it
        //    proves the trailing frame survived the gate; `MSG_ALL_CHILDREN_REAPED`
        //    is the outer supervisor's kernel-mediated terminal proof.
        let mut saw_ownership = false;
        let mut saw_terminate_signal = false;
        let mut saw_terminal_event = false;
        while !saw_terminal_event {
            let (kind, payload) = next_frame(
                &mut stream,
                &mut reader,
                &mut outer,
                Some(inner_pid),
                deadline,
                "the invocation never terminated: the TERMINATE frame that shared its read with \
                 START was lost at the pre-ownership gate",
            );
            match kind {
                MSG_OWNERSHIP_ESTABLISHED => saw_ownership = true,
                MSG_SIGNAL_ATTEMPT => {
                    assert!(payload.len() >= 9, "malformed signal-attempt frame");
                    let pgid = i32::from_le_bytes(payload[0..4].try_into().expect("four bytes"));
                    let signal = i32::from_le_bytes(payload[4..8].try_into().expect("four bytes"));
                    let emitted = payload[8] != 0;
                    assert_eq!(pgid, inner_pid, "the signal targets the invocation group");
                    assert!(emitted, "the TERM sequence must reach the kernel");
                    if signal == libc::SIGTERM {
                        saw_terminate_signal = true;
                    }
                }
                // The shell's canonical exit status; the fixture is killed
                // by the TERM sequence, so its exact shape is not asserted.
                MSG_SHELL_EXITED => {}
                MSG_ALL_CHILDREN_REAPED => saw_terminal_event = true,
                MSG_PROCESS_CONTROL_FAILURE => contain_and_fail(
                    &mut outer,
                    Some(inner_pid),
                    &format!(
                        "unexpected process-control failure: {}",
                        String::from_utf8_lossy(&payload)
                    ),
                ),
                other => contain_and_fail(
                    &mut outer,
                    Some(inner_pid),
                    &format!("unexpected control frame kind {other:#04x}"),
                ),
            }
        }
        assert!(
            saw_ownership,
            "START must be consumed and commit the owned Bash spawn"
        );
        assert!(
            saw_terminate_signal,
            "the trailing TERMINATE must drive the TERM sequence without another control write"
        );

        // 4. Closing the rustX end ends the outer's terminal-ack wait
        //    without writing a control byte. Everything the invocation
        //    owned is provably gone: the anchor is reaped and the numeric
        //    invocation group no longer exists, so no Bash descendant
        //    remains.
        drop(stream);
        let status = outer.wait().expect("outer supervisor wait");
        assert!(
            status.success(),
            "the outer must exit 0 after the terminal event"
        );
        wait_for_reaped(inner_pid);
        wait_for_group_gone(inner_pid);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod start_gate_frame_ownership_tests {
    //! The deterministic proof of the `START` gate's control-frame
    //! ownership: one buffered reader, in-process, with nothing between the
    //! two facts it relates.
    //!
    //! A Unix stream preserves byte order, not the writer's write
    //! boundaries, so a regression that writes `START || TERMINATE` into a
    //! socket cannot *prove* the peer saw both frames in one `read()` — the
    //! kernel may legally split them, and a gate-local reader would then
    //! survive by accident because the second frame was still unread in the
    //! kernel. This regression removes the kernel from the proof entirely:
    //! the [`FrameReader`] is preloaded with both complete frames before the
    //! gate runs, so "already read from the stream" is established by
    //! construction and the only thing under test is the phase transition:
    //!
    //! ```text
    //! preload: [START][TERMINATE]  -> one FrameReader
    //!     await_start consumes exactly START
    //!     the same reader still owns a complete TERMINATE
    //! ```
    //!
    //! Its whole responsibility is that ownership assertion. What the owned
    //! control loop then *does* with the surviving `TERMINATE` — the `TERM`
    //! sequence, reaping, and the empty process group — is the separate
    //! end-to-end regression
    //! [`super::anchor_reaping_tests::start_and_terminate_in_one_read_never_loses_the_terminate`].
    //! Keeping the two layers apart is what lets this one stay a pure
    //! buffered-reader test: no socket, no `read`, no `write`, no `dup`, no
    //! process-global fd state, no subprocess, no sleep and no scheduler
    //! assumption participates in it.

    use super::*;

    /// One control frame with an empty payload, as it appears on the wire:
    /// `[u32 LE length][kind]`.
    fn frame(kind: u8) -> [u8; 5] {
        [1, 0, 0, 0, kind]
    }

    /// The deterministic `START` gate phase-transition regression.
    ///
    /// One [`FrameReader`] owns a complete `START` and a complete
    /// `TERMINATE` frame before the gate executes. [`await_start`] must
    /// consume exactly the `START` frame and leave the `TERMINATE` owned by
    /// the same reader — intact enough that the very next `pop` parses it as
    /// the authoritative terminate request the owned control loop reads.
    /// Nothing is fed into the reader between the preload and the final
    /// assertions, so its surviving contents can only be what the gate
    /// chose not to discard.
    ///
    /// A gate-local reader fails this immediately and deterministically:
    /// `await_start` would return the same `Ok(true)`, but the reader would
    /// be empty afterwards and the `TERMINATE` unrecoverable.
    #[test]
    fn preloaded_terminate_survives_the_start_gate() {
        // 1. One reader owns both complete frames before the gate starts.
        //    This is the byte-level state a coalesced `read()` produces —
        //    established here by construction, not by the kernel.
        let mut reader = FrameReader::new();
        let mut preloaded = Vec::with_capacity(10);
        preloaded.extend_from_slice(&frame(MSG_START));
        preloaded.extend_from_slice(&frame(MSG_TERMINATE));
        reader.feed(&preloaded);
        assert_eq!(
            reader.buffered(),
            preloaded.as_slice(),
            "the gate must start with both complete frames already read"
        );

        // 2. The gate returns its existing successful START result, which
        //    is what authorizes the owned Bash spawn.
        assert_eq!(
            await_start(&mut reader),
            Ok(true),
            "the buffered START authorizes the owned spawn"
        );

        // 3. It consumed exactly that one frame: the trailing TERMINATE is
        //    still owned by the same reader after the lifecycle phase
        //    transition, byte for byte.
        assert_eq!(
            reader.buffered(),
            frame(MSG_TERMINATE),
            "the gate consumes exactly START and keeps the trailing frame owned"
        );

        // 4. The survivor is a *usable* frame, not just surplus bytes: the
        //    owned control loop's only input step parses it as the
        //    authoritative terminate request, and nothing else remains.
        assert_eq!(
            reader.pop(),
            Some((MSG_TERMINATE, Vec::new())),
            "the surviving frame parses as the authoritative terminate request"
        );
        assert!(
            reader.buffered().is_empty(),
            "exactly two frames were preloaded and exactly two were consumed"
        );
    }
}
