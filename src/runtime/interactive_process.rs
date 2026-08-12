//! The rustX-side driver of one long-lived interactive process (MCP stdio
//! servers).
//!
//! The driver is the physical settlement owner of the interactive
//! supervisor unit:
//!
//! - the runtime child-subreaper prerequisite is established before the
//!   supervisor unit spawns;
//! - once the supervisor spawn succeeds, the runtime-owned driver task
//!   immediately owns physical settlement; a later handshake/control setup
//!   error (accept failure, connection loss) transfers into an explicit
//!   containment/reap path instead of stranding a raw child;
//! - the runtime->outer startup gate (`MSG_OWNER_ATTACHED`) is written only
//!   after the outer control connection was accepted and retained, so a
//!   failed accept can only ever find a gated outer that owns nothing;
//! - the direct supervisor child is reaped before physical settlement is
//!   published;
//! - the unit's terminal event is the outer supervisor's authoritative
//!   `AllChildrenReaped` report; control-channel loss before it escalates
//!   to the shared adopted-anchor emergency containment;
//! - physical settlement is published **only** with proven terminality:
//!   the authoritative terminal event plus the direct-child reap, an
//!   emergency containment that returned
//!   [`EmergencyContainment::TerminalProven`], or a unit that never left
//!   the pre-ownership state. An unavailable emergency anchor, a
//!   containment failure, or a containment-task failure is reported as
//!   [`UnitSettlement::TerminalityUnproven`] and is never a successful
//!   physical settlement;
//! - stderr is drained until EOF; only a bounded preview is retained, and
//!   reading never stops merely because the preview limit was reached;
//! - dropping the business-facing handle requests shutdown but never
//!   abandons the physical process owner.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use tokio::io::AsyncReadExt;

use crate::runtime::interactive_supervisor::RUSTX_CONTROL_ENV;
use crate::runtime::process_runner::{
    MAX_PROCESS_OUTPUT_BYTES, SupervisorEvent, read_supervisor_event, send_owner_attached,
    send_start, send_terminal_ack, send_terminate,
};
use crate::runtime::supervised_unit::{EmergencyContainment, emergency_contain_group};

/// The published physical settlement of one interactive supervisor unit.
///
/// The two states are not interchangeable: only [`Self::PhysicallySettled`]
/// claims that the owned process tree is provably terminal. Process-control
/// failures are a separate axis and are reported inside the unproven
/// reason, never as a successful settlement.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnitSettlement {
    /// The owned process tree is provably terminal and the direct
    /// supervisor child was reaped: the authoritative `AllChildrenReaped`
    /// event was received, emergency containment returned
    /// [`EmergencyContainment::TerminalProven`], or the unit never left the
    /// pre-ownership state (no owned process tree was ever created).
    PhysicallySettled,
    /// The owned process tree's terminal state could not be proven. This is
    /// never a successful physical settlement: the group may still exist.
    TerminalityUnproven(String),
}

/// The rustX-side ownership state of one interactive supervisor unit
/// (mirrors the M5 `ProcessLifecycle`).
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnitLifecycle {
    /// No owned process tree can exist yet.
    PreOwnership,
    /// The unit anchor was announced; the owned child may not be spawned
    /// yet, but the anchor identity is retained by the supervisor unit.
    OwnershipPossible { pgid: i32 },
    /// The owned server was spawned inside the fixed-membership group.
    Owned { pgid: i32 },
    /// The owned process tree is provably terminal.
    Terminal,
}

/// The settlement publication cell shared by the driver and the handle.
#[cfg(unix)]
struct SettlementCell {
    state: Mutex<Option<UnitSettlement>>,
    notify: tokio::sync::Notify,
}

#[cfg(unix)]
impl SettlementCell {
    fn new() -> Self {
        Self {
            state: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        }
    }

    /// Publishes the one settlement of the unit; later calls are ignored.
    fn publish(&self, settlement: UnitSettlement) {
        let mut state = self.state.lock().expect("interactive settlement lock");
        if state.is_none() {
            *state = Some(settlement);
        }
        drop(state);
        self.notify.notify_waiters();
    }

    fn observed(&self) -> Option<UnitSettlement> {
        self.state
            .lock()
            .expect("interactive settlement lock")
            .clone()
    }
}

/// The test-only control seams of one interactive supervisor unit.
///
/// In non-test builds this type is a fieldless shell (mirroring
/// `RunnerTestControl`), so no production behavior is affected. The seams
/// exist so in-crate regressions can force an unavailable emergency anchor
/// and a runtime-facing accept failure, and can observe the supervisor
/// events the driver actually received — without an operating system
/// mocking framework.
#[cfg(unix)]
#[derive(Clone)]
pub(crate) struct InteractiveTestControl {
    /// Forces the emergency containment of a lost unit to report
    /// [`EmergencyContainment::AnchorUnavailable`].
    #[cfg(test)]
    pub(crate) force_emergency_anchor_unavailable: Arc<std::sync::atomic::AtomicBool>,
    /// Forces the runtime-facing `accept()` to fail while the outer
    /// supervisor is still alive and gated.
    #[cfg(test)]
    pub(crate) force_accept_failure: Arc<std::sync::atomic::AtomicBool>,
    /// The supervisor events the driver observed, in arrival order.
    #[cfg(test)]
    pub(crate) observed_events: Arc<Mutex<Vec<String>>>,
}

