//! The long-lived interactive supervisor unit for MCP stdio servers.
//!
//! # Structure and ownership (M5-equivalent)
//!
//! ```text
//! rustX
//!   └─ interactive outer supervisor (rustX child; subreaper; reaper of
//!      last resort; frame relay between rustX and the inner)
//!        └─ interactive inner (outer child; setsid -> session/group leader;
//!                             subreaper; server parent; orphan reaper;
//!                             IPC peer)
//!             └─ MCP stdio server
//!                  └─ descendants
//! ```
//!
//! The unit is the M5 Bash supervisor shape applied to a long-lived server:
//!
//! - the inner calls `setsid()`: it becomes the leader of a fresh session
//!   and of the session's first process group, whose numeric id is the
//!   inner's own pid. The server is spawned without a new process group, so
//!   the server and every descendant that stays in the group live in that
//!   one unit-owned group;
//! - the inner installs the shared fixed-membership restriction
//!   (`supervised_unit::enforce_fixed_group_membership`)
//!   before the server spawn, so `setsid(2)`/`setpgid(2)` escape attempts
//!   fail deterministically with `EPERM` for the server and every
//!   descendant;
//! - `TERM` -> grace -> `KILL` is issued by the inner with `killpg` against
//!   **its own process group**, whose numeric id is its own pid — provably
//!   allocated exactly while the inner lives, so no foreign process group
//!   can ever receive the unit's numeric group id while signaling is legal;
//! - the kernel-mediated terminal proof is the group-scoped wait
//!   (`waitid(Id::PGid)` returning `ECHILD`) at the inner (normal) and at
//!   the outer (release gate), never a `/proc` scan and never a
//!   `killpg(..., 0)` probe;
//! - the inner supervisor pid is the unit's structural ownership anchor
//!   with exactly one reaping owner: the outer's dedicated observation
//!   (`WNOWAIT`) and the outer's group-scoped release. The outer reports
//!   the authoritative terminal event only after its gate reaches `ECHILD`,
//!   and only after it released the anchor;
//! - when the inner terminates abnormally with possibly-live owned work
//!   (`INNER_EXIT_CONTAINMENT`), the outer issues the one fallback
//!   containment `SIGKILL` while the anchor is still retained, then
//!   releases the anchor through the gate;
//! - when the outer itself is lost, rustX (already the child-subreaper
//!   prerequisite) runs the shared adopted-anchor emergency containment;
//! - supervisor control traffic uses private Unix sockets, fully separate
//!   from the server's business stdin/stdout protocol pair.
//!
//! The binary is dispatched by argv role (`outer`/`inner`), mirroring the
//! Bash supervisor's `RUSTX_SUPERVISOR_ROLE` dispatch.

use std::process::{Command, Stdio};
use std::time::Instant;

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::signal::Signal;
use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid, waitpid};
use nix::unistd::Pid;

use crate::runtime::supervised_unit::{
    FrameReader, INNER_EXIT_CONTAINMENT, INNER_EXIT_NORMAL, MSG_ALL_CHILDREN_REAPED,
    MSG_ANCHOR_READY, MSG_NO_OWNERSHIP, MSG_OWNER_ATTACHED, MSG_OWNERSHIP_ESTABLISHED,
    MSG_PROCESS_CONTROL_FAILURE, MSG_SIGNAL_ATTEMPT, MSG_START, MSG_TERMINAL_ACK, MSG_TERMINATE,
    POLL_INTERVAL, TERM_GRACE, TERMINAL_ACK_TIMEOUT, become_child_subreaper,
    enforce_fixed_group_membership, ignore_group_term, signal_group, write_frame,
};

/// The environment variable naming the rustX-facing control socket.
pub(crate) const RUSTX_CONTROL_ENV: &str = "RUSTX_INTERACTIVE_CONTROL";
/// The environment variable naming the inner-facing control socket.
pub(crate) const INNER_CONTROL_ENV: &str = "RUSTX_INTERACTIVE_INNER_CONTROL";

