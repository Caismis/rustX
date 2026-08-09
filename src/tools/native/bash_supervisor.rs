//! The per-invocation Bash process supervisor (Linux).
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
//! [`enforce_fixed_group_membership`]) between its own `setsid()` setup and
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
//! The syscall numbers are **detected at runtime** by a side-effect probe
//! in a throwaway child, never guessed from a table: the Linux x86-64
//! syscall numbers were renumbered in kernel 7.0 (an asm-generic-style
//! table), while older kernels and the other supported ABIs keep their own
//! numbers. The restriction is installed with `PR_SET_NO_NEW_PRIVS` plus a
//! `seccomp` filter and requires no privileges. A failure to install it is
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
//!   child of the inner that is in the invocation group is reaped; it then
//!   exits with [`INNER_EXIT_NORMAL`];
//! - the outer supervisor's group-scoped wait returning `ECHILD` — no
//!   child of the outer remains in the invocation group (the inner anchor
//!   itself is a member and is released by this same wait, strictly after
//!   any fallback containment signal); it then reports
//!   [`MSG_ALL_CHILDREN_REAPED`] and exits.
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
//! the dedicated [`INNER_EXIT_CONTAINMENT`] status. The outer supervisor —
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
//! reach; the bounded confirmation window then settles the invocation as
//! an explicit bounded failure (never an unbounded wait).
//!
//! # IPC
//!
//! rustX creates one `UnixStream` pair. The child end becomes the outer
//! supervisor's stdin (fd 0) and is inherited by the inner supervisor,
//! which is the only supervisor process that reads it (the outer never
//! reads, so the inner's lifecycle events can never be consumed by it).
//! bash gets a null stdin so it never sees the control channel. Messages
//! are length-prefixed frames: `[u32 LE length][u8 kind][payload]`. The
//! inner supervisor writes [`MSG_SHELL_EXITED`], [`MSG_PROCESS_CONTROL_FAILURE`],
//! and [`MSG_SIGNAL_ATTEMPT`] (test observability); the outer supervisor
//! writes [`MSG_ALL_CHILDREN_REAPED`] — the single authoritative terminal
//! report — plus [`MSG_PROCESS_CONTROL_FAILURE`] and [`MSG_SIGNAL_ATTEMPT`]
//! for its fallback containment; rustX writes [`MSG_TERMINATE`] to request
//! the `TERM` -> grace -> `KILL` sequence. There is no generic IPC
//! framework.
//!
//! # Platform assumption
//!
//! The subreaper mechanism is Linux-specific (`PR_SET_CHILD_SUBREAPER`),
//! and the fixed-membership restriction uses the Linux `seccomp`/`prctl`
//! primitives. The crate already requires `/bin/bash`; the lifecycle
//! contract is claimed only on Linux (`x86_64`, `aarch64`, and `riscv64`
//! are supported; building for any other architecture is a compile-time
//! error). On unsupported systems the supervisor reports an explicit setup
//! failure, which settles the invocation as `Failed` — the same contract
//! is never silently weakened.

use std::process::{Command, Stdio};
use std::time::Duration;

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid, waitpid};
use nix::unistd::{Pid, read, write};

/// The outer supervisor role name in `RUSTX_SUPERVISOR_ROLE`.
pub const ROLE_OUTER: &str = "outer";

/// The inner supervisor role name in `RUSTX_SUPERVISOR_ROLE`.
pub const ROLE_INNER: &str = "inner";

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

/// The shell's canonical exit status: `{ exit_code: i32 LE, signaled: u8,
/// signal: i32 LE }`.
const MSG_SHELL_EXITED: u8 = 0x02;

/// All invocation-owned children are reaped (kernel `ECHILD` reached).
const MSG_ALL_CHILDREN_REAPED: u8 = 0x03;

/// A process-control failure; payload is the human-readable message.
const MSG_PROCESS_CONTROL_FAILURE: u8 = 0x04;

/// One attempted group signal for test observability:
/// `{ pgid: i32 LE, signal: i32 LE, emitted: u8 }`.
const MSG_SIGNAL_ATTEMPT: u8 = 0x05;

/// rustX -> supervisor: run the `TERM` -> grace -> `KILL` sequence.
const MSG_TERMINATE: u8 = 0x10;