#[cfg(unix)]
impl InteractiveTestControl {
    /// A control bundle without injected failures.
    #[cfg(test)]
    fn new() -> Self {
        Self {
            force_emergency_anchor_unavailable: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            force_accept_failure: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            observed_events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A control bundle without injected failures (non-test build: a
    /// fieldless shell).
    #[cfg(not(test))]
    fn new() -> Self {
        Self {}
    }

    /// The observed supervisor events, in arrival order.
    #[cfg(test)]
    pub(crate) fn observed_events(&self) -> Vec<String> {
        self.observed_events
            .lock()
            .expect("interactive event lock")
            .clone()
    }
}

/// Records one observed supervisor event for the in-crate regressions.
#[cfg(all(unix, test))]
fn record_event(control: &InteractiveTestControl, event: &str) {
    control
        .observed_events
        .lock()
        .expect("interactive event lock")
        .push(event.to_owned());
}

#[cfg(all(unix, not(test)))]
fn record_event(_control: &InteractiveTestControl, _event: &str) {}

/// The explicit command description of one long-lived interactive server.
/// Unlike the supervised-command spec, the business stdin/stdout pair
/// belongs to the child protocol; supervisor control uses a private Unix
/// socket.
#[cfg(unix)]
#[derive(Debug, Clone)]
pub(crate) struct InteractiveProcessSpec {
    /// The executable to run inside the owned process group.
    pub program: PathBuf,
    /// Explicit program arguments.
    pub args: Vec<String>,
    /// Explicit working directory.
    pub cwd: PathBuf,
    /// Explicit environment, never inherited.
    pub environment: Vec<(String, String)>,
}

/// A rustX-owned interactive process handle.
///
/// The returned protocol streams are business-facing handles. The detached
/// driver owns the supervisor child and the control connection, so dropping
/// the streams cannot abandon the process hierarchy. `Drop` requests orderly
/// shutdown; the driver waits for the supervisor's terminal event and then
/// reaps its direct child.
#[cfg(unix)]
pub(crate) struct SupervisedInteractiveProcess {
    pub stdin: Option<tokio::process::ChildStdin>,
    pub stdout: Option<tokio::process::ChildStdout>,
    /// The bounded stderr preview; the drain task reads until EOF regardless
    /// of the preview bound.
    pub(crate) stderr_preview: Arc<Mutex<Vec<u8>>>,
    settlement: Arc<SettlementCell>,
    shutdown: Option<tokio::sync::mpsc::Sender<()>>,
    /// Test-only: the pid of the direct supervisor child (observability for
    /// the direct-reap regression).
    #[cfg(test)]
    supervisor_child_pid: Option<u32>,
}

#[cfg(unix)]
impl SupervisedInteractiveProcess {
    /// Starts one interactive process under the dedicated long-lived owner.
    pub(crate) fn spawn(spec: InteractiveProcessSpec) -> Result<Self, String> {
        Self::spawn_with_control(spec, InteractiveTestControl::new())
    }

    /// Starts one interactive process with the given control seams.
    fn spawn_with_control(
        spec: InteractiveProcessSpec,
        test_control: InteractiveTestControl,
    ) -> Result<Self, String> {
        use std::sync::atomic::AtomicU64;
        static NEXT_SOCKET: AtomicU64 = AtomicU64::new(1);

        // The runtime child-subreaper capability is a pre-ownership
        // prerequisite: the catastrophic fallback authority must exist
        // before the supervisor unit spawns (mirrors the short-lived
        // supervised-command runner).
        crate::runtime::process_supervision::ensure_child_subreaper()?;

        let InteractiveProcessSpec {
            program,
            args,
            cwd,
            environment,
        } = spec;
        let socket_path = std::env::temp_dir().join(format!(
            "rustx-interactive-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = tokio::net::UnixListener::bind(&socket_path)
            .map_err(|error| format!("cannot bind interactive control socket: {error}"))?;
        let mut supervisor = tokio::process::Command::new(
            crate::runtime::process_runner::interactive_supervisor_binary(),
        );
        supervisor
            .arg("outer")
            .arg(&program)
            .args(&args)
            .current_dir(&cwd)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &environment {
            supervisor.env(key, value);
        }
        supervisor.env(RUSTX_CONTROL_ENV, &socket_path);
        let mut child = supervisor
            .spawn()
            .map_err(|error| format!("cannot spawn interactive supervisor: {error}"))?;
        drop(supervisor);
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stderr_capture = Arc::new(Mutex::new(Vec::new()));
        if let Some(mut stderr_pipe) = stderr {
            let capture = stderr_capture.clone();
            // The stderr drain reads until EOF — the preview bound only
            // limits what is retained, never how long the pipe is drained,
            // so a server that keeps writing stderr can never die on a full
            // pipe while it continues operating. The bounded preview is
            // published incrementally so it is observable before EOF.
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    match stderr_pipe.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            let remaining = MAX_PROCESS_OUTPUT_BYTES.saturating_sub(bytes.len());
                            bytes.extend_from_slice(&buffer[..count.min(remaining)]);
                            capture
                                .lock()
                                .expect("interactive stderr lock")
                                .clone_from(&bytes);
                        }
                    }
                }
                *capture.lock().expect("interactive stderr lock") = bytes;
            });
        }
        let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel(1);
        let settlement = Arc::new(SettlementCell::new());
        let settlement_for_driver = settlement.clone();
        #[cfg(test)]
        let supervisor_child_pid = child.id().expect("the spawned supervisor child has a pid");
        // The runtime-owned driver is the physical settlement owner from
        // the moment the spawn succeeded. Dropping the JoinHandle detaches
        // it; the driver completes the terminal exchange and reaps the
        // direct supervisor child regardless of the business handles.
        std::mem::drop(tokio::spawn(drive_interactive_unit(
            child,
            listener,
            socket_path,
            shutdown_rx,
            settlement_for_driver,
            test_control,
        )));
        Ok(Self {
            stdin,
            stdout,
            stderr_preview: stderr_capture,
            settlement,
            shutdown: Some(shutdown_tx),
            #[cfg(test)]
            supervisor_child_pid: Some(supervisor_child_pid),
        })
    }

