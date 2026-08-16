//! The long-lived interactive supervisor unit for MCP stdio servers.
//!
//! # Structure and ownership (M5-equivalent)
//!
//! ```text
//! rustX
//!   └─ interactive outer supervisor (rustX child; Linux subreaper/reaper of
//!      last resort; frame relay between rustX and the inner)
//!        └─ interactive inner (outer child; setsid -> session/group leader;
//!                             Linux subreaper; server parent; orphan reaper;
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
//!   (`supervised_unit::enforce_fixed_group_membership`) before the server
//!   spawn. Linux rejects `setsid(2)`/`setpgid(2)` with `EPERM`; macOS keeps
//!   the dedicated process group but cannot provide that seccomp guarantee;
//! - `TERM` -> grace -> `KILL` is issued by the inner with `killpg` against
//!   **its own process group**, whose numeric id is its own pid — provably
//!   allocated exactly while the inner lives, so no foreign process group
//!   can ever receive the unit's numeric group id while signaling is legal;
//! - the kernel-mediated terminal proof is the group-scoped wait
//!   (`waitid(Id::PGid)` returning `ECHILD`) at the inner (normal) and at
//!   the outer (release gate), never a `/proc` scan and never a
//!   `killpg(..., 0)` probe. On Linux, child-subreaper adoption plus the
//!   fixed-membership restriction makes that `ECHILD` a complete whole-group
//!   proof; on macOS it only proves the waiting supervisor has no waitable
//!   group child left (reparented descendants are invisible), so macOS
//!   escalates to the outer's fallback containment `SIGKILL` instead of
//!   claiming the group is empty;
//! - the inner supervisor pid is the unit's structural ownership anchor
//!   with exactly one reaping owner: the outer's dedicated observation
//!   (`WNOWAIT`) and the outer's group-scoped release. The outer reports
//!   the authoritative terminal event only after its gate reaches `ECHILD`,
//!   and only after it released the anchor;
//! - when the inner terminates abnormally with possibly-live owned work
//!   (`INNER_EXIT_CONTAINMENT`), the outer issues the one fallback
//!   containment `SIGKILL` while the anchor is still retained, then
//!   releases the anchor through the gate; a containment signal whose result
//!   is `EPERM` is never itself terminal — on Linux it is an explicit
//!   containment failure (the unit never reports the canonical terminal
//!   event), while on macOS the `killpg(pgid, 0)` absence probe after the
//!   anchor release is the terminal authority;
//! - when the outer itself is lost, Linux rustX runs the shared adopted-anchor
//!   emergency containment; macOS has no orphan-adoption primitive and
//!   reports terminality as unproven if the anchor is no longer waitable;
//! - supervisor control traffic uses private Unix sockets, fully separate
//!   from the server's business stdin/stdout protocol pair.
//!
//! # The anchor commit point (pre-anchor vs. anchored ownership)
//!
//! ```text
//! RuntimeGated
//!     | MSG_OWNER_ATTACHED (rustX accepted and retained the outer)
//! InnerSpawnedPreAnchor          <- the inner is a direct child PID only
//!     | valid MSG_ANCHOR_READY(inner_pid)   <- THE commit point
//! Anchored                       <- inner pid == owned PGID
//!     | MSG_OWNERSHIP_ESTABLISHED
//! Owned
//!     | group-scoped waitid(Id::PGid) == ECHILD
//! Terminal
//! ```
//!
//! `MSG_ANCHOR_READY` is the exact linearization point at which the inner
//! supervisor's pid acquires its second meaning
//! (`inner pid == owned process-group id == structural ownership anchor`).
//! The inner connects its control socket **before** it runs `setsid()`,
//! the subreaper/signal setup, the fixed-membership seccomp install, and
//! the anchor announcement, so any of those can still fail on a connected
//! inner. Before the commit point the inner is therefore only a direct
//! pre-ownership child of the outer, and the outer:
//!
//! - owns and settles it strictly by pid (`kill`/`wait` on the direct
//!   child), never by process group;
//! - never runs a group-scoped wait against `inner_pid` — a
//!   `waitid(Id::PGid(inner_pid))` can return `ECHILD` without reaping an
//!   inner whose `setsid()` failed, so it is never a terminal proof and can
//!   never produce `MSG_ALL_CHILDREN_REAPED`;
//! - never relays the inner's own `MSG_NO_OWNERSHIP` upstream. A
//!   pre-anchor `MSG_NO_OWNERSHIP` is a *report of the inner's setup
//!   failure*, not a settlement proof: the outer reaps the direct inner pid
//!   first and only then emits its own **proof-carrying**
//!   `MSG_NO_OWNERSHIP` (payload: the reaped pid). If the direct reap
//!   cannot be proven, the outer emits `MSG_PROCESS_CONTROL_FAILURE` and
//!   **no** `MSG_NO_OWNERSHIP`, so terminality stays explicitly unproven.
//!
//! A valid `MSG_ANCHOR_READY` must occur exactly once, carry a positive
//! pgid, and match the direct inner child's pid; it is the only transition
//! into the group-owned lifecycle. After the commit point the group-scoped
//! ownership core alone decides terminality, so a post-anchor
//! `MSG_NO_OWNERSHIP` (for example a failed server spawn) is suppressed:
//! the inner exits, the outer retains the anchor, and the group-scoped gate
//! reaches `ECHILD` and reports `MSG_ALL_CHILDREN_REAPED`.
//!
//! # Control-frame ownership across phase transitions
//!
//! A control frame that has been read from a stream must remain owned until
//! it is either processed or explicitly rejected, so changing lifecycle
//! phase must never discard already-read bytes. Unix stream reads do not
//! preserve the writer's frame boundaries: two frames written separately can
//! arrive in one read, so gate recognition must consume exactly the gate
//! frame and leave every valid frame that followed it owned by the next
//! phase.
//!
//! Each stream direction therefore has exactly one logical buffered reader
//! for the whole connection lifetime, never a gate-local one:
//!
//! ```text
//! rustX -> outer:  await_owner_attached -> pre-inner drain
//!                  -> attach_inner_control -> await_anchor_commit
//!                  -> anchored relay loop
//! outer -> inner:  await_start -> owned inner control loop
//! ```
//!
//! Every phase drains what is already buffered before it waits for another
//! read, so a pending `MSG_TERMINATE` is observed even when no further byte
//! ever arrives. No ACK protocol and no timing assumption is used to prevent
//! coalescing: correctness does not depend on how the kernel batches reads.
//!
//! The binary is dispatched by argv role (`outer`/`inner`), mirroring the
//! Bash supervisor's `RUSTX_SUPERVISOR_ROLE` dispatch.