/// Test-only injection: the outer supervisor exits before connecting to
/// rustX (the driver's accept-vs-wait handshake regression).
pub(crate) const OUTER_FAIL_ENV: &str = "RUSTX_TEST_INTERACTIVE_OUTER_FAIL";
/// Test-only injection: the inner supervisor fails the server spawn.
pub(crate) const FAIL_SERVER_SPAWN_ENV: &str = "RUSTX_TEST_INTERACTIVE_FAIL_SERVER_SPAWN";
/// Test-only injection: the inner supervisor refuses every group signal.
pub(crate) const FAIL_SIGNAL_ENV: &str = "RUSTX_TEST_INTERACTIVE_FAIL_SIGNAL";
/// Test-only injection: the inner supervisor fails its SIGTERM handler
/// installation (a pre-ownership setup failure).
pub(crate) const FAIL_SIGTERM_HANDLER_ENV: &str = "RUSTX_TEST_INTERACTIVE_FAIL_SIGTERM";
/// Test-only observability: the inner supervisor writes its own pid (the
/// unit's process-group id) to this file.
pub(crate) const ANCHOR_PID_FILE_ENV: &str = "RUSTX_INTERACTIVE_ANCHOR_PID_FILE";
/// Test-only injection: the inner supervisor exits before connecting its
/// control socket. The value names the pid file the inner writes first, so
/// the regression can prove the pre-ownership inner was reaped.
pub(crate) const INNER_EXIT_BEFORE_CONNECT_ENV: &str =
    "RUSTX_TEST_INTERACTIVE_INNER_EXIT_BEFORE_CONNECT";
/// The inner supervisor's exit status for the test-only
/// exits-before-connecting injection.
const INNER_EXIT_BEFORE_CONNECT_STATUS: i32 = 43;