    /// The bounded, lossily-decoded stderr preview observed so far.
    ///
    /// The drain task reads the pipe until EOF regardless of this bound, so
    /// reading this never affects the server's ability to keep writing.
    pub(crate) fn stderr_preview(&self) -> String {
        let bytes = self.stderr_preview.lock().expect("interactive stderr lock");
        String::from_utf8_lossy(&bytes).trim().to_owned()
    }

    /// Requests orderly server retirement without using the business stdin.
    pub(crate) fn request_shutdown(&self) {
        if let Some(shutdown) = &self.shutdown {
            let _ = shutdown.try_send(());
        }
    }

    /// Waits until the detached supervisor driver published the unit's one
    /// settlement.
    ///
    /// `Ok(())` is the physical settlement: the owned process tree is
    /// provably terminal and the direct supervisor child was reaped. An
    /// unproven terminal state — an unavailable emergency anchor, a
    /// containment failure, a containment-task failure, or a failed
    /// direct-child reap — is returned as an explicit error and is never
    /// represented as a successful physical settlement.
    ///
    /// # Errors
    ///
    /// Returns the reason the owned process tree could not be proven
    /// terminal.
    pub(crate) async fn wait_for_settlement(&self) -> Result<(), String> {
        let settlement = loop {
            let notified = self.settlement.notify.notified();
            if let Some(settlement) = self.settlement.observed() {
                break settlement;
            }
            notified.await;
        };
        match settlement {
            UnitSettlement::PhysicallySettled => Ok(()),
            UnitSettlement::TerminalityUnproven(reason) => Err(reason),
        }
    }
}

#[cfg(unix)]
impl Drop for SupervisedInteractiveProcess {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.try_send(());
        }
    }
}

/// The detached driver of one interactive supervisor unit: the single
/// physical settlement owner of the supervisor child.
#[cfg(unix)]
async fn drive_interactive_unit(
    mut child: tokio::process::Child,
    listener: tokio::net::UnixListener,
    socket_path: PathBuf,
    mut shutdown_rx: tokio::sync::mpsc::Receiver<()>,
    settlement: Arc<SettlementCell>,
    test_control: InteractiveTestControl,
) {
    let outcome = run_interactive_unit(
        &mut child,
        listener,
        &socket_path,
        &mut shutdown_rx,
        &test_control,
    )
    .await;
    let _ = std::fs::remove_file(&socket_path);
    settlement.publish(outcome);
}

/// The runtime-facing startup outcome of one supervisor unit.
#[cfg(unix)]
enum StartupOutcome {
    /// The outer control connection was accepted, retained, and gated open.
    Attached(tokio::net::UnixStream),
    /// Startup ended before any owned process tree could exist; the unit's
    /// settlement is already decided.
    Concluded(UnitSettlement),
}

/// The three distinguishable runtime-facing accept results.
#[cfg(unix)]
enum AcceptOutcome {
    /// `accept()` succeeded.
    Accepted(tokio::net::UnixStream),
    /// The outer child exited and was reaped by the same wait.
    OuterExitedAndReaped,
    /// The outer child exited but could not be reaped.
    ReapFailed(String),
    /// `accept()` failed while the outer supervisor may still exist. The
    /// child was **not** reaped by this outcome.
    AcceptFailed(String),
}

/// Establishes the runtime-facing control connection and opens the
/// runtime->outer startup gate.
///
/// The three post-spawn outcomes are strictly distinguished: a successful
/// accept, a reaped outer exit, and an accept error that leaves the outer
/// possibly alive. Only the second one may claim the child was reaped; the
/// third transfers into the explicit containment/reap path below.
#[cfg(unix)]
async fn establish_outer_control(
    child: &mut tokio::process::Child,
    listener: tokio::net::UnixListener,
    test_control: &InteractiveTestControl,
) -> StartupOutcome {
    #[cfg(test)]
    let forced_accept_failure = test_control
        .force_accept_failure
        .load(std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(test))]
    let forced_accept_failure = false;
    let outcome = if forced_accept_failure {
        // The injected seam models an accept error with a live, gated outer:
        // the listener is never polled, so the outer stays at its startup
        // gate and owns nothing.
        AcceptOutcome::AcceptFailed("injected interactive control accept failure".to_owned())
    } else {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((control, _)) => AcceptOutcome::Accepted(control),
                Err(error) => AcceptOutcome::AcceptFailed(format!(
                    "cannot accept the interactive control connection: {error}"
                )),
            },
            status = child.wait() => match status {
                Ok(_) => AcceptOutcome::OuterExitedAndReaped,
                Err(error) => AcceptOutcome::ReapFailed(format!(
                    "cannot reap the interactive supervisor that exited before connecting: {error}"
                )),
            },
        }
    };
    match outcome {
        AcceptOutcome::Accepted(mut control) => {
            // The startup gate is opened only now: the outer control
            // connection is accepted and retained by this driver.
            if let Err(error) = send_owner_attached(&mut control).await {
                return StartupOutcome::Concluded(contain_gated_outer(child, &error).await);
            }
            record_event(test_control, "owner_attached");
            StartupOutcome::Attached(control)
        }
        AcceptOutcome::OuterExitedAndReaped => {
            // The outer exited before connecting, so it never received the
            // startup gate and never created any part of the hierarchy; the
            // same wait reaped it. This is a proven pre-ownership
            // settlement.
            record_event(test_control, "outer_exited_before_connecting");
            StartupOutcome::Concluded(UnitSettlement::PhysicallySettled)
        }
        AcceptOutcome::ReapFailed(message) => {
            StartupOutcome::Concluded(UnitSettlement::TerminalityUnproven(message))
        }
        AcceptOutcome::AcceptFailed(message) => {
            record_event(test_control, "accept_failed");
            StartupOutcome::Concluded(contain_gated_outer(child, &message).await)
        }
    }
}