use std::process::{Command, Stdio};
use std::time::Instant;

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::signal::Signal;
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;

use crate::runtime::process_wait::{Id, waitid};
#[cfg(target_os = "macos")]
use crate::runtime::supervised_unit::prove_group_absent;
use crate::runtime::supervised_unit::{
    ContainmentOutcome, FrameReader, INNER_EXIT_CONTAINMENT, INNER_EXIT_NORMAL,
    MSG_ALL_CHILDREN_REAPED, MSG_ANCHOR_READY, MSG_NO_OWNERSHIP, MSG_OWNER_ATTACHED,
    MSG_OWNERSHIP_ESTABLISHED, MSG_PROCESS_CONTROL_FAILURE, MSG_SIGNAL_ATTEMPT, MSG_START,
    MSG_TERMINAL_ACK, MSG_TERMINATE, POLL_INTERVAL, TERM_GRACE, TERMINAL_ACK_TIMEOUT,
    become_child_subreaper, contain_group, enforce_fixed_group_membership, ignore_group_term,
    signal_group, write_frame,
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
/// Test-only injection: the inner supervisor's `setsid()` fails **after**
/// its control connection is established and before `MSG_ANCHOR_READY`, so
/// the inner is a connected direct child whose pid is provably not a
/// process-group id. The value names the pid file the inner writes first.
pub(crate) const FAIL_SETSID_ENV: &str = "RUSTX_TEST_INTERACTIVE_FAIL_SETSID";
/// Test-only injection: the inner supervisor connects its control socket
/// and then stalls forever before `MSG_ANCHOR_READY`. The value names the
/// pid file the inner writes right after connecting, so a regression can
/// order "the inner exists and is connected" against an outer loss without
/// a sleep.
pub(crate) const INNER_STALL_BEFORE_ANCHOR_ENV: &str =
    "RUSTX_TEST_INTERACTIVE_INNER_STALL_BEFORE_ANCHOR";
/// Test-only injection: the outer's pre-anchor cleanup cannot prove the
/// direct inner reap. This injects the semantic state only — the direct
/// child is never actually waited for, so the proof-carrying
/// `MSG_NO_OWNERSHIP` must not be emitted.
pub(crate) const FAIL_PRE_ANCHOR_REAP_ENV: &str = "RUSTX_TEST_INTERACTIVE_FAIL_PREANCHOR_REAP";

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
    // One buffered reader owns the complete rustX -> outer stream direction
    // for the whole connection lifetime: the startup gate, the pre-inner
    // phase, the pre-anchor phase, and the anchored relay loop all drain
    // this one reader. Unix stream reads do not preserve the writer's frame
    // boundaries, so a frame that arrived in the same read as the gate frame
    // must survive the phase transition instead of being discarded with a
    // phase-local reader.
    let mut upstream_reader = FrameReader::new();
    // The runtime->outer startup gate. rustX writes `MSG_OWNER_ATTACHED`
    // only after it accepted and retained this control connection, so no
    // part of the unit hierarchy — not the inner, not the server — can be
    // created while rustX might still fail its accept. Nothing is owned
    // before this returns.
    match await_owner_attached(&mut upstream, &mut upstream_reader) {
        Ok(true) => {}
        // rustX disappeared or requested shutdown at the gate: nothing was
        // ever created, so exiting is the complete settlement.
        Ok(false) => return 0,
        Err(error) => {
            eprintln!("interactive supervisor: {error}");
            return 1;
        }
    }
    // The pre-inner phase. Gate recognition consumed exactly the gate frame,
    // so anything that shared its read is still owned here and is drained
    // before any further read and before any part of the hierarchy exists. A
    // shutdown request that arrived together with the gate frame is
    // therefore observed even if no further byte ever arrives; nothing was
    // created, so "every pre-anchor child is gone" holds vacuously and the
    // empty proof-carrying `MSG_NO_OWNERSHIP` is the complete settlement.
    if drain_terminate_request(&mut upstream_reader) {
        let _ = write_frame(&mut upstream, MSG_NO_OWNERSHIP, &[]);
        return 0;
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
        // No inner child exists yet, so "every pre-anchor child is gone" is
        // vacuously proven: this `NoOwnership` is proof-carrying with an
        // empty payload (no pid was reaped because none was created).
        let _ = std::fs::remove_file(&inner_socket);
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
            // The spawn failed, so no pre-anchor child exists: an empty
            // proof-carrying `NoOwnership`.
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
    // The inner-facing direction gets its own single reader, threaded
    // through the pre-anchor and anchored phases for the same reason.
    let mut downstream_reader = FrameReader::new();
    let mut downstream = match attach_inner_control(
        &mut child,
        &inner_listener,
        &mut upstream,
        &mut upstream_reader,
    ) {
        InnerAttachment::Attached(downstream) => downstream,
        // The pre-ownership state machine already settled the direct inner
        // by pid and reported the outcome: no server-owned process tree
        // ever escaped the pre-ownership state.
        InnerAttachment::Concluded(status) => {
            let _ = std::fs::remove_file(&inner_socket);
            return status;
        }
    };
    let _ = std::fs::remove_file(&inner_socket);
    if let Err(error) = fcntl(&downstream, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)) {
        // A pre-anchor cleanup path: the inner is owned as a direct child
        // pid only, so no-ownership may be reported only after that pid is
        // provably reaped.
        return conclude_pre_anchor(
            &mut child,
            &mut upstream,
            Some(&format!(
                "cannot configure the inner control channel: {error}"
            )),
        );
    }
    // The inner is a connected direct child. Its pid is NOT a process-group
    // id yet: `setsid()` runs after the connection and can still fail. Only
    // a valid `MSG_ANCHOR_READY` commits the second meaning of this pid.
    let inner_pid = i32::try_from(child.id()).expect("the inner supervisor pid fits i32");
    match await_anchor_commit(
        &mut child,
        inner_pid,
        &mut downstream,
        &mut upstream,
        &mut downstream_reader,
        &mut upstream_reader,
    ) {
        PreAnchor::Anchored => {}
        PreAnchor::Concluded(status) => return status,
    }
    // The anchor commit point has passed: `inner_pid` is provably the owned
    // process-group id, so the group-scoped ownership core now applies.
    let mut anchor = AnchorState::Running;
    let mut anchor_loss_reported = false;
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
                        anchor = if code == INNER_EXIT_NORMAL {
                            AnchorState::TerminalRetained
                        } else {
                            // Abnormal termination with possibly-live owned
                            // work: active containment while the anchor is
                            // still held (observed but un-reaped).
                            contain_after_abnormal_exit(&mut upstream, inner_pid)
                        };
                    }
                    Ok(WaitStatus::Signaled(..)) => {
                        anchor = contain_after_abnormal_exit(&mut upstream, inner_pid);
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
                        //
                        // macOS: `ECHILD` only proves this supervisor has no
                        // waitable group child left; a reparented descendant
                        // is invisible to it. The group's absence is proven
                        // independently (after the anchor was reaped) before
                        // the terminal frame is emitted.
                        #[cfg(target_os = "macos")]
                        if let Err(error) = prove_group_absent(inner_pid) {
                            let _ = write_frame(
                                &mut upstream,
                                MSG_PROCESS_CONTROL_FAILURE,
                                error.as_bytes(),
                            );
                            anchor = AnchorState::ContainmentFailed;
                            continue;
                        }
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
            AnchorState::UnexpectedlyLost | AnchorState::ContainmentFailed => {
                // Fail-safe: never signal the unproven numeric id and never
                // report the canonical terminal event (the anchor was lost
                // before its intentional release, or the fallback
                // containment signal failed). The specific failure was
                // already reported once above.
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
                // Frames buffered by an earlier phase are drained first, so
                // nothing is stranded in the reader.
                while let Some((kind, _payload)) = upstream_reader.pop() {
                    handle_upstream_frame(kind, &mut downstream, &mut upstream, &mut ack_seen);
                }
                if ack_seen {
                    break;
                }
                match nix::unistd::read(&upstream, &mut chunk) {
                    Ok(0) => {
                        if !terminal_reported {
                            let _ = write_frame(&mut downstream, MSG_TERMINATE, &[]);
                        }
                        ack_seen = true;
                        break;
                    }
                    Ok(count) => upstream_reader.feed(&chunk[..count]),
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
        // Downstream frames (inner): post-anchor reports are relayed
        // upstream, filtered by the anchored relay contract.
        {
            let mut chunk = [0u8; 256];
            loop {
                while let Some((kind, payload)) = downstream_reader.pop() {
                    relay_anchored_frame(kind, &payload, &mut upstream);
                }
                match nix::unistd::read(&downstream, &mut chunk) {
                    Ok(0) | Err(Errno::EAGAIN | Errno::EINTR) => break,
                    Ok(count) => downstream_reader.feed(&chunk[..count]),
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
///
/// The reader is owned by the caller for the whole rustX -> outer connection
/// lifetime: already-buffered frames are drained before another read is
/// awaited, and gate recognition consumes **exactly** the gate frame, so
/// every valid frame that followed it in the same stream read stays owned by
/// the phase that runs next.
fn await_owner_attached(
    upstream: &mut std::os::unix::net::UnixStream,
    reader: &mut FrameReader,
) -> Result<bool, String> {
    loop {
        if let Some((kind, _payload)) = reader.pop() {
            return handle_startup_gate_frame(kind);
        }
        let mut chunk = [0u8; 256];
        match nix::unistd::read(&*upstream, &mut chunk) {
            Ok(0) => return Ok(false),
            Ok(count) => reader.feed(&chunk[..count]),
            Err(Errno::EAGAIN | Errno::EINTR) => std::thread::sleep(POLL_INTERVAL),
            Err(error) => return Err(format!("cannot read the startup gate: {error}")),
        }
    }
}

/// Drains every complete frame already buffered for the rustX -> outer
/// direction and reports whether a termination request was among them.
///
/// This is the pre-anchor upstream contract in one place: before the anchor
/// commit point only `MSG_TERMINATE` changes the outer's behavior, and a
/// request that shared a read with an earlier phase's frame must be observed
/// without requiring any further byte from rustX.
fn drain_terminate_request(reader: &mut FrameReader) -> bool {
    let mut terminate = false;
    while let Some((kind, _payload)) = reader.pop() {
        terminate |= kind == MSG_TERMINATE;
    }
    terminate
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
    /// The unit ended before ownership could exist. The pre-anchor inner
    /// was settled by pid and the outcome was reported upstream; the outer
    /// exits with this status.
    Concluded(i32),
}

/// The outcome of the outer's pre-anchor ownership phase.
enum PreAnchor {
    /// A valid `MSG_ANCHOR_READY` committed the anchor: from here the inner
    /// pid is provably the owned process-group id.
    Anchored,
    /// The unit ended before the anchor commit point. The direct inner was
    /// settled by pid and the outcome was reported upstream; the outer
    /// exits with this status.
    Concluded(i32),
}

/// The classification of one inner control frame received before the anchor
/// commit point.
enum PreAnchorFrame {
    /// A valid, unique, pid-matching `MSG_ANCHOR_READY`: the commit point.
    AnchorCommit,
    /// A report that carries no ownership claim; relayed upstream as-is.
    Informational,
    /// The inner reported that its setup ended before it could own
    /// anything. This is **not** a settlement proof: the outer must reap
    /// the direct inner pid before it may report no-ownership itself.
    SetupEnded,
    /// A pre-anchor protocol violation; the message is human-readable.
    Violation(String),
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
    upstream_reader: &mut FrameReader,
) -> InnerAttachment {
    loop {
        match inner_listener.accept() {
            Ok((downstream, _)) => return InnerAttachment::Attached(downstream),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(error) => {
                return InnerAttachment::Concluded(conclude_pre_anchor(
                    child,
                    upstream,
                    Some(&format!(
                        "cannot accept the inner control connection: {error}"
                    )),
                ));
            }
        }
        // The inner direct child may have exited before connecting. The
        // server spawn is gated behind the control connection, so its exit
        // here provably leaves no owned process tree; the pid-scoped
        // settlement below is the complete containment.
        match child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                return InnerAttachment::Concluded(conclude_pre_anchor(
                    child,
                    upstream,
                    Some(&format!(
                        "the inner supervisor exited before connecting its control channel: \
                         {status}"
                    )),
                ));
            }
            Err(error) => {
                return InnerAttachment::Concluded(conclude_pre_anchor(
                    child,
                    upstream,
                    Some(&format!(
                        "cannot observe the pre-ownership inner supervisor: {error}"
                    )),
                ));
            }
        }
        // rustX termination requests and control loss before ownership.
        // Frames buffered by an earlier phase are drained before another
        // read is awaited, so a request that arrived in the same read as the
        // startup gate frame is acted on here even if rustX never writes
        // again.
        let mut chunk = [0u8; 256];
        loop {
            if drain_terminate_request(upstream_reader) {
                return InnerAttachment::Concluded(conclude_pre_anchor(child, upstream, None));
            }
            match nix::unistd::read(&*upstream, &mut chunk) {
                Ok(0) => {
                    return InnerAttachment::Concluded(conclude_pre_anchor(child, upstream, None));
                }
                Ok(count) => upstream_reader.feed(&chunk[..count]),
                Err(Errno::EAGAIN | Errno::EINTR) => break,
                Err(error) => {
                    return InnerAttachment::Concluded(conclude_pre_anchor(
                        child,
                        upstream,
                        Some(&format!("cannot read the rustX control channel: {error}")),
                    ));
                }
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The outer's pre-anchor ownership phase: the inner is a connected direct
/// child pid, and nothing about it is group-scoped yet.
///
/// Exactly three transitions are monitored, none of which may use
/// `inner_pid` as a process-group id:
///
/// - a valid `MSG_ANCHOR_READY` (unique, positive pgid, equal to the direct
///   inner child's pid) — the commit point, relayed upstream;
/// - the direct inner child's own exit, or a pre-anchor report/violation on
///   its control channel — settled by pid;
/// - rustX termination/control loss — settled by pid.
fn await_anchor_commit(
    child: &mut std::process::Child,
    inner_pid: i32,
    downstream: &mut std::os::unix::net::UnixStream,
    upstream: &mut std::os::unix::net::UnixStream,
    downstream_reader: &mut FrameReader,
    upstream_reader: &mut FrameReader,
) -> PreAnchor {
    loop {
        let mut chunk = [0u8; 256];
        // Inner control frames before the commit point.
        loop {
            let mut frame = downstream_reader.pop();
            while let Some((kind, payload)) = frame {
                match classify_pre_anchor_frame(kind, &payload, inner_pid) {
                    PreAnchorFrame::AnchorCommit => {
                        // The single linearization point into the anchored,
                        // group-owned lifecycle.
                        let _ = write_frame(upstream, MSG_ANCHOR_READY, &payload);
                        return PreAnchor::Anchored;
                    }
                    PreAnchorFrame::Informational => {
                        let _ = write_frame(upstream, kind, &payload);
                    }
                    PreAnchorFrame::SetupEnded => {
                        // Never relayed as a proof: the outer settles the
                        // direct inner pid and reports no-ownership itself.
                        return PreAnchor::Concluded(conclude_pre_anchor(child, upstream, None));
                    }
                    PreAnchorFrame::Violation(message) => {
                        return PreAnchor::Concluded(conclude_pre_anchor(
                            child,
                            upstream,
                            Some(&message),
                        ));
                    }
                }
                frame = downstream_reader.pop();
            }
            match nix::unistd::read(&*downstream, &mut chunk) {
                Ok(0) => {
                    return PreAnchor::Concluded(conclude_pre_anchor(
                        child,
                        upstream,
                        Some(
                            "the inner supervisor closed its control channel before announcing \
                             the unit anchor",
                        ),
                    ));
                }
                Ok(count) => downstream_reader.feed(&chunk[..count]),
                Err(Errno::EAGAIN | Errno::EINTR) => break,
                Err(error) => {
                    return PreAnchor::Concluded(conclude_pre_anchor(
                        child,
                        upstream,
                        Some(&format!(
                            "cannot read the inner control channel before the unit anchor: \
                             {error}"
                        )),
                    ));
                }
            }
        }
        // The direct inner child's exit before the commit point. Its pid is
        // not a process-group id, so only this pid-scoped wait can settle
        // it — a group-scoped wait would reach `ECHILD` without reaping it.
        match child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                return PreAnchor::Concluded(conclude_pre_anchor(
                    child,
                    upstream,
                    Some(&format!(
                        "the inner supervisor exited before announcing the unit anchor: {status}"
                    )),
                ));
            }
            Err(error) => {
                return PreAnchor::Concluded(conclude_pre_anchor(
                    child,
                    upstream,
                    Some(&format!(
                        "cannot observe the pre-anchor inner supervisor: {error}"
                    )),
                ));
            }
        }
        // rustX termination requests and control loss before the anchor.
        // Buffered frames are drained before another read is awaited: a
        // request carried over from an earlier phase's read is owned here.
        loop {
            if drain_terminate_request(upstream_reader) {
                return PreAnchor::Concluded(conclude_pre_anchor(child, upstream, None));
            }
            match nix::unistd::read(&*upstream, &mut chunk) {
                Ok(0) => return PreAnchor::Concluded(conclude_pre_anchor(child, upstream, None)),
                Ok(count) => upstream_reader.feed(&chunk[..count]),
                Err(Errno::EAGAIN | Errno::EINTR) => break,
                Err(error) => {
                    return PreAnchor::Concluded(conclude_pre_anchor(
                        child,
                        upstream,
                        Some(&format!(
                            "cannot read the rustX control channel before the unit anchor: {error}"
                        )),
                    ));
                }
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Classifies one inner control frame received before the anchor commit.
fn classify_pre_anchor_frame(kind: u8, payload: &[u8], inner_pid: i32) -> PreAnchorFrame {
    match kind {
        MSG_ANCHOR_READY => {
            let Ok(bytes) = <[u8; 4]>::try_from(payload) else {
                return PreAnchorFrame::Violation(
                    "the unit anchor announcement is malformed".to_owned(),
                );
            };
            let pgid = i32::from_le_bytes(bytes);
            if pgid <= 0 {
                PreAnchorFrame::Violation(format!(
                    "the unit anchor announcement carries the non-positive process-group id \
                     {pgid}"
                ))
            } else if pgid != inner_pid {
                PreAnchorFrame::Violation(format!(
                    "the unit anchor announcement ({pgid}) does not match the direct inner \
                     supervisor child ({inner_pid})"
                ))
            } else {
                PreAnchorFrame::AnchorCommit
            }
        }
        MSG_NO_OWNERSHIP => PreAnchorFrame::SetupEnded,
        MSG_PROCESS_CONTROL_FAILURE | MSG_SIGNAL_ATTEMPT => PreAnchorFrame::Informational,
        other => PreAnchorFrame::Violation(format!(
            "unexpected pre-anchor inner control message {other:#04x}"
        )),
    }
}

/// Settles the pre-anchor inner **by pid** and reports the outcome.
///
/// Before the anchor commit point the inner is only a direct child: its pid
/// is not a process-group id, and it cannot have spawned the server (that
/// is gated behind `MSG_START`, which rustX sends only after the anchor
/// announcement). Terminating and reaping that one pid is therefore the
/// complete physical containment of the unit at this stage.
///
/// `MSG_NO_OWNERSHIP` is emitted **only** on a proven reap, and carries the
/// reaped pid as its payload: that is what makes it proof-carrying. A
/// failed direct-inner reap leaves terminality unproven, so it reports a
/// process-control failure and deliberately emits no `MSG_NO_OWNERSHIP`.
///
/// Returns the outer's exit status.
fn conclude_pre_anchor(
    child: &mut std::process::Child,
    upstream: &mut std::os::unix::net::UnixStream,
    failure: Option<&str>,
) -> i32 {
    if let Some(message) = failure {
        let _ = write_frame(upstream, MSG_PROCESS_CONTROL_FAILURE, message.as_bytes());
    }
    let inner_pid = i32::try_from(child.id()).unwrap_or(0);
    let _ = child.kill();
    let reaped = if std::env::var(FAIL_PRE_ANCHOR_REAP_ENV).is_ok() {
        // Test-only injection of the semantic state "the pre-anchor child
        // cleanup cannot prove the reap"; the child is deliberately not
        // waited for.
        Err("injected pre-anchor reap failure".to_owned())
    } else {
        child
            .wait()
            .map(|_status| ())
            .map_err(|error| error.to_string())
    };
    match reaped {
        Ok(()) => {
            let _ = write_frame(upstream, MSG_NO_OWNERSHIP, &inner_pid.to_le_bytes());
            0
        }
        Err(error) => {
            let _ = write_frame(
                upstream,
                MSG_PROCESS_CONTROL_FAILURE,
                format!(
                    "cannot reap the pre-anchor inner supervisor (pid {inner_pid}): {error}; the \
                     pre-anchor child's terminal state cannot be proven, so no-ownership is not \
                     reported"
                )
                .as_bytes(),
            );
            1
        }
    }
}

/// Relays one post-anchor inner control frame upstream.
///
/// After the commit point the unit's terminality is decided by the outer's
/// group-scoped gate alone. A post-anchor `MSG_NO_OWNERSHIP` (for example a
/// failed server spawn) is therefore inner-local information and is
/// suppressed: the inner exits, the outer retains the anchor, and the
/// group-scoped wait reaches `ECHILD` and reports the authoritative
/// `MSG_ALL_CHILDREN_REAPED`. A second `MSG_ANCHOR_READY` is a protocol
/// violation: it is reported, never relayed.
fn relay_anchored_frame(kind: u8, payload: &[u8], upstream: &mut std::os::unix::net::UnixStream) {
    match kind {
        MSG_NO_OWNERSHIP => {}
        MSG_ANCHOR_READY => {
            let _ = write_frame(
                upstream,
                MSG_PROCESS_CONTROL_FAILURE,
                b"the inner supervisor announced the unit anchor more than once",
            );
        }
        _ => {
            let _ = write_frame(upstream, kind, payload);
        }
    }
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
    /// The fallback containment signal failed while the anchor was still
    /// retained. The owned group was not provably contained, so the unit
    /// fails safely and never reports the canonical terminal event.
    ContainmentFailed,
}

/// The outer supervisor's active containment: one final `SIGKILL` to the
/// unit's process group, issued only while the structural anchor is held.
///
/// The raw result is classified into [`ContainmentOutcome`]: `Contained`
/// (`Ok` or `ESRCH`) versus `Unproven` (`EPERM` and every other error).
fn containment_signal(
    upstream: &mut std::os::unix::net::UnixStream,
    pgid: i32,
) -> ContainmentOutcome {
    let mut payload = Vec::with_capacity(9);
    payload.extend_from_slice(&pgid.to_le_bytes());
    payload.extend_from_slice(&(Signal::SIGKILL as i32).to_le_bytes());
    payload.push(1);
    let _ = write_frame(upstream, MSG_SIGNAL_ATTEMPT, &payload);
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
fn contain_after_abnormal_exit(
    upstream: &mut std::os::unix::net::UnixStream,
    pgid: i32,
) -> AnchorState {
    match containment_signal(upstream, pgid) {
        ContainmentOutcome::Contained => AnchorState::TerminalRetained,
        ContainmentOutcome::Unproven(error) => {
            #[cfg(target_os = "linux")]
            {
                let _ = write_frame(upstream, MSG_PROCESS_CONTROL_FAILURE, error.as_bytes());
                AnchorState::ContainmentFailed
            }
            #[cfg(target_os = "macos")]
            {
                let _ = error;
                AnchorState::TerminalRetained
            }
        }
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
    if let Ok(pid_file) = std::env::var(INNER_STALL_BEFORE_ANCHOR_ENV) {
        // Test-only injection: a connected direct child that never reaches
        // the anchor commit point. The pid file is written after the
        // connection, so a regression can order "the inner exists and is
        // connected" against an outer loss without a sleep.
        let _ = std::fs::write(&pid_file, std::process::id().to_string());
        loop {
            std::thread::sleep(POLL_INTERVAL);
        }
    }
    if let Ok(pid_file) = std::env::var(FAIL_SETSID_ENV) {
        // Test-only injection: `setsid()` fails after the control
        // connection exists. This is byte-for-byte the real setsid-failure
        // path below, so the inner stays in its parent's process group and
        // its pid is provably not a process-group id.
        let _ = std::fs::write(&pid_file, std::process::id().to_string());
        let _ = write_frame(
            &mut control,
            MSG_PROCESS_CONTROL_FAILURE,
            b"injected setsid failure after the inner control connection",
        );
        let _ = write_frame(&mut control, MSG_NO_OWNERSHIP, &[]);
        return INNER_EXIT_NORMAL;
    }
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
    // One buffered reader owns the complete outer -> inner stream direction:
    // the START gate and the owned control loop below share it, so a frame
    // that arrived in the same read as `MSG_START` remains owned by the loop
    // instead of being dropped with a phase-local reader.
    let mut reader = FrameReader::new();
    match await_start(&mut control, &mut reader) {
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
        //
        // On macOS this proof does not hold: without a child-subreaper, a
        // descendant that outlives the server is reparented to launchd and
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
                let _ = write_frame(
                    &mut control,
                    MSG_PROCESS_CONTROL_FAILURE,
                    format!("cannot observe the owned group terminal state: {error}").as_bytes(),
                );
                return INNER_EXIT_CONTAINMENT;
            }
        }
        // Control frames from the outer (TERMINATE relayed from rustX).
        // Frames buffered while the START gate frame was processed belong to
        // this phase, so they are drained before another read is awaited: a
        // TERMINATE that arrived in the same read as START is acted on even
        // if no further byte ever arrives.
        let mut chunk = [0u8; 256];
        loop {
            while let Some((kind, _payload)) = reader.pop() {
                if let Err(error) =
                    handle_inner_control_frame(kind, &mut control, &mut kill_deadline, fail_signal)
                {
                    let _ =
                        write_frame(&mut control, MSG_PROCESS_CONTROL_FAILURE, error.as_bytes());
                    return INNER_EXIT_CONTAINMENT;
                }
            }
            match nix::unistd::read(&control, &mut chunk) {
                Ok(0) => {
                    // The outer is gone: the group may still be live. The
                    // containment status escalates to rustX's adopted-anchor
                    // emergency path (the outer is dead and cannot contain).
                    return INNER_EXIT_CONTAINMENT;
                }
                Ok(count) => reader.feed(&chunk[..count]),
                Err(Errno::EAGAIN | Errno::EINTR) => break,
                Err(error) => {
                    let _ = write_frame(
                        &mut control,
                        MSG_PROCESS_CONTROL_FAILURE,
                        format!("cannot read the control channel: {error}").as_bytes(),
                    );
                    return INNER_EXIT_CONTAINMENT;
                }
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
///
/// The reader is owned by the caller across the gate: already-buffered
/// frames are drained before another read is awaited, and gate recognition
/// consumes **exactly** the `MSG_START` frame. Everything that followed it in
/// the same stream read — a `MSG_TERMINATE` in particular — stays owned by
/// the inner's control loop and is processed there without requiring another
/// socket read.
fn await_start(
    control: &mut std::os::unix::net::UnixStream,
    reader: &mut FrameReader,
) -> Result<bool, String> {
    loop {
        if let Some((kind, _payload)) = reader.pop() {
            return handle_start_gate_frame(kind);
        }
        let mut chunk = [0u8; 256];
        match nix::unistd::read(&*control, &mut chunk) {
            Ok(0) => return Ok(false),
            Ok(count) => reader.feed(&chunk[..count]),
            Err(Errno::EAGAIN | Errno::EINTR) => {
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(error) => return Err(format!("cannot read the ownership start gate: {error}")),
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod coalesced_gate_tests {
    //! Deterministic regressions of the two control-protocol gates against
    //! coalesced stream reads.
    //!
    //! Unix stream reads do not preserve the writer's frame boundaries, so
    //! both gate frames are delivered here as exactly one input batch: a
    //! single `write_all` of two complete frames into a stream socket whose
    //! peer reads with one 256-byte buffer. Nothing is ever written a second
    //! time and no sleep is used for synchronization — the ordering points
    //! are the socket accept and the supervisor's own announcement frames.
    //!
    //! Each test can therefore only pass if the trailing frame, buffered
    //! while the gate frame was being processed, is still owned by the phase
    //! that follows the gate. A gate that reverted to a phase-local
    //! [`FrameReader`] would discard it and hang until the deadline.

    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use nix::sys::signal::Signal;
    use nix::unistd::Pid;

    use super::{
        INNER_CONTROL_ENV, INNER_EXIT_NORMAL, MSG_ANCHOR_READY, MSG_NO_OWNERSHIP,
        MSG_OWNER_ATTACHED, MSG_OWNERSHIP_ESTABLISHED, MSG_START, MSG_TERMINATE, RUSTX_CONTROL_ENV,
    };
    use crate::runtime::process_runner::interactive_supervisor_binary;
    use crate::runtime::supervised_unit::FrameReader;

    /// The deadlock guard of every wait in this module; never a correctness
    /// assertion.
    const DEADLINE: Duration = Duration::from_secs(20);
    /// The bounded read cadence of the collecting test peer.
    const POLL: Duration = Duration::from_millis(10);

    /// A control-socket path unique per test and per process.
    fn unique_socket(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        std::env::temp_dir().join(format!(
            "rustx-gate-{}-{}-{name}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// One control frame on the wire: `[u32 LE length][kind][payload]`.
    fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
        let length = u32::try_from(1 + payload.len()).expect("frame length fits u32");
        let mut bytes = length.to_le_bytes().to_vec();
        bytes.push(kind);
        bytes.extend_from_slice(payload);
        bytes
    }

    /// Kills the supervisor process and its process group before failing, so
    /// a failing regression can never leave a long-lived server behind.
    fn abandon(child: &mut Child, message: &str) -> ! {
        if let Ok(pid) = i32::try_from(child.id()) {
            let _ = nix::sys::signal::killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("{message}");
    }

    /// Accepts the supervisor's control connection.
    fn accept_within(listener: &UnixListener, child: &mut Child, description: &str) -> UnixStream {
        listener
            .set_nonblocking(true)
            .expect("bounded accept polling");
        let deadline = Instant::now() + DEADLINE;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream
                        .set_nonblocking(false)
                        .expect("blocking control stream");
                    return stream;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        abandon(child, description);
                    }
                    std::thread::sleep(POLL);
                }
                Err(error) => abandon(child, &format!("{description}: {error}")),
            }
        }
    }

    /// Reads the next complete control frame written by the supervisor.
    fn read_frame(control: &mut UnixStream, child: &mut Child, description: &str) -> (u8, Vec<u8>) {
        control
            .set_read_timeout(Some(POLL))
            .expect("bounded control reads");
        let mut reader = FrameReader::new();
        let mut chunk = [0u8; 256];
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(frame) = reader.pop() {
                return frame;
            }
            match control.read(&mut chunk) {
                Ok(0) => abandon(child, &format!("{description}: the control channel closed")),
                Ok(count) => reader.feed(&chunk[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => abandon(child, &format!("{description}: {error}")),
            }
            if Instant::now() >= deadline {
                abandon(child, description);
            }
        }
    }

    /// Collects every control frame the supervisor writes until it exits,
    /// and returns its exit status.
    ///
    /// The test peer never writes again while collecting, so a supervisor
    /// that lost the trailing frame cannot be rescued by further input: it
    /// hits the deadline and the regression fails.
    fn collect_until_exit(
        control: &mut UnixStream,
        child: &mut Child,
        description: &str,
    ) -> (Vec<(u8, Vec<u8>)>, ExitStatus) {
        control
            .set_read_timeout(Some(POLL))
            .expect("bounded control reads");
        let mut reader = FrameReader::new();
        let mut frames = Vec::new();
        let mut chunk = [0u8; 256];
        let deadline = Instant::now() + DEADLINE;
        loop {
            match control.read(&mut chunk) {
                Ok(0) => std::thread::sleep(POLL),
                Ok(count) => {
                    reader.feed(&chunk[..count]);
                    while let Some(frame) = reader.pop() {
                        frames.push(frame);
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => abandon(child, &format!("{description}: {error}")),
            }
            match child.try_wait().expect("observe the supervisor process") {
                // The process may have written its last frames between this
                // iteration's read and its exit, so the queued bytes are
                // drained to EOF before the frames are reported.
                Some(status) => {
                    loop {
                        match control.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(count) => {
                                reader.feed(&chunk[..count]);
                                while let Some(frame) = reader.pop() {
                                    frames.push(frame);
                                }
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(_) => break,
                        }
                        assert!(
                            Instant::now() < deadline,
                            "{description}: the control channel never reached EOF"
                        );
                    }
                    return (frames, status);
                }
                None => {
                    if Instant::now() >= deadline {
                        abandon(child, description);
                    }
                }
            }
        }
    }

    /// The runtime -> outer gate: `MSG_OWNER_ATTACHED` and `MSG_TERMINATE`
    /// arrive in one read.
    ///
    /// The gate must consume exactly the gate frame. The shutdown request
    /// that shared its read belongs to the next phase, which observes it
    /// without any further input: the outer never creates the unit
    /// hierarchy and settles with the empty proof-carrying
    /// `MSG_NO_OWNERSHIP` (nothing was created, so "every pre-anchor child
    /// is gone and reaped" holds vacuously).
    ///
    /// The single `MSG_NO_OWNERSHIP` frame is also what proves the gate
    /// opened: an outer that treated the batch as a closed gate exits
    /// silently without writing anything at all.
    #[test]
    fn owner_attached_and_terminate_in_one_read_never_loses_the_terminate() {
        let fixture = tempfile::tempdir().expect("fixture dir");
        let server_marker = fixture.path().join("server-started");
        let socket = unique_socket("owner-attached");
        let listener = UnixListener::bind(&socket).expect("bind the rustX control socket");
        let mut outer = Command::new(interactive_supervisor_binary())
            .arg("outer")
            .arg("/bin/sh")
            .arg("-c")
            .arg(format!(
                "echo started > {}; exec sleep 600",
                server_marker.display()
            ))
            .current_dir(fixture.path())
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env(RUSTX_CONTROL_ENV, &socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the outer supervisor");
        let mut control = accept_within(
            &listener,
            &mut outer,
            "the outer supervisor must connect its control channel",
        );
        // One write, one input batch: the startup gate frame and the
        // shutdown request are indistinguishable from a single stream read.
        let mut batch = frame(MSG_OWNER_ATTACHED, &[]);
        batch.extend_from_slice(&frame(MSG_TERMINATE, &[]));
        control
            .write_all(&batch)
            .expect("the coalesced startup-gate batch");
        // Nothing is ever written again.
        let (frames, status) = collect_until_exit(
            &mut control,
            &mut outer,
            "the outer supervisor must observe the buffered shutdown request without another \
             input write",
        );
        assert_eq!(
            frames,
            vec![(MSG_NO_OWNERSHIP, Vec::new())],
            "the gate must open and the coalesced shutdown request must settle the unit with the \
             empty proof-carrying no-ownership frame"
        );
        assert_eq!(
            status.code(),
            Some(0),
            "the settled outer supervisor must exit cleanly"
        );
        assert!(
            !server_marker.exists(),
            "no long-lived server may be created by a unit that was terminated at its startup gate"
        );
        let _ = std::fs::remove_file(&socket);
    }

    /// The outer -> inner START gate: `MSG_START` and `MSG_TERMINATE` arrive
    /// in one read.
    ///
    /// START commits the gate and the owned server is spawned
    /// (`MSG_OWNERSHIP_ESTABLISHED` proves it). The trailing shutdown
    /// request stays owned by the inner's control loop, which acts on it
    /// without another socket read: the unit group is terminated and the
    /// inner exits with [`INNER_EXIT_NORMAL`], which it returns **only**
    /// after the group-scoped `waitid(Id::PGid)` reached `ECHILD` — the
    /// kernel-mediated proof that no owned process remains.
    #[test]
    fn start_and_terminate_in_one_read_never_loses_the_terminate() {
        let fixture = tempfile::tempdir().expect("fixture dir");
        let socket = unique_socket("start-gate");
        let listener = UnixListener::bind(&socket).expect("bind the inner control socket");
        let mut inner = Command::new(interactive_supervisor_binary())
            .arg("inner")
            .arg("/bin/sh")
            .arg("-c")
            .arg("exec sleep 600")
            .current_dir(fixture.path())
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env(INNER_CONTROL_ENV, &socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the inner supervisor");
        let inner_pid = i32::try_from(inner.id()).expect("the inner supervisor pid fits i32");
        let mut control = accept_within(
            &listener,
            &mut inner,
            "the inner supervisor must connect its control channel",
        );
        // The synchronization point is the inner's own anchor announcement:
        // once it is read, the inner is provably waiting at the START gate.
        let (kind, payload) = read_frame(
            &mut control,
            &mut inner,
            "the inner supervisor must announce the unit anchor",
        );
        assert_eq!(
            kind, MSG_ANCHOR_READY,
            "the anchor announcement comes first"
        );
        assert_eq!(
            i32::from_le_bytes(<[u8; 4]>::try_from(payload.as_slice()).expect("pgid payload")),
            inner_pid,
            "the announced anchor is the inner supervisor's own pid"
        );
        // One write, one input batch: START and the shutdown request are
        // indistinguishable from a single stream read.
        let mut batch = frame(MSG_START, &[]);
        batch.extend_from_slice(&frame(MSG_TERMINATE, &[]));
        control
            .write_all(&batch)
            .expect("the coalesced start-gate batch");
        // Nothing is ever written again.
        let (frames, status) = collect_until_exit(
            &mut control,
            &mut inner,
            "the inner supervisor must observe the buffered shutdown request without another \
             input write",
        );
        assert!(
            frames
                .iter()
                .any(|(kind, _payload)| *kind == MSG_OWNERSHIP_ESTABLISHED),
            "START must be recognized and commit the owned server spawn: {frames:?}"
        );
        assert_eq!(
            status.code(),
            Some(INNER_EXIT_NORMAL),
            "the buffered shutdown request must settle the owned group: this status is returned \
             only after the group-scoped wait reached ECHILD"
        );
        let _ = std::fs::remove_file(&socket);
    }
}