/// Runs the outer supervisor role; returns its exit status.
#[must_use]
#[allow(clippy::too_many_lines)] // one coherent outer supervise/relay/contain pipeline
pub fn run_outer(arguments: &[String]) -> i32 {
    let Some(socket) = std::env::var_os(RUSTX_CONTROL_ENV) else {
        eprintln!("interactive supervisor: control socket path is missing");
        return 1;
    };
    if std::env::var(OUTER_FAIL_ENV).is_ok() {
        // Test-only injection: the outer dies before connecting, so the
        // rustX driver must settle the unit instead of stranding it.
        return 42;
    }
    let mut upstream = match std::os::unix::net::UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("interactive supervisor: cannot connect control socket: {error}");
            return 1;
        }
    };
    if let Err(error) = fcntl(&upstream, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)) {
        eprintln!("interactive supervisor: cannot configure the control socket: {error}");
        return 1;
    }
    // The runtime->outer startup gate. rustX writes `MSG_OWNER_ATTACHED`
    // only after it accepted and retained this control connection, so no
    // part of the unit hierarchy — not the inner, not the server — can be
    // created while rustX might still fail its accept. Nothing is owned
    // before this returns.
    match await_owner_attached(&mut upstream) {
        Ok(true) => {}
        // rustX disappeared or requested shutdown at the gate: nothing was
        // ever created, so exiting is the complete settlement.
        Ok(false) => return 0,
        Err(error) => {
            eprintln!("interactive supervisor: {error}");
            return 1;
        }
    }
    let inner_socket = std::env::temp_dir().join(format!(
        "rustx-interactive-inner-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let inner_listener = match std::os::unix::net::UnixListener::bind(&inner_socket) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("interactive supervisor: cannot bind the inner control socket: {error}");
            return 1;
        }
    };
    // The pre-ownership acceptance is a bounded ownership state machine, not
    // a blocking one-shot accept: the inner control connection, the inner's
    // own exit, and upstream termination/control loss are all polled.
    if let Err(error) = inner_listener.set_nonblocking(true) {
        eprintln!("interactive supervisor: cannot configure the inner control socket: {error}");
        let _ = std::fs::remove_file(&inner_socket);
        return 1;
    }
    if let Err(error) = become_child_subreaper() {
        let _ = write_frame(
            &mut upstream,
            MSG_PROCESS_CONTROL_FAILURE,
            error.clone().as_bytes(),
        );
        let _ = write_frame(&mut upstream, MSG_NO_OWNERSHIP, &[]);
        return 0;
    }
    let current_exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("interactive supervisor: cannot locate itself: {error}");
            return 1;
        }
    };
    let mut inner = Command::new(current_exe);
    inner.arg("inner").args(arguments);
    inner.env(INNER_CONTROL_ENV, &inner_socket);
    inner
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = match inner.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = std::fs::remove_file(&inner_socket);
            let _ = write_frame(
                &mut upstream,
                MSG_PROCESS_CONTROL_FAILURE,
                error.to_string().as_bytes(),
            );
            let _ = write_frame(&mut upstream, MSG_NO_OWNERSHIP, &[]);
            return 0;
        }
    };
    let mut downstream = match attach_inner_control(&mut child, &inner_listener, &mut upstream) {
        InnerAttachment::Attached(downstream) => downstream,
        // The pre-ownership state machine already reaped the inner and
        // reported the outcome: no server-owned process tree ever escaped
        // the pre-ownership state.
        InnerAttachment::Concluded => {
            let _ = std::fs::remove_file(&inner_socket);
            return 0;
        }
    };
    let _ = std::fs::remove_file(&inner_socket);
    if let Err(error) = fcntl(&downstream, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = write_frame(
            &mut upstream,
            MSG_PROCESS_CONTROL_FAILURE,
            error.to_string().as_bytes(),
        );
        let _ = write_frame(&mut upstream, MSG_NO_OWNERSHIP, &[]);
        return 0;
    }
    // The inner supervisor's pid is the unit's process-group id and its
    // structural ownership anchor.
    let inner_pid = i32::try_from(child.id()).expect("the inner supervisor pid fits i32");
    let mut anchor = AnchorState::Running;
    let mut anchor_loss_reported = false;
    let mut upstream_reader = FrameReader::new();
    let mut downstream_reader = FrameReader::new();
    let mut terminal_reported = false;
    let mut ack_seen = false;
    let mut ack_deadline: Option<Instant> = None;
    loop {
        match anchor {
            AnchorState::Running => {
                match waitid(
                    Id::Pid(Pid::from_raw(inner_pid)),
                    WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT,
                ) {
                    Ok(WaitStatus::StillAlive) => {}
                    Ok(WaitStatus::Stopped(..) | WaitStatus::Continued(_)) => {}
                    #[cfg(target_os = "linux")]
                    Ok(WaitStatus::PtraceEvent(..) | WaitStatus::PtraceSyscall(_)) => {}
                    Ok(WaitStatus::Exited(_, code)) => {
                        anchor = AnchorState::TerminalRetained;
                        if code != INNER_EXIT_NORMAL {
                            // Abnormal termination with possibly-live owned
                            // work: active containment while the anchor is
                            // still held (observed but un-reaped).
                            containment_signal(&mut upstream, inner_pid);
                        }
                    }
                    Ok(WaitStatus::Signaled(..)) => {
                        anchor = AnchorState::TerminalRetained;
                        containment_signal(&mut upstream, inner_pid);
                    }
                    Err(Errno::EINTR) => {}
                    Err(Errno::ECHILD) => {
                        // The anchor is not a waitable child before its
                        // intentional release: an ownership invariant
                        // violation, never a terminal observation. The unit
                        // fails safely: no signal on an unproven numeric id,
                        // no canonical terminal event.
                        anchor = AnchorState::UnexpectedlyLost;
                        if !anchor_loss_reported {
                            anchor_loss_reported = true;
                            let _ = write_frame(
                                &mut upstream,
                                MSG_PROCESS_CONTROL_FAILURE,
                                "the unit anchor became unwaitable before its intentional \
                                 release; owned-group terminality can no longer be proven"
                                    .as_bytes(),
                            );
                        }
                    }
                    Err(error) => {
                        let _ = write_frame(
                            &mut upstream,
                            MSG_PROCESS_CONTROL_FAILURE,
                            format!("cannot observe the unit anchor: {error}").as_bytes(),
                        );
                    }
                }
                if anchor == AnchorState::Running {
                    match waitid(
                        Id::Pid(Pid::from_raw(inner_pid)),
                        WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED,
                    ) {
                        Ok(WaitStatus::Stopped(..)) => {
                            let _ =
                                nix::sys::signal::kill(Pid::from_raw(inner_pid), Signal::SIGKILL);
                        }
                        Ok(WaitStatus::StillAlive | _) | Err(Errno::EINTR | Errno::ECHILD) => {}
                        Err(error) => {
                            let _ = write_frame(
                                &mut upstream,
                                MSG_PROCESS_CONTROL_FAILURE,
                                format!("cannot observe the unit anchor freeze state: {error}")
                                    .as_bytes(),
                            );
                        }
                    }
                }
            }
            AnchorState::TerminalRetained => {
                match waitid(
                    Id::PGid(Pid::from_raw(inner_pid)),
                    WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED,
                ) {
                    Ok(WaitStatus::StillAlive | _) | Err(Errno::EINTR) => {}
                    Err(Errno::ECHILD) => {
                        // The owned-group gate: no child of ours remains in
                        // the unit group — the anchor itself was released by
                        // this same wait, strictly after any fallback
                        // containment signal. This is the authoritative
                        // terminal event.
                        if !terminal_reported {
                            terminal_reported = true;
                            if write_frame(&mut upstream, MSG_ALL_CHILDREN_REAPED, &[]).is_ok() {
                                ack_deadline = Some(Instant::now() + TERMINAL_ACK_TIMEOUT);
                            }
                        }
                    }
                    Err(error) => {
                        let _ = write_frame(
                            &mut upstream,
                            MSG_PROCESS_CONTROL_FAILURE,
                            format!("cannot observe the owned group terminal state: {error}")
                                .as_bytes(),
                        );
                    }
                }
            }
            AnchorState::UnexpectedlyLost => {
                // Fail-safe: never signal the unproven numeric id and never
                // report the canonical terminal event.
            }
        }
        if terminal_reported && ack_seen {
            return 0;
        }
        if terminal_reported && ack_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return 0;
        }
        // Upstream frames (rustX): TERMINATE is relayed to the inner;
        // TERMINAL_ACK completes the terminal exchange; EOF means rustX
        // disappeared — the unit still runs its TERM sequence so the owned
        // group cannot outlive the owner.
        {
            let mut chunk = [0u8; 256];
            loop {
                match nix::unistd::read(&upstream, &mut chunk) {
                    Ok(0) => {
                        if !terminal_reported {
                            let _ = write_frame(&mut downstream, MSG_TERMINATE, &[]);
                        }
                        ack_seen = true;
                        break;
                    }
                    Ok(count) => {
                        if let Some((kind, _payload)) = upstream_reader.push(&chunk[..count]) {
                            handle_upstream_frame(
                                kind,
                                &mut downstream,
                                &mut upstream,
                                &mut ack_seen,
                            );
                        }
                        while let Some((kind, _payload)) = upstream_reader.pop() {
                            handle_upstream_frame(
                                kind,
                                &mut downstream,
                                &mut upstream,
                                &mut ack_seen,
                            );
                        }
                        if ack_seen {
                            break;
                        }
                    }
                    Err(Errno::EAGAIN | Errno::EINTR) => break,
                    Err(error) => {
                        let _ = write_frame(
                            &mut upstream,
                            MSG_PROCESS_CONTROL_FAILURE,
                            format!("cannot read the rustX control channel: {error}").as_bytes(),
                        );
                        break;
                    }
                }
            }
        }
        // Downstream frames (inner): reports are relayed upstream.
        {
            let mut chunk = [0u8; 256];
            loop {
                match nix::unistd::read(&downstream, &mut chunk) {
                    Ok(0) | Err(Errno::EAGAIN | Errno::EINTR) => break,
                    Ok(count) => {
                        if let Some((kind, payload)) = downstream_reader.push(&chunk[..count]) {
                            let _ = write_frame(&mut upstream, kind, &payload);
                        }
                        while let Some((kind, payload)) = downstream_reader.pop() {
                            let _ = write_frame(&mut upstream, kind, &payload);
                        }
                    }
                    Err(error) => {
                        let _ = write_frame(
                            &mut upstream,
                            MSG_PROCESS_CONTROL_FAILURE,
                            format!("cannot read the inner control channel: {error}").as_bytes(),
                        );
                        break;
                    }
                }
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Waits at the runtime->outer startup gate.
///
/// `Ok(true)` means rustX accepted and retained this control connection, so
/// the unit hierarchy may be created. `Ok(false)` means rustX disappeared
/// or requested shutdown before the gate opened — nothing was created, so
/// there is nothing to contain.
fn await_owner_attached(upstream: &mut std::os::unix::net::UnixStream) -> Result<bool, String> {
    let mut reader = FrameReader::new();
    loop {
        let mut chunk = [0u8; 256];
        match nix::unistd::read(&*upstream, &mut chunk) {
            Ok(0) => return Ok(false),
            Ok(count) => {
                if let Some((kind, _payload)) = reader.push(&chunk[..count]) {
                    return handle_startup_gate_frame(kind);
                }
                if let Some((kind, _payload)) = reader.pop() {
                    return handle_startup_gate_frame(kind);
                }
            }
            Err(Errno::EAGAIN | Errno::EINTR) => std::thread::sleep(POLL_INTERVAL),
            Err(error) => return Err(format!("cannot read the startup gate: {error}")),
        }
    }
}

/// Handles one runtime->outer startup gate frame.
fn handle_startup_gate_frame(kind: u8) -> Result<bool, String> {
    match kind {
        MSG_OWNER_ATTACHED => Ok(true),
        MSG_TERMINATE => Ok(false),
        other => Err(format!(
            "unexpected startup gate control message {other:#04x}"
        )),
    }
}

/// The outcome of the outer's pre-ownership inner-attachment state machine.
enum InnerAttachment {
    /// The inner control connection was established.
    Attached(std::os::unix::net::UnixStream),
    /// The unit ended before ownership could exist. The pre-ownership inner
    /// was reaped and the outcome was reported upstream; the outer exits.
    Concluded,
}

/// Attaches the inner control connection with a bounded ownership state
/// machine.
///
/// A blocking one-shot `accept()` would hang forever if the inner exited
/// before connecting, so all three pre-ownership transitions are polled
/// with the supervisor's existing polling model:
///
/// - the inner control connection is established;
/// - the inner direct child exits before connecting (reap it, report the
///   process-control failure and no-ownership);
/// - rustX disappears or requests shutdown (terminate and reap the
///   pre-ownership inner, report no-ownership).
///
/// The inner spawns the server only after the `START` gate, which can only
/// be relayed over the connection established here, so no server-owned
/// process tree can escape the pre-ownership state.
fn attach_inner_control(
    child: &mut std::process::Child,
    inner_listener: &std::os::unix::net::UnixListener,
    upstream: &mut std::os::unix::net::UnixStream,
) -> InnerAttachment {
    let mut reader = FrameReader::new();
    loop {
        match inner_listener.accept() {
            Ok((downstream, _)) => return InnerAttachment::Attached(downstream),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(error) => {
                return conclude_pre_ownership(
                    child,
                    upstream,
                    Some(&format!(
                        "cannot accept the inner control connection: {error}"
                    )),
                );
            }
        }
        // The inner direct child may have exited before connecting. The
        // server spawn is gated behind the control connection, so its exit
        // here provably leaves no owned process tree; `try_wait` reaps it.
        match child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                let _ = write_frame(
                    upstream,
                    MSG_PROCESS_CONTROL_FAILURE,
                    format!(
                        "the inner supervisor exited before connecting its control channel: \
                         {status}"
                    )
                    .as_bytes(),
                );
                let _ = write_frame(upstream, MSG_NO_OWNERSHIP, &[]);
                return InnerAttachment::Concluded;
            }
            Err(error) => {
                return conclude_pre_ownership(
                    child,
                    upstream,
                    Some(&format!(
                        "cannot observe the pre-ownership inner supervisor: {error}"
                    )),
                );
            }
        }
        // rustX termination requests and control loss before ownership.
        let mut chunk = [0u8; 256];
        match nix::unistd::read(&*upstream, &mut chunk) {
            Ok(0) => return conclude_pre_ownership(child, upstream, None),
            Ok(count) => {
                let mut terminate =
                    matches!(reader.push(&chunk[..count]), Some((MSG_TERMINATE, _)));
                while let Some((kind, _payload)) = reader.pop() {
                    terminate |= kind == MSG_TERMINATE;
                }
                if terminate {
                    return conclude_pre_ownership(child, upstream, None);
                }
            }
            Err(Errno::EAGAIN | Errno::EINTR) => {}
            Err(error) => {
                return conclude_pre_ownership(
                    child,
                    upstream,
                    Some(&format!("cannot read the rustX control channel: {error}")),
                );
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Terminates and reaps the pre-ownership inner, then reports no-ownership.
///
/// The inner cannot have spawned the server before its control connection
/// carried the `START` gate, so terminating its pid is the complete
/// physical containment of the unit at this stage.
fn conclude_pre_ownership(
    child: &mut std::process::Child,
    upstream: &mut std::os::unix::net::UnixStream,
    failure: Option<&str>,
) -> InnerAttachment {
    let _ = child.kill();
    let reaped = child.wait();
    if let Some(message) = failure {
        let _ = write_frame(upstream, MSG_PROCESS_CONTROL_FAILURE, message.as_bytes());
    }
    if let Err(error) = reaped {
        let _ = write_frame(
            upstream,
            MSG_PROCESS_CONTROL_FAILURE,
            format!("cannot reap the pre-ownership inner supervisor: {error}").as_bytes(),
        );
    }
    let _ = write_frame(upstream, MSG_NO_OWNERSHIP, &[]);
    InnerAttachment::Concluded
}

/// Handles one upstream control frame (from rustX): START and TERMINATE
/// are relayed to the inner; `TERMINAL_ACK` completes the terminal exchange.
fn handle_upstream_frame(
    kind: u8,
    downstream: &mut std::os::unix::net::UnixStream,
    upstream: &mut std::os::unix::net::UnixStream,
    ack_seen: &mut bool,
) {
    match kind {
        MSG_TERMINATE | MSG_START => {
            let _ = write_frame(downstream, kind, &[]);
        }
        MSG_TERMINAL_ACK => {
            *ack_seen = true;
        }
        other => {
            let _ = write_frame(
                upstream,
                MSG_PROCESS_CONTROL_FAILURE,
                format!("unknown upstream control kind {other:#04x}").as_bytes(),
            );
        }
    }
}

/// Handles one control frame of the inner loop: TERMINATE starts the
/// `TERM` -> grace -> `KILL` sequence; anything else is a control failure.
fn handle_inner_control_frame(
    kind: u8,
    control: &mut std::os::unix::net::UnixStream,
    kill_deadline: &mut Option<Instant>,
    fail_signal: bool,
) -> Result<(), String> {
    match kind {
        MSG_TERMINATE => {
            if fail_signal {
                let _ = write_frame(
                    control,
                    MSG_PROCESS_CONTROL_FAILURE,
                    b"injected signaling failure",
                );
                return Err("injected signaling failure".to_owned());
            }
            signal_group(
                i32::try_from(std::process::id()).unwrap_or(0),
                Signal::SIGTERM,
            )?;
            if kill_deadline.is_none() {
                *kill_deadline = Some(Instant::now() + TERM_GRACE);
            }
            Ok(())
        }
        other => Err(format!("unknown control message kind {other:#04x}")),
    }
}

/// The outer supervisor's ownership state of the inner anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnchorState {
    /// The anchor is alive (or not yet observed terminal) and retained.
    Running,
    /// The anchor's terminal state was observed with `WNOWAIT` (identity
    /// not consumed) and any fallback containment signal was already issued
    /// while the identity was retained; the group-scoped gate now owns the
    /// release.
    TerminalRetained,
    /// The anchor is not waitable before its intentional release: an
    /// ownership invariant violation. The unit fails safely.
    UnexpectedlyLost,
}

/// The outer supervisor's active containment: one final `SIGKILL` to the
/// unit's process group, issued only while the structural anchor is held.
fn containment_signal(upstream: &mut std::os::unix::net::UnixStream, pgid: i32) {
    let mut payload = Vec::with_capacity(9);
    payload.extend_from_slice(&pgid.to_le_bytes());
    payload.extend_from_slice(&(Signal::SIGKILL as i32).to_le_bytes());
    payload.push(1);
    let _ = write_frame(upstream, MSG_SIGNAL_ATTEMPT, &payload);
    if let Err(error) = signal_group(pgid, Signal::SIGKILL) {
        let _ = write_frame(
            upstream,
            MSG_PROCESS_CONTROL_FAILURE,
            format!("cannot contain the owned unit group: {error}").as_bytes(),
        );
    }
}

/// Runs the inner supervisor role; returns its exit status.
#[must_use]
#[allow(clippy::too_many_lines)] // one coherent inner session/spawn/reap pipeline
pub fn run_inner(arguments: &[String]) -> i32 {
    let Some(inner_socket) = std::env::var_os(INNER_CONTROL_ENV) else {
        eprintln!("interactive supervisor: inner control socket path is missing");
        return 1;
    };
    if let Ok(pid_file) = std::env::var(INNER_EXIT_BEFORE_CONNECT_ENV) {
        // Test-only injection: the inner exits before its control
        // connection exists. The outer's pre-ownership state machine must
        // reap it and report no-ownership instead of blocking forever on a
        // connection that can never arrive.
        let _ = std::fs::write(&pid_file, std::process::id().to_string());
        return INNER_EXIT_BEFORE_CONNECT_STATUS;
    }
    let mut control = match std::os::unix::net::UnixStream::connect(&inner_socket) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("interactive supervisor: cannot connect the inner control socket: {error}");
            return 1;
        }
    };
    if let Err(error) = fcntl(&control, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)) {
        eprintln!("interactive supervisor: cannot configure the inner control socket: {error}");
        return 1;
    }
    let Some(program) = arguments.first() else {
        eprintln!("interactive supervisor: server program is missing");
        return 1;
    };
    let fail_signal = std::env::var(FAIL_SIGNAL_ENV).is_ok();
    if let Err(error) = nix::unistd::setsid() {
        let _ = write_frame(
            &mut control,
            MSG_PROCESS_CONTROL_FAILURE,
            error.to_string().as_bytes(),
        );
        let _ = write_frame(&mut control, MSG_NO_OWNERSHIP, &[]);
        return INNER_EXIT_NORMAL;
    }
    if let Err(error) = become_child_subreaper() {
        let _ = write_frame(
            &mut control,
            MSG_PROCESS_CONTROL_FAILURE,
            error.clone().as_bytes(),
        );
        let _ = write_frame(&mut control, MSG_NO_OWNERSHIP, &[]);
        return INNER_EXIT_NORMAL;
    }
    if std::env::var(FAIL_SIGTERM_HANDLER_ENV).is_ok() {
        let _ = write_frame(
            &mut control,
            MSG_PROCESS_CONTROL_FAILURE,
            b"injected SIGTERM handler installation failure",
        );
        let _ = write_frame(&mut control, MSG_NO_OWNERSHIP, &[]);
        return INNER_EXIT_NORMAL;
    }
    if let Err(error) = ignore_group_term() {
        let _ = write_frame(
            &mut control,
            MSG_PROCESS_CONTROL_FAILURE,
            error.clone().as_bytes(),
        );
        let _ = write_frame(&mut control, MSG_NO_OWNERSHIP, &[]);
        return INNER_EXIT_NORMAL;
    }
    // The fixed-membership restriction: from this point on, this process,
    // the server, and every descendant are structurally prevented from
    // changing process-group/session membership (setsid/setpgid are
    // rejected). An install failure is a pre-ownership setup failure.
    if let Err(error) = enforce_fixed_group_membership() {
        let _ = write_frame(
            &mut control,
            MSG_PROCESS_CONTROL_FAILURE,
            error.clone().as_bytes(),
        );
        let _ = write_frame(&mut control, MSG_NO_OWNERSHIP, &[]);
        return INNER_EXIT_NORMAL;
    }
    if let Ok(path) = std::env::var(ANCHOR_PID_FILE_ENV) {
        let _ = std::fs::write(&path, std::process::id().to_string());
    }
    let self_pid = i32::try_from(std::process::id()).unwrap_or(0);
    if write_frame(&mut control, MSG_ANCHOR_READY, &self_pid.to_le_bytes()).is_err() {
        return INNER_EXIT_NORMAL;
    }
    match await_start(&mut control) {
        Ok(true) => {}
        Ok(false) => {
            let _ = write_frame(&mut control, MSG_NO_OWNERSHIP, &[]);
            return INNER_EXIT_NORMAL;
        }
        Err(error) => {
            let _ = write_frame(
                &mut control,
                MSG_PROCESS_CONTROL_FAILURE,
                error.clone().as_bytes(),
            );
            let _ = write_frame(&mut control, MSG_NO_OWNERSHIP, &[]);
            return INNER_EXIT_NORMAL;
        }
    }
    if std::env::var(FAIL_SERVER_SPAWN_ENV).is_ok() {
        let _ = write_frame(
            &mut control,
            MSG_PROCESS_CONTROL_FAILURE,
            b"injected server spawn failure",
        );
        let _ = write_frame(&mut control, MSG_NO_OWNERSHIP, &[]);
        return INNER_EXIT_NORMAL;
    }
    let mut command = Command::new(program);
    command
        .args(&arguments[1..])
        .env_remove(RUSTX_CONTROL_ENV)
        .env_remove(INNER_CONTROL_ENV);
    let server = match command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = write_frame(
                &mut control,
                MSG_PROCESS_CONTROL_FAILURE,
                format!("cannot spawn the interactive server: {error}").as_bytes(),
            );
            let _ = write_frame(&mut control, MSG_NO_OWNERSHIP, &[]);
            return INNER_EXIT_NORMAL;
        }
    };
    // The handle is discarded after spawn: the reaping hygiene below owns
    // the server and every adopted descendant through `waitpid(-1)`. The
    // server's process-group membership (not the handle) is the ownership
    // identity.
    drop(server);
    if write_frame(&mut control, MSG_OWNERSHIP_ESTABLISHED, &[]).is_err() {
        return INNER_EXIT_CONTAINMENT;
    }
    let mut reader = FrameReader::new();
    let mut kill_deadline: Option<Instant> = None;
    loop {
        // Reaping hygiene of the inner child domain: consumes every child
        // of this process — the server and owned in-group descendants
        // adopted through subreaper reparenting — so no zombie is left
        // behind. This is deliberately NOT the settlement gate.
        match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(..) | WaitStatus::Signaled(..) | WaitStatus::StillAlive | _)
            | Err(Errno::EINTR | Errno::ECHILD) => {}
            Err(error) => {
                let _ = write_frame(
                    &mut control,
                    MSG_PROCESS_CONTROL_FAILURE,
                    format!("cannot wait for the owned children: {error}").as_bytes(),
                );
                return INNER_EXIT_CONTAINMENT;
            }
        }
        // The owned-group gate: the kernel-mediated terminal condition of
        // the unit process group. Because membership is immutable for
        // server descendants, every in-group process other than this
        // supervisor is a server descendant that can never leave the group;
        // ECHILD therefore means no process other than this supervisor
        // remains in the unit group — the complete terminal state.
        match waitid(
            Id::PGid(Pid::from_raw(self_pid)),
            WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED,
        ) {
            Ok(WaitStatus::StillAlive | _) | Err(Errno::EINTR) => {}
            Err(Errno::ECHILD) => return INNER_EXIT_NORMAL,
            Err(error) => {
                let _ = write_frame(
                    &mut control,
                    MSG_PROCESS_CONTROL_FAILURE,
                    format!("cannot observe the owned group terminal state: {error}").as_bytes(),
                );
                return INNER_EXIT_CONTAINMENT;
            }
        }
        // Control frames from the outer (TERMINATE relayed from rustX).
        let mut chunk = [0u8; 256];
        match nix::unistd::read(&control, &mut chunk) {
            Ok(0) => {
                // The outer is gone: the group may still be live. The
                // containment status escalates to rustX's adopted-anchor
                // emergency path (the outer is dead and cannot contain).
                return INNER_EXIT_CONTAINMENT;
            }
            Ok(count) => {
                if let Some((kind, _payload)) = reader.push(&chunk[..count])
                    && let Err(error) = handle_inner_control_frame(
                        kind,
                        &mut control,
                        &mut kill_deadline,
                        fail_signal,
                    )
                {
                    let _ =
                        write_frame(&mut control, MSG_PROCESS_CONTROL_FAILURE, error.as_bytes());
                    return INNER_EXIT_CONTAINMENT;
                }
                while let Some((kind, _payload)) = reader.pop() {
                    if let Err(error) = handle_inner_control_frame(
                        kind,
                        &mut control,
                        &mut kill_deadline,
                        fail_signal,
                    ) {
                        let _ = write_frame(
                            &mut control,
                            MSG_PROCESS_CONTROL_FAILURE,
                            error.as_bytes(),
                        );
                        return INNER_EXIT_CONTAINMENT;
                    }
                }
            }
            Err(Errno::EAGAIN | Errno::EINTR) => {}
            Err(error) => {
                let _ = write_frame(
                    &mut control,
                    MSG_PROCESS_CONTROL_FAILURE,
                    format!("cannot read the control channel: {error}").as_bytes(),
                );
                return INNER_EXIT_CONTAINMENT;
            }
        }
        // The grace period after TERM: if the owned tree has not reached
        // its terminal child set by the deadline, KILL the unit group
        // (including this process; the outer supervisor reaps everything).
        if kill_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            let _ = signal_group(self_pid, Signal::SIGKILL);
            return INNER_EXIT_CONTAINMENT;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Handles one pre-ownership gate frame.
fn handle_start_gate_frame(kind: u8) -> Result<bool, String> {
    match kind {
        MSG_START => Ok(true),
        MSG_TERMINATE => Ok(false),
        other => Err(format!(
            "unexpected pre-ownership control message {other:#04x}"
        )),
    }
}

/// Waits at the pre-ownership gate for the owner to retain the unit anchor.
fn await_start(control: &mut std::os::unix::net::UnixStream) -> Result<bool, String> {
    let mut reader = FrameReader::new();
    loop {
        let mut chunk = [0u8; 256];
        match nix::unistd::read(&*control, &mut chunk) {
            Ok(0) => return Ok(false),
            Ok(count) => {
                if let Some((kind, _payload)) = reader.push(&chunk[..count]) {
                    return handle_start_gate_frame(kind);
                }
                if let Some((kind, _payload)) = reader.pop() {
                    return handle_start_gate_frame(kind);
                }
            }
            Err(Errno::EAGAIN | Errno::EINTR) => {
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(error) => return Err(format!("cannot read the ownership start gate: {error}")),
        }
    }
}