/// The inner supervisor's exit status for a normal completion: it reached
/// the kernel `ECHILD` terminal child state (or no owned process tree was
/// ever created), so the outer supervisor only needs to reap it and its
/// child domain is provably empty.
const INNER_EXIT_NORMAL: i32 = 0;

/// The inner supervisor's exit status for an abnormal termination with
/// possibly-live owned work: the outer supervisor must actively contain
/// the invocation process group (one structurally-anchored fallback
/// `SIGKILL`) before reaping.
const INNER_EXIT_CONTAINMENT: i32 = 42;

/// The internal poll cadence of the supervisor loops (an implementation
/// detail of the grace period and the wait loops — never a test
/// synchronization mechanism).
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The `TERM` -> `KILL` grace period, kept in sync with
/// `crate::tools::limits::BASH_TERM_GRACE`.
const TERM_GRACE: Duration = Duration::from_secs(2);

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

/// The outer supervisor: the final containment and reaping authority. It
/// survives the invocation group signals (it is outside the group's
/// session), inherits the inner supervisor's children when the inner dies
/// with them, and is the only process that reports the canonical terminal
/// event ([`MSG_ALL_CHILDREN_REAPED`]).
///
/// On abnormal inner termination with possibly-live owned work it becomes
/// an **active** containment authority: it observes the inner's terminal
/// state without releasing its identity (`waitid` + `WNOWAIT` keeps the
/// inner an un-reaped zombie), sends the one fallback containment
/// `SIGKILL` to the invocation group while the anchor is still provably
/// allocated, and only then releases the anchor. The canonical terminal
/// event is the outer's group-scoped wait reaching `ECHILD`: no child of
/// the outer remains in the invocation process group, which — with
/// membership immutable for bash descendants — is exactly the empty
/// invocation group.
#[allow(clippy::too_many_lines)] // one coherent observe/un-wedge/contain/reap pipeline
fn run_outer() -> i32 {
    let mut stream = ControlStream;
    if let Err(error) = become_child_subreaper() {
        let _ = stream.write_failure(&format!("cannot become the invocation subreaper: {error}"));
        return 0;
    }
    let inner_pid = match Command::new(supervisor_binary())
        .env("RUSTX_SUPERVISOR_ROLE", ROLE_INNER)
        .spawn()
    {
        Ok(child) => child.id(),
        Err(error) => {
            let _ = stream.write_failure(&format!(
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
    let mut inner_terminal = false;
    let mut inner_frozen = false;
    loop {
        if !inner_terminal {
            // Observe the inner's terminal state without releasing its
            // identity. `WNOWAIT` leaves the inner an un-reaped zombie, so a
            // fallback containment signal still has its structural ownership
            // proof and the anchor is released only by a later reap.
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
                    inner_terminal = true;
                    if code != INNER_EXIT_NORMAL {
                        // Abnormal termination with possibly-live owned work:
                        // active containment while the anchor is still held.
                        containment_signal(&mut stream, inner_pid);
                    }
                }
                Ok(WaitStatus::Signaled(..)) => {
                    inner_terminal = true;
                    containment_signal(&mut stream, inner_pid);
                }
                Err(Errno::EINTR) => {}
                Err(Errno::ECHILD) => {
                    // The inner is not a child of this supervisor (it was
                    // already reaped by us, which cannot normally happen).
                    inner_terminal = true;
                }
                Err(error) => {
                    let _ = stream
                        .write_failure(&format!("cannot observe the invocation anchor: {error}"));
                }
            }
            // The inner is never legitimately stopped: an observed `SIGSTOP`
            // state is an external freeze of the whole containment unit. The
            // outer un-wedges it with `SIGKILL`, so a frozen anchor can never
            // strand the owned group behind a dead control chain; the inner's
            // death then follows the normal abnormal-exit containment path.
            if !inner_frozen {
                match waitid(
                    Id::Pid(Pid::from_raw(inner_pid)),
                    WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED,
                ) {
                    Ok(WaitStatus::Stopped(..)) => {
                        inner_frozen = true;
                        match nix::sys::signal::kill(Pid::from_raw(inner_pid), Signal::SIGKILL) {
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
        if inner_terminal {
            // The owned-group gate: the kernel-mediated terminal condition
            // of the invocation process group at the outer level. No child
            // of ours remains in the invocation group once this returns
            // ECHILD — the inner anchor itself is a member and is released
            // (reaped) by this group-scoped wait, which happens strictly
            // after any fallback containment signal. With membership
            // immutable for bash descendants, every in-group process is
            // always a matching child here, so ECHILD is exactly the empty
            // invocation group.
            match waitid(
                Id::PGid(Pid::from_raw(inner_pid)),
                WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED,
            ) {
                Ok(WaitStatus::StillAlive | _) | Err(Errno::EINTR) => {}
                Err(Errno::ECHILD) => {
                    let _ = stream.write_frame(MSG_ALL_CHILDREN_REAPED, &[]);
                    return 0;
                }
                Err(error) => {
                    let _ = stream.write_failure(&format!(
                        "cannot observe the owned group terminal state: {error}"
                    ));
                }
            }
        }
        // Reaping hygiene: whatever else died in our child domain
        // (members not yet reaped by the gate) is reaped so no zombie is
        // settlement gate: live adopted children outside the invocation
        // group are handed to init when this supervisor exits.
        loop {
            match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::StillAlive | _) | Err(Errno::ECHILD) => break,
                Err(Errno::EINTR) => {}
                Err(error) => {
                    let _ = stream
                        .write_failure(&format!("cannot wait for the owned children: {error}"));
                    break;
                }
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The outer supervisor's active containment: one final `SIGKILL` to the
/// invocation's process group. This is issued only while the structural
/// anchor is held — the inner supervisor is still un-reaped, so the group
/// id is provably allocated to this invocation — and never afterwards.
fn containment_signal(stream: &mut ControlStream, pgid: i32) {
    stream
        .write_frame(
            MSG_SIGNAL_ATTEMPT,
            &signal_attempt_payload(pgid, Signal::SIGKILL, true),
        )
        .ok();
    match killpg(Pid::from_raw(pgid), Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(error) => {
            let _ = stream.write_failure(&format!(
                "cannot contain the owned invocation group: {error}"
            ));
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
            let _ =
                stream.write_failure("the bash command is missing from the supervisor environment");
            return INNER_EXIT_NORMAL;
        }
    };
    if let Err(error) = nix::unistd::setsid() {
        let _ = stream.write_failure(&format!("cannot create the invocation session: {error}"));
        return INNER_EXIT_NORMAL;
    }
    if let Err(error) = become_child_subreaper() {
        let _ = stream.write_failure(&format!("cannot become the invocation subreaper: {error}"));
        return INNER_EXIT_NORMAL;
    }
    // The invocation group TERM targets this process too; it must survive
    // to keep reaping while bash and its descendants handle the TERM. A
    // handler-installation failure is a pre-ownership setup failure: no
    // bash tree exists yet, so an immediate normal exit (with the explicit
    // failure report) is the correct settlement.
    if std::env::var(FAIL_SIGTERM_HANDLER_ENV).is_ok() {
        let _ = stream.write_failure("injected SIGTERM handler installation failure");
        return INNER_EXIT_NORMAL;
    }
    if let Err(error) = ignore_group_term() {
        let _ = stream.write_failure(&format!(
            "cannot install the invocation SIGTERM handler: {error}"
        ));
        return INNER_EXIT_NORMAL;
    }
    // The control channel is non-blocking so the loop can poll for the
    // TERMINATE request without blocking the reap loop.
    if let Err(error) = fcntl(std::io::stdin(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK)) {
        let _ = stream.write_failure(&format!(
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
        let _ = stream.write_failure(&format!(
            "cannot install the fixed process-group membership restriction: {error}"
        ));
        return INNER_EXIT_NORMAL;
    }
    if let Ok(path) = std::env::var(ANCHOR_PID_FILE_ENV) {
        let _ = std::fs::write(&path, std::process::id().to_string());
    }
    if std::env::var(FAIL_BASH_SPAWN_ENV).is_ok() {
        let _ = stream.write_failure("injected bash spawn failure");
        return INNER_EXIT_NORMAL;
    }
    let fail_wait = std::env::var(FAIL_WAIT_ENV).is_ok();
    let fail_signal = std::env::var(FAIL_SIGNAL_ENV).is_ok();
    let force_anchor_loss = std::env::var(FORCE_ANCHOR_LOSS_ENV).is_ok();
    let bash = match Command::new("/bin/bash")
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = stream.write_failure(&format!("cannot spawn /bin/bash: {error}"));
            return INNER_EXIT_NORMAL;
        }
    };
    // SAFETY-free pid capture: the pid is a positive `u32` from the kernel;
    // it is only compared against `waitpid` pids of the same conversion.
    let bash_pid = i32::try_from(bash.id()).unwrap_or(0);
    let self_pid = i32::try_from(std::process::id()).unwrap_or(0);
    let mut shell_reported = false;
    let mut read_buf: Vec<u8> = Vec::with_capacity(256);
    let mut kill_deadline: Option<std::time::Instant> = None;
    loop {
        // Reaping hygiene: everything that died in our child domain — the
        // shell and owned group members — is reaped here so no zombie is
        // ever left behind. This is deliberately NOT the settlement gate:
        // the group-scoped wait below decides settlement.
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
        match waitid(
            Id::PGid(Pid::from_raw(self_pid)),
            WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED,
        ) {
            Ok(WaitStatus::StillAlive | _) | Err(Errno::EINTR) => {}
            Err(Errno::ECHILD) => return INNER_EXIT_NORMAL,
            Err(error) => {
                let _ = stream.write_failure(&format!(
                    "cannot observe the owned group terminal state: {error}"
                ));
                return INNER_EXIT_CONTAINMENT;
            }
        }
        // Control frames from rustX (TERMINATE). The channel is
        // non-blocking; the poll cadence keeps this loop deterministic.
        let mut chunk = [0u8; 256];
        match read(std::io::stdin(), &mut chunk) {
            Ok(0) => {
                // rustX closed the control channel: the invocation is
                // abandoned and nobody reads our reports anymore. Owned
                // work may still be alive, so the exit signals the outer
                // supervisor to fail-safe-contain the invocation.
                return INNER_EXIT_CONTAINMENT;
            }
            Ok(control_read) => {
                read_buf.extend_from_slice(&chunk[..control_read]);
                if let Err(error) = handle_frames(
                    &mut read_buf,
                    &mut stream,
                    self_pid,
                    fail_signal,
                    force_anchor_loss,
                    &mut kill_deadline,
                ) {
                    let _ = stream.write_failure(&error);
                    return INNER_EXIT_CONTAINMENT;
                }
            }
            Err(Errno::EAGAIN) => {} // non-blocking; EWOULDBLOCK == EAGAIN on Linux
            Err(error) => {
                let _ = stream.write_failure(&format!("cannot read the control channel: {error}"));
                return INNER_EXIT_CONTAINMENT;
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

/// Parses complete control frames out of `buf` and handles them.
fn handle_frames(
    buf: &mut Vec<u8>,
    stream: &mut ControlStream,
    self_pid: i32,
    fail_signal: bool,
    force_anchor_loss: bool,
    kill_deadline: &mut Option<std::time::Instant>,
) -> Result<(), String> {
    loop {
        if buf.len() < 4 {
            return Ok(());
        }
        let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if buf.len() < 4 + len {
            return Ok(());
        }
        let kind = buf[4];
        let _payload = buf[5..4 + len].to_vec();
        buf.drain(..4 + len);
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

/// `PR_SET_CHILD_SUBREAPER`: orphaned descendants of the shell reparent
/// into this process's child domain instead of being rediscovered from
/// `/proc`.
///
/// This is one of the two narrowly scoped `libc` calls of the crate (the
/// other is the SIGTERM handler installation); everything else stays
/// unsafe-free. Linux-only: the lifecycle contract is claimed only where
/// the kernel provides the subreaper mechanism.
#[allow(unsafe_code)]
fn become_child_subreaper() -> Result<(), String> {
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
        Err("the invocation supervisor requires Linux (PR_SET_CHILD_SUBREAPER)".to_owned())
    }
}

/// The no-op `SIGTERM` handler of the inner supervisor: the invocation
/// group `TERM` must not kill the inner supervisor while bash and its
/// descendants handle it.
///
/// A **caught** handler (not `SIG_IGN`) is required: `exec` resets caught
/// dispositions to the default, so `/bin/bash` starts with a default
/// `SIGTERM` disposition and its own `trap '...' TERM` handlers stay
/// effective. An ignored signal would be inherited by bash, and a
/// non-interactive shell cannot re-trap a signal that was ignored on entry.
///
/// This is the second narrowly scoped `libc` call of the crate (besides
/// [`become_child_subreaper`]); the handled signal is delivered to the
/// process only by the invocation's own `killpg`, and the handler runs no
/// application code beyond a return.
extern "C" fn ignore_sigterm(_signal: libc::c_int) {}

#[allow(unsafe_code)]
fn ignore_group_term() -> Result<(), String> {
    // SAFETY: installing a no-op handler with no pointer payload is a
    // single scalar libc call; the handler never dereferences anything.
    let handler = ignore_sigterm as extern "C" fn(libc::c_int) as libc::sighandler_t;
    let result = unsafe { libc::signal(libc::SIGTERM, handler) };
    if result == libc::SIG_ERR {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

/// The supported Linux ABIs of the fixed-membership restriction. Building
/// for any other architecture is an explicit compile-time failure: the
/// contract is never silently weakened on unsupported systems.
#[cfg(not(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64"
)))]
compile_error!(
    "the fixed Bash process-group membership restriction supports only x86_64, aarch64, and riscv64 Linux"
);

/// Candidate syscall numbers that may implement `setpgid(2)` on the
/// supported ABIs: the classic x86-64 table (65), the kernel 7.0+
/// renumbered x86-64 table (109), and the asm-generic tables used by
/// aarch64/riscv64 (154). The real number of the running kernel is
/// **detected by side effect** (see [`probe_membership_syscall`]), never
/// guessed: a wrong candidate cannot match the verified membership side
/// effect.
#[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64"
))]
const SETPGID_CANDIDATES: &[i64] = &[65, 109, 154];

/// Candidate syscall numbers that may implement `setsid(2)`: the classic
/// and renumbered x86-64 tables (112), the legacy 32-bit x86 table (66),
/// and the asm-generic tables (157, 147). Same runtime side-effect
/// detection as [`SETPGID_CANDIDATES`].
#[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64"
))]
const SETSID_CANDIDATES: &[i64] = &[112, 66, 157, 147];

/// The `AUDIT_ARCH` constant of the compiled architecture, used by the
/// seccomp filter to reject syscalls from an unexpected ABI before any
/// other check.
#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xC000_003E; // AUDIT_ARCH_X86_64
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xC000_00B7; // AUDIT_ARCH_AARCH64
#[cfg(target_arch = "riscv64")]
const AUDIT_ARCH: u32 = 0xC000_00F3; // AUDIT_ARCH_RISCV64

/// One membership syscall kind whose number is detected at runtime.
#[derive(Clone, Copy)]
enum MembershipSyscall {
    Setpgid,
    Setsid,
}

/// Detects the running kernel's syscall number for one membership syscall:
/// each candidate is executed in a throwaway child and the child reports
/// whether the call produced the real membership side effect. This works
/// across kernel ABI renumberings (the Linux x86-64 syscall numbers were
/// renumbered in kernel 7.0) with no version probing.
#[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64"
))]
fn detect_membership_syscall(kind: MembershipSyscall) -> Result<i64, String> {
    let candidates: &[i64] = match kind {
        MembershipSyscall::Setpgid => SETPGID_CANDIDATES,
        MembershipSyscall::Setsid => SETSID_CANDIDATES,
    };
    for &candidate in candidates {
        if probe_membership_syscall(kind, candidate)? {
            return Ok(candidate);
        }
    }
    Err("cannot identify the kernel's setpgid/setsid syscall number".to_owned())
}

/// Runs one candidate syscall in a throwaway child and verifies the actual
/// membership side effect:
///
/// - `setpgid(0, 0)` succeeds only when the child's process group becomes
///   its own pid **while its session is unchanged** (this excludes a real
///   `setsid` and every unrelated syscall);
/// - `setsid()` succeeds only when the child becomes its own session
///   leader (`getsid(0) == pid`).
///
/// The child writes one byte (`1` = verified) and exits; the parent reaps
/// it, so no zombie or orphan is ever left behind. The probe runs before
/// the invocation group exists, so its transient side effects never touch
/// owned work.
#[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64"
))]
#[allow(unsafe_code)]
fn probe_membership_syscall(kind: MembershipSyscall, number: i64) -> Result<bool, String> {
    use std::os::fd::AsRawFd;

    use nix::unistd::{getpgrp, getpid, getsid};

    let (read_fd, write_fd) = nix::unistd::pipe()
        .map_err(|error| format!("cannot create the membership probe pipe: {error}"))?;
    // SAFETY: fork(2); the child runs exactly one probe syscall and _exit.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(format!(
            "cannot fork the membership probe child: {}",
            std::io::Error::last_os_error()
        ));
    }
    if pid == 0 {
        let verified = match kind {
            MembershipSyscall::Setpgid => {
                let before_pgrp = getpgrp();
                let before_sid = getsid(None).ok();
                // SAFETY: one syscall with two scalar pid arguments.
                let result = unsafe { libc::syscall(number, 0, 0) };
                let after_pgrp = getpgrp();
                result == 0
                    && after_pgrp == getpid()
                    && after_pgrp != before_pgrp
                    && getsid(None).ok() == before_sid
            }
            MembershipSyscall::Setsid => {
                // SAFETY: one syscall with no arguments.
                let result = unsafe { libc::syscall(number) };
                let self_pid = getpid();
                result > 0
                    && result == i64::from(self_pid.as_raw())
                    && getsid(None).ok() == Some(self_pid)
            }
        };
        let byte = [u8::from(verified)];
        // SAFETY: write one byte to the pipe end the parent reads.
        unsafe { libc::write(write_fd.as_raw_fd(), byte.as_ptr().cast(), 1) };
        // SAFETY: _exit terminates the probe child without touching stdio.
        unsafe { libc::_exit(0) };
    }
    drop(write_fd);
    let mut byte = [0u8; 1];
    let read = nix::unistd::read(&read_fd, &mut byte)
        .map_err(|error| format!("cannot read the membership probe pipe: {error}"))?;
    drop(read_fd);
    // Reap the probe child deterministically (it exits immediately);
    // EINTR just retries the reap.
    loop {
        match nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), None) {
            Ok(_) => break,
            Err(Errno::EINTR) => {}
            Err(error) => {
                return Err(format!("cannot reap the membership probe child: {error}"));
            }
        }
    }
    Ok(read == 1 && byte[0] == 1)
}