/// The explicit containment/reap path of a still-gated outer supervisor.
///
/// The runtime->outer startup gate is written only after a successful
/// accept, so a gated outer provably has not spawned the inner and no
/// server-owned process tree can exist. Terminating and reaping the direct
/// child is therefore the complete physical ownership of the unit: nothing
/// is claimed reaped that was not.
#[cfg(unix)]
async fn contain_gated_outer(child: &mut tokio::process::Child, cause: &str) -> UnitSettlement {
    // A start_kill error only means the child is already terminal; the
    // reap below is the authority.
    let _ = child.start_kill();
    match child.wait().await {
        Ok(_) => UnitSettlement::PhysicallySettled,
        Err(error) => UnitSettlement::TerminalityUnproven(format!(
            "{cause}; the gated interactive supervisor could not be reaped: {error}"
        )),
    }
}

/// Runs one interactive supervisor unit to its single settlement.
#[cfg(unix)]
#[allow(clippy::too_many_lines)] // one coherent accept/relay/contain/reap pipeline
async fn run_interactive_unit(
    child: &mut tokio::process::Child,
    listener: tokio::net::UnixListener,
    socket_path: &PathBuf,
    shutdown_rx: &mut tokio::sync::mpsc::Receiver<()>,
    test_control: &InteractiveTestControl,
) -> UnitSettlement {
    let control = match establish_outer_control(child, listener, test_control).await {
        StartupOutcome::Attached(control) => control,
        StartupOutcome::Concluded(settlement) => return settlement,
    };
    let _ = std::fs::remove_file(socket_path);
    let (mut control_read, mut control_write) = tokio::io::split(control);
    let mut lifecycle = UnitLifecycle::PreOwnership;
    let mut started = false;
    let mut control_failure: Option<String> = None;
    loop {
        tokio::select! {
            biased;
            command = shutdown_rx.recv(), if lifecycle != UnitLifecycle::Terminal => {
                if command.is_some() {
                    let () = send_terminate(&mut control_write).await;
                }
                // A dropped sender is not a shutdown request: the business
                // handle requests shutdown explicitly.
            }
            event = read_supervisor_event(&mut control_read) => match event {
                Ok(Some(SupervisorEvent::AnchorReady { pgid })) => {
                    record_event(test_control, "anchor_ready");
                    if pgid > 0 && lifecycle == UnitLifecycle::PreOwnership {
                        lifecycle = UnitLifecycle::OwnershipPossible { pgid };
                    } else if control_failure.is_none() {
                        control_failure =
                            Some("invalid interactive ownership anchor transition".to_owned());
                    }
                    if !started {
                        started = true;
                        if let Err(error) = send_start(&mut control_write).await
                            && control_failure.is_none()
                        {
                            control_failure = Some(error);
                        }
                    }
                }
                Ok(Some(SupervisorEvent::OwnershipEstablished)) => {
                    record_event(test_control, "ownership_established");
                    if let UnitLifecycle::OwnershipPossible { pgid } = lifecycle {
                        lifecycle = UnitLifecycle::Owned { pgid };
                    }
                }
                Ok(Some(SupervisorEvent::NoOwnership)) => {
                    record_event(test_control, "no_ownership");
                    // Setup ended before any owned process was spawned: the
                    // supervisor unit reaped its own pre-ownership child
                    // domain, so no owned process tree exists (M5 parity).
                    if matches!(
                        lifecycle,
                        UnitLifecycle::PreOwnership | UnitLifecycle::OwnershipPossible { .. }
                    ) {
                        lifecycle = UnitLifecycle::Terminal;
                    }
                }
                Ok(Some(
                    SupervisorEvent::SignalAttempt { .. } | SupervisorEvent::ShellExited { .. },
                )) => {}
                Ok(Some(SupervisorEvent::ProcessControlFailure { message })) => {
                    record_event(test_control, "process_control_failure");
                    if control_failure.is_none() {
                        control_failure = Some(message);
                    }
                }
                Ok(Some(SupervisorEvent::AllChildrenReaped)) => {
                    // The authoritative terminal event of the unit. The
                    // direct supervisor child is still reaped below before
                    // physical settlement is published.
                    record_event(test_control, "all_children_reaped");
                    lifecycle = UnitLifecycle::Terminal;
                    let () = send_terminal_ack(&mut control_write).await;
                    break;
                }
                Ok(None) => {
                    // Control EOF before the terminal event: the supervisor
                    // unit is lost. The owned group may still be live; the
                    // shared adopted-anchor emergency containment decides.
                    record_event(test_control, "control_eof");
                    if lifecycle != UnitLifecycle::Terminal && control_failure.is_none() {
                        control_failure =
                            Some("the interactive supervisor exited unexpectedly".to_owned());
                    }
                    break;
                }
                Err(error) => {
                    record_event(test_control, "control_error");
                    if control_failure.is_none() {
                        control_failure = Some(error);
                    }
                    break;
                }
            },
        }
    }
    // The direct supervisor child is reaped before physical settlement is
    // published, on every path.
    if let Err(error) = child.wait().await {
        return UnitSettlement::TerminalityUnproven(unproven_reason(
            &format!("cannot reap the direct interactive supervisor child: {error}"),
            control_failure.as_deref(),
        ));
    }
    match lifecycle {
        // The owned tree is provably terminal: the authoritative terminal
        // event, or a unit that never created an owned process tree.
        UnitLifecycle::Terminal | UnitLifecycle::PreOwnership => UnitSettlement::PhysicallySettled,
        UnitLifecycle::OwnershipPossible { pgid } | UnitLifecycle::Owned { pgid } => {
            emergency_settlement(pgid, control_failure.as_deref(), test_control).await
        }
    }
}

