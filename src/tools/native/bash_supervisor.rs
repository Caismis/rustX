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
//! settlement on exactly the processes that remain in that group. A
//! descendant that explicitly leaves the group/session (for example via
//! `setsid` or `setpgid`) has intentionally escaped the Bash execution
//! domain: it is no longer part of the tool's owned lifecycle, it is not
//! signaled by the group `TERM`/`KILL`, and it must never block terminal
//! settlement. Subreaper adoption is a **reaping implementation detail**:
//! adopted children outside the group are reaped for hygiene when they
//! die, and handed to init when the supervisor exits, but they never
//! expand semantic ownership beyond the process-group boundary.
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
//! matches only children in the invocation process group:
//!
//! - the inner supervisor's group-scoped wait returning `ECHILD` — the
//!   shell and every owned member are reaped; adopted children that left
//!   the group never match — it then exits with [`INNER_EXIT_NORMAL`];
//! - the outer supervisor's group-scoped wait returning `ECHILD` — no
//!   child of the outer remains in the invocation group (the inner anchor
//!   itself is a member and is released by this same wait, strictly after
//!   any fallback containment signal); it then reports
//!   [`MSG_ALL_CHILDREN_REAPED`] and exits.
//!
//! This is the exact kernel-mediated terminal linearization point of the
//! owned process group. It is not a `/proc` scan, not a `killpg(0)` probe
//! (an un-reaped leader zombie keeps the group observable, so probes
//! cannot distinguish live members), and not a timing observation: a live
//! group member is a matching child and keeps the group-scoped wait from
//! returning `ECHILD`, while an escaped child is not in the group and can
//! never block it.
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
//! The subreaper mechanism is Linux-specific (`PR_SET_CHILD_SUBREAPER`).
//! The crate already requires `/bin/bash`; the lifecycle contract is
//! claimed only on Linux. On other Unix platforms the supervisor reports an
//! explicit setup failure, which settles the invocation as `Failed` — the
//! same contract is never silently weakened.

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
/// the outer remains in the invocation process group — adopted children
/// that left the group/session (escaped descendants) never match and can
/// never block settlement.
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
        }
        if inner_terminal {
            // The owned-group gate: the kernel-mediated terminal condition
            // of the invocation process group at the outer level. No child
            // of ours remains in the invocation group once this returns
            // ECHILD — the inner anchor itself is a member and is released
            // (reaped) by this group-scoped wait, which happens strictly
            // after any fallback containment signal. Adopted children that
            // left the group/session (escaped descendants) never match and
            // never block the canonical terminal report.
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
        // (adopted escaped descendants, members not yet reaped by the
        // gate) is reaped so no zombie is left behind. This is never the
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
/// `ECHILD` there means the shell and every owned member are reaped, while
/// adopted children that left the group/session never match and never
/// block the invocation.
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
        // shell, owned group members, and adopted children that left the
        // group (escaped descendants) — is reaped here so no zombie is
        // ever left behind. This is deliberately NOT the settlement gate:
        // adopted children outside the invocation group must not keep the
        // invocation alive.
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
        // the invocation process group. Only children inside the
        // invocation group (bash and the owned members) match this
        // group-scoped wait; adopted children that explicitly left the
        // group/session never match and can never block settlement.
        // ECHILD means no child of ours remains in the owned group — the
        // group's child domain is terminal. This is the exact structural
        // point; it is not a probe, a scan, or a timing observation.
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