/// The `sock_filter` BPF program of the fixed-membership restriction:
///
/// ```text
/// 0: load seccomp_data.arch
/// 1: if arch == the compiled AUDIT_ARCH -> 2, else -> 7 (kill, fail closed)
/// 2: load seccomp_data.nr
/// 3: if nr == setpgid -> 6 (EPERM), else -> 4
/// 4: if nr == setsid -> 6 (EPERM), else -> 5
/// 5: allow
/// 6: return EPERM
/// 7: kill (foreign ABI)
/// ```
///
/// The `u32 -> u16` instruction-code casts and the `i64 -> u32` syscall
/// number casts are safe by construction: the BPF opcode constants are
/// designed to fit `u16`, and the detected syscall numbers always fit
/// `u32`.
#[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64"
))]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // BPF opcodes and syscall numbers fit their fields
fn membership_restriction_program(setpgid: i64, setsid: i64) -> [libc::sock_filter; 8] {
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
            k: setpgid as u32,
        },
        libc::sock_filter {
            code: (BPF_JMP | BPF_JEQ | BPF_K) as u16,
            jt: 1,
            jf: 0,
            k: setsid as u32,
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

/// Installs the fixed-membership restriction: `PR_SET_NO_NEW_PRIVS` plus a
/// `seccomp` filter that rejects `setpgid(2)` and `setsid(2)` with
/// `EPERM`. The filter is inherited by `/bin/bash` and every descendant
/// across `fork`/`exec`; with `no_new_privs` set, a descendant can only
/// stack *more* restrictive filters, never remove this one, and no
/// privilege gain can bypass it. This is the third narrowly scoped `libc`
/// call site of the crate (besides [`become_child_subreaper`] and
/// [`ignore_group_term`]).
#[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64"
))]
#[allow(unsafe_code)]
fn enforce_fixed_group_membership() -> Result<(), String> {
    let setpgid = detect_membership_syscall(MembershipSyscall::Setpgid)?;
    let setsid = detect_membership_syscall(MembershipSyscall::Setsid)?;
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
    let program = membership_restriction_program(setpgid, setsid);
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