/// The catastrophic fallback settlement of a unit whose supervisor was lost
/// while an owned process tree could exist.
///
/// Only [`EmergencyContainment::TerminalProven`] settles the unit. An
/// unavailable anchor, a containment failure, and a containment-task
/// failure are process-control outcomes without a terminal proof, so they
/// are published as [`UnitSettlement::TerminalityUnproven`].
#[cfg(unix)]
async fn emergency_settlement(
    pgid: i32,
    control_failure: Option<&str>,
    test_control: &InteractiveTestControl,
) -> UnitSettlement {
    #[cfg(test)]
    let anchor_unavailable = test_control
        .force_emergency_anchor_unavailable
        .load(std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(test))]
    let anchor_unavailable = false;
    let contained =
        tokio::task::spawn_blocking(move || emergency_contain_group(pgid, anchor_unavailable))
            .await;
    match contained {
        Ok(Ok(EmergencyContainment::TerminalProven)) => {
            record_event(test_control, "emergency_terminal_proven");
            UnitSettlement::PhysicallySettled
        }
        Ok(Ok(EmergencyContainment::AnchorUnavailable)) => {
            // Anchor loss is never itself a terminal proof: the owned group
            // may still exist, so no physical settlement may be published.
            record_event(test_control, "emergency_anchor_unavailable");
            UnitSettlement::TerminalityUnproven(unproven_reason(
                "the emergency containment anchor is unavailable: the owned unit group's \
                 terminal state cannot be proven",
                control_failure,
            ))
        }
        Ok(Err(error)) => {
            record_event(test_control, "emergency_failed");
            UnitSettlement::TerminalityUnproven(unproven_reason(
                &format!("emergency containment failed: {error}"),
                control_failure,
            ))
        }
        Err(error) => {
            record_event(test_control, "emergency_task_failed");
            UnitSettlement::TerminalityUnproven(unproven_reason(
                &format!("the emergency containment task failed: {error}"),
                control_failure,
            ))
        }
    }
}

/// Composes the unproven-terminality reason, keeping the process-control
/// failure a separate, explicitly labelled fact.
#[cfg(unix)]
fn unproven_reason(reason: &str, control_failure: Option<&str>) -> String {
    control_failure.map_or_else(
        || reason.to_owned(),
        |failure| format!("{reason} (after the process-control failure: {failure})"),
    )
}

#[cfg(all(test, unix))]
mod interactive_tests {
    //! Deterministic regressions of the interactive supervisor unit's
    //! M5-equivalent physical ownership. Every test runs the real supervisor
    //! binary through [`SupervisedInteractiveProcess::spawn`] and uses
    //! marker/pid files with strict deadlock guards — never timing-based
    //! correctness assertions.

    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use super::{InteractiveProcessSpec, InteractiveTestControl, SupervisedInteractiveProcess};
    use crate::runtime::interactive_supervisor::{
        ANCHOR_PID_FILE_ENV, FAIL_SIGNAL_ENV, INNER_EXIT_BEFORE_CONNECT_ENV, OUTER_FAIL_ENV,
    };
    use crate::runtime::process_runner::MAX_PROCESS_OUTPUT_BYTES;

    const DEADLINE: Duration = Duration::from_secs(20);

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("fixture dir");
            Self { dir }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn spawn(
            &self,
            script: &str,
            extra_env: Vec<(String, String)>,
        ) -> Result<SupervisedInteractiveProcess, String> {
            self.spawn_with_control(script, extra_env, InteractiveTestControl::new())
        }

        fn spawn_with_control(
            &self,
            script: &str,
            extra_env: Vec<(String, String)>,
            control: InteractiveTestControl,
        ) -> Result<SupervisedInteractiveProcess, String> {
            let mut environment =
                vec![("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned())];
            environment.extend(extra_env);
            let spec = InteractiveProcessSpec {
                program: PathBuf::from("/bin/sh"),
                args: vec!["-c".to_owned(), script.to_owned()],
                cwd: self.dir.path().to_path_buf(),
                environment,
            };
            SupervisedInteractiveProcess::spawn_with_control(spec, control)
        }
    }

    fn wait_for_file(path: &Path, description: &str) {
        let deadline = Instant::now() + DEADLINE;
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "{description} never appeared: {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn read_pid(path: &Path) -> i32 {
        wait_for_file(path, "pid file");
        std::fs::read_to_string(path)
            .expect("pid file")
            .trim()
            .parse()
            .expect("pid")
    }

    fn proc_state(pid: i32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let close = stat.rfind(')')?;
        stat[close + 2..].chars().next()
    }

    fn wait_for_reaped(pid: i32, description: &str) {
        let deadline = Instant::now() + DEADLINE;
        while proc_state(pid).is_some() {
            assert!(
                Instant::now() < deadline,
                "{description} (pid {pid}) was never reaped"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    async fn settle(process: &mut SupervisedInteractiveProcess) {
        process.request_shutdown();
        tokio::time::timeout(DEADLINE, process.wait_for_settlement())
            .await
            .expect("the unit must settle")
            .expect("the unit must publish proven physical settlement");
    }

    /// Waits until the driver published its settlement and returns it.
    async fn published_settlement(
        process: &SupervisedInteractiveProcess,
        description: &str,
    ) -> Result<(), String> {
        tokio::time::timeout(DEADLINE, process.wait_for_settlement())
            .await
            .unwrap_or_else(|_| panic!("{description}"))
    }

    fn python_available() -> bool {
        ["/usr/local/bin/python3", "/usr/bin/python3", "/bin/python3"]
            .iter()
            .any(|path| Path::new(path).is_file())
    }

    /// Normal server shutdown: `request_shutdown` runs the TERM sequence and
    /// the whole unit settles.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn normal_server_shutdown_settles_the_unit() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let script = format!("echo started > {}; sleep 30", marker.display());
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&marker, "server start marker");
        settle(&mut process).await;
    }

    /// A server child that outlives its server parent stays owned: the unit
    /// does not settle while the in-group descendant lives, and shutdown
    /// terminates it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn server_child_outliving_server_parent_is_contained() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let child_pid_file = fixture.path("child.pid");
        let script = format!(
            "sleep 30 & echo $! > {}; echo started > {}; sleep 5",
            child_pid_file.display(),
            marker.display()
        );
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&marker, "server start marker");
        let child_pid = read_pid(&child_pid_file);
        settle(&mut process).await;
        wait_for_reaped(child_pid, "outliving server child");
    }

    /// Runs one in-server escape attempt and returns the three deterministic
    /// observations: the server really reached the syscall, the syscall was
    /// rejected with `EPERM`, and the post-escape marker was never written.
    ///
    /// The `reached` marker is what makes this non-vacuous: an absent escape
    /// marker only proves containment if the attempt actually executed.
    async fn assert_escape_is_denied(call: &str) {
        let fixture = Fixture::new();
        let reached = fixture.path("reached");
        let denied = fixture.path("denied");
        let escaped = fixture.path("escaped");
        // The server records that it reached the syscall, then classifies the
        // outcome: PermissionError (EPERM from the inherited fixed-membership
        // seccomp filter) versus a successful escape.
        let program = format!(
            "import os\n\
             open({reached:?}, 'w').close()\n\
             try:\n\
             \x20   os.{call}\n\
             except PermissionError:\n\
             \x20   open({denied:?}, 'w').close()\n\
             else:\n\
             \x20   open({escaped:?}, 'w').close()\n",
            reached = reached.display().to_string(),
            denied = denied.display().to_string(),
            escaped = escaped.display().to_string(),
        );
        let script = format!("python3 -c {}", shell_single_quote(&program));
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&reached, "escape attempt reached marker");
        settle(&mut process).await;
        assert!(
            denied.exists(),
            "{call} must be rejected with EPERM by the inherited membership filter"
        );
        assert!(
            !escaped.exists(),
            "{call} must never succeed: nothing may leave the owned group"
        );
    }

    /// Quotes one argument for `/bin/sh -c` as a single-quoted word.
    fn shell_single_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    /// A `setsid` escape attempt fails deterministically with EPERM (the
    /// shared fixed-membership restriction): the attempt provably runs, the
    /// syscall is denied, and nothing leaves the owned group.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn setsid_escape_attempt_fails_deterministically() {
        if !python_available() {
            eprintln!("python3 unavailable; setsid escape regression not exercised");
            return;
        }
        assert_escape_is_denied("setsid()").await;
    }

    /// A `setpgid` escape attempt fails deterministically with EPERM.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn setpgid_escape_attempt_fails_deterministically() {
        if !python_available() {
            eprintln!("python3 unavailable; setpgid escape regression not exercised");
            return;
        }
        assert_escape_is_denied("setpgid(0, 0)").await;
    }

    /// A TERM-resistant server is killed by the grace-period KILL, and the
    /// unit settles only after the owned group is terminal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn term_resistant_server_is_killed_after_the_grace_period() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let server_pid_file = fixture.path("server.pid");
        let script = format!(
            "trap '' TERM; echo $$ > {}; echo started > {}; sleep 30",
            server_pid_file.display(),
            marker.display()
        );
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&marker, "server start marker");
        let server_pid = read_pid(&server_pid_file);
        settle(&mut process).await;
        wait_for_reaped(server_pid, "TERM-resistant server after KILL");
    }

    /// Inner/supervisor control failure: the inner is killed while the
    /// server lives. The outer observes the abnormal anchor exit, issues
    /// the fallback containment while the anchor is retained, and the unit
    /// settles with the owned group terminal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inner_supervisor_loss_is_contained_by_the_outer() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let anchor_pid_file = fixture.path("anchor.pid");
        let server_pid_file = fixture.path("server.pid");
        let script = format!(
            "echo $$ > {}; echo started > {}; sleep 30",
            server_pid_file.display(),
            marker.display()
        );
        let process = fixture
            .spawn(
                &script,
                vec![(
                    ANCHOR_PID_FILE_ENV.to_owned(),
                    anchor_pid_file.display().to_string(),
                )],
            )
            .expect("spawn");
        wait_for_file(&marker, "server start marker");
        let server_pid = read_pid(&server_pid_file);
        let inner_pid = read_pid(&anchor_pid_file);
        // Kill the inner supervisor: the outer's dedicated anchor
        // observation sees the abnormal exit and performs the fallback
        // containment of the owned group.
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(inner_pid),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("kill the inner supervisor");
        published_settlement(&process, "the outer must contain and settle the unit")
            .await
            .expect("the contained unit must publish proven physical settlement");
        wait_for_reaped(server_pid, "server after inner-supervisor loss");
    }

    /// Dropping the business-facing handle requests shutdown and never
    /// abandons the physical process owner: the unit settles and the server
    /// is terminated.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_business_handle_requests_shutdown_and_settles() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let server_pid_file = fixture.path("server.pid");
        let script = format!(
            "echo $$ > {}; echo started > {}; sleep 30",
            server_pid_file.display(),
            marker.display()
        );
        let process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&marker, "server start marker");
        let server_pid = read_pid(&server_pid_file);
        drop(process);
        wait_for_reaped(server_pid, "server after business-handle drop");
    }

    /// A post-spawn handshake failure (the outer dies before connecting)
    /// settles the unit instead of stranding a raw child.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn post_spawn_handshake_failure_settles_without_stranding() {
        let fixture = Fixture::new();
        let process = fixture
            .spawn(
                "sleep 30",
                vec![(OUTER_FAIL_ENV.to_owned(), "1".to_owned())],
            )
            .expect("spawn");
        published_settlement(&process, "the driver must settle a handshake-failed unit")
            .await
            .expect("a gated outer that exited owns nothing: settlement is proven");
    }

    /// The direct supervisor child is reaped before physical settlement is
    /// published.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn direct_supervisor_child_is_reaped_before_settlement() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let script = format!("echo started > {}; sleep 30", marker.display());
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        wait_for_file(&marker, "server start marker");
        let supervisor_pid = i32::try_from(
            process
                .supervisor_child_pid
                .expect("test-only supervisor pid"),
        )
        .expect("pid fits i32");
        settle(&mut process).await;
        wait_for_reaped(supervisor_pid, "direct supervisor child");
    }

    /// stderr is drained until EOF even far beyond the bounded preview, so a
    /// server that floods stderr keeps operating; the retained preview stays
    /// bounded.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stderr_is_drained_beyond_the_bounded_preview() {
        let fixture = Fixture::new();
        let marker = fixture.path("operating");
        let script = format!(
            "i=0; while [ $i -lt 300 ]; do echo 'stderr flood line number {{{{i}}}} {}'; i=$((i+1)); done; echo operating > {}; sleep 30",
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            marker.display()
        );
        let mut process = fixture.spawn(&script, Vec::new()).expect("spawn");
        // The marker appears only if the stderr pipe keeps being drained
        // past the 64 KiB preview bound; a drain that stopped at the bound
        // would block the server on a full pipe forever.
        wait_for_file(&marker, "server continued operating marker");
        let preview_len = process.stderr_preview().len();
        assert!(
            preview_len <= MAX_PROCESS_OUTPUT_BYTES,
            "the retained stderr preview must stay bounded"
        );
        settle(&mut process).await;
    }

    /// An injected signaling failure escalates to containment: the unit
    /// still settles with the owned group terminal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn injected_signal_failure_escalates_to_containment() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let server_pid_file = fixture.path("server.pid");
        let script = format!(
            "echo $$ > {}; echo started > {}; sleep 30",
            server_pid_file.display(),
            marker.display()
        );
        let mut process = fixture
            .spawn(&script, vec![(FAIL_SIGNAL_ENV.to_owned(), "1".to_owned())])
            .expect("spawn");
        wait_for_file(&marker, "server start marker");
        let server_pid = read_pid(&server_pid_file);
        settle(&mut process).await;
        wait_for_reaped(server_pid, "server after injected signal failure");
    }

    /// An unavailable emergency anchor is never successful physical
    /// settlement.
    ///
    /// The outer supervisor is killed while the owned group is provably
    /// executing, so the driver escalates to the shared adopted-anchor
    /// emergency containment; the injected seam makes that containment
    /// report `AnchorUnavailable`. The driver must publish an explicit
    /// unproven-terminality settlement — never `PhysicallySettled` — and no
    /// process-group signal may be issued against the unproven numeric id.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unavailable_emergency_anchor_never_publishes_physical_settlement() {
        let fixture = Fixture::new();
        let marker = fixture.path("started");
        let anchor_pid_file = fixture.path("anchor.pid");
        let server_pid_file = fixture.path("server.pid");
        // `exec` replaces the shell, so the owned group holds exactly the
        // inner supervisor and one server process with known pids.
        let script = format!(
            "echo $$ > {}; echo started > {}; exec sleep 30",
            server_pid_file.display(),
            marker.display()
        );
        let control = InteractiveTestControl::new();
        control
            .force_emergency_anchor_unavailable
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let process = fixture
            .spawn_with_control(
                &script,
                vec![(
                    ANCHOR_PID_FILE_ENV.to_owned(),
                    anchor_pid_file.display().to_string(),
                )],
                control.clone(),
            )
            .expect("spawn");
        wait_for_file(&marker, "server start marker");
        let server_pid = read_pid(&server_pid_file);
        let inner_pid = read_pid(&anchor_pid_file);
        let outer_pid = i32::try_from(
            process
                .supervisor_child_pid
                .expect("test-only supervisor pid"),
        )
        .expect("pid fits i32");
        // Catastrophic supervisor loss with a live owned group: the driver
        // reaps the lost outer and escalates to emergency containment.
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(outer_pid),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("kill the outer supervisor");
        let settlement = published_settlement(
            &process,
            "the driver must publish an explicit settlement, never hang",
        )
        .await;
        let reason =
            settlement.expect_err("an unavailable emergency anchor is never a physical settlement");
        assert!(
            reason.contains("anchor is unavailable"),
            "the settlement must name the unproven terminal state: {reason}"
        );
        let events = control.observed_events();
        assert!(
            events
                .iter()
                .any(|event| event == "emergency_anchor_unavailable"),
            "the containment outcome must be the unavailable anchor: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| event == "emergency_terminal_proven"),
            "no terminal proof may be recorded: {events:?}"
        );
        // The owned group is provably still executing: the unproven state is
        // real, and it was never signalled through the lost anchor.
        assert!(
            proc_state(server_pid).is_some(),
            "the owned server must still exist when the anchor is unavailable"
        );
        // Test-side cleanup: the emergency path correctly consumed nothing,
        // so the test terminates the owned group and reaps the adopted
        // processes directly.
        nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(inner_pid),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("test terminates the owned group");
        nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(inner_pid), None)
            .expect("reap the adopted anchor");
        nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(server_pid), None)
            .expect("reap the adopted server");
    }

    /// The inner supervisor exits before connecting its control socket.
    ///
    /// The outer's pre-ownership state machine must observe the inner's
    /// exit instead of blocking forever on an accept that can never
    /// complete: the inner is reaped, the outcome is reported, the outer
    /// exits cleanly and is reaped, no server-owned tree ever exists, and
    /// the runtime reaches a proven pre-ownership settlement.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inner_exit_before_connecting_reaches_proven_pre_ownership_settlement() {
        let fixture = Fixture::new();
        let inner_pid_file = fixture.path("inner.pid");
        let server_marker = fixture.path("server-started");
        let script = format!("echo started > {}; sleep 30", server_marker.display());
        let control = InteractiveTestControl::new();
        let process = fixture
            .spawn_with_control(
                &script,
                vec![(
                    INNER_EXIT_BEFORE_CONNECT_ENV.to_owned(),
                    inner_pid_file.display().to_string(),
                )],
                control.clone(),
            )
            .expect("spawn");
        let outer_pid = i32::try_from(
            process
                .supervisor_child_pid
                .expect("test-only supervisor pid"),
        )
        .expect("pid fits i32");
        // No hang: the bounded ownership state machine settles the unit.
        published_settlement(
            &process,
            "a one-shot blocking inner accept would hang here forever",
        )
        .await
        .expect("a unit that never owned a process tree settles physically");
        // The inner wrote its pid before exiting; the outer reaped it.
        let inner_pid = read_pid(&inner_pid_file);
        wait_for_reaped(inner_pid, "the pre-ownership inner supervisor");
        wait_for_reaped(outer_pid, "the outer supervisor");
        let events = control.observed_events();
        assert!(
            events
                .iter()
                .any(|event| event == "process_control_failure"),
            "the pre-ownership inner loss is a process-control failure: {events:?}"
        );
        assert!(
            events.iter().any(|event| event == "no_ownership"),
            "the unit must report that no ownership was ever established: {events:?}"
        );
        assert!(
            !events.iter().any(|event| event == "anchor_ready"),
            "no unit anchor may exist: {events:?}"
        );
        // Everything the unit could have spawned is provably reaped, so the
        // absent server marker is conclusive: no server tree ever existed.
        assert!(
            !server_marker.exists(),
            "no server-owned process tree may escape the pre-ownership state"
        );
    }

    /// A runtime-facing accept failure never claims the outer was reaped.
    ///
    /// The startup gate is written only after a successful accept, so the
    /// injected accept failure leaves a live outer that owns nothing. The
    /// driver must transfer into the explicit containment/reap path: the
    /// gated outer is terminated and reaped, no hierarchy is ever created,
    /// and settlement is proven.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accept_failure_contains_and_reaps_the_gated_outer() {
        let fixture = Fixture::new();
        let server_marker = fixture.path("server-started");
        let script = format!("echo started > {}; sleep 30", server_marker.display());
        let control = InteractiveTestControl::new();
        control
            .force_accept_failure
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let process = fixture
            .spawn_with_control(&script, Vec::new(), control.clone())
            .expect("spawn");
        let outer_pid = i32::try_from(
            process
                .supervisor_child_pid
                .expect("test-only supervisor pid"),
        )
        .expect("pid fits i32");
        published_settlement(&process, "the accept failure must settle the unit")
            .await
            .expect("a contained gated outer is a proven physical settlement");
        wait_for_reaped(outer_pid, "the gated outer supervisor");
        let events = control.observed_events();
        assert!(
            events.iter().any(|event| event == "accept_failed"),
            "the accept failure must be the observed startup outcome: {events:?}"
        );
        assert!(
            !events.iter().any(|event| event == "owner_attached"),
            "the startup gate must never open on a failed accept: {events:?}"
        );
        assert!(
            !server_marker.exists(),
            "a gated outer may never create the unit hierarchy"
        );
    }
}
