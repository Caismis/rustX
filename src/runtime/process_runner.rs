//! The internal supervised command runner shared by native Bash and Skill
//! environment materialization.
//!
//! This module owns the rustX-side lifecycle of one **owned supervised
//! command**: the spawn of the per-invocation supervisor unit (the
//! `bash-supervisor` binary in its outer role), the control-channel protocol
//! (anchor gate, ownership commit, shell exit, all-children-reaped), the
//! `TERM` -> grace -> `KILL` cancellation/timeout settlement, the
//! catastrophic supervisor-loss containment, and the reaping of the direct
//! supervisor child. It is the same proven M5 Bash process-group lifecycle
//! extracted so a second production subprocess hierarchy (Skill environment
//! package managers) does not need a second, independent ownership domain.
//!
//! # Ownership contract (unchanged from M5)
//!
//! - every production supervised command executes inside one fixed
//!   rustX-owned process group created by the supervisor unit (`setsid`),
//!   with `setsid(2)`/`setpgid(2)` rejected by the inherited seccomp
//!   filter, so no command can escape the owned execution domain;
//! - the runtime child-subreaper capability is consulted lazily, one-time,
//!   idempotently and sticky, strictly before the supervisor unit spawns:
//!   `START` — which authorizes the command spawn — is never sent before
//!   catastrophic fallback authority exists;
//! - there is no generic `waitpid(-1)` process-wide reaper; catastrophic
//!   containment is invocation-scoped (retained anchor pid and invocation
//!   process group only);
//! - cancellation and the invocation deadline remain authoritative until
//!   the complete lifecycle settles: they trigger the supervisor's
//!   `TERM` -> grace -> `KILL` sequence, so a shell-parent exit can never
//!   let owned group work escape the timeout/cancellation contract;
//! - a terminal result is returned only after the owned process group is
//!   terminal and the direct supervisor child is reaped.
//!
//! # Two consumption shapes
//!
//! - [`SupervisedCommandRunner::spawn`] hands the child stdout/stderr pipes
//!   to the caller (native Bash) so the caller owns its streaming capture;
//! - [`run_supervised_command`] is the bounded convenience path (Skill
//!   environment materialization): it captures bounded output internally
//!   and returns it with the terminal outcome.
//!
//! The module implements no generic process-management framework beyond
//! these two real consumers.
//!
//! # Test seams
//!
//! [`RunnerTestControl`] is a `#[cfg(test)]`-only seam bundle mirroring the
//! M5 Bash seams; in non-test builds it is an uninhabited shell, so no
//! production behavior is affected.

use std::os::unix::io::OwnedFd;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::runtime::cancellation::CancellationSignal;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(test)]
use std::sync::{Arc, Mutex};

/// The supervisor role env var (mirrors `bash_supervisor::ROLE_OUTER`).
pub(crate) const SUPERVISOR_ROLE_ENV: &str = "RUSTX_SUPERVISOR_ROLE";
/// The outer supervisor role value (mirrors `bash_supervisor::ROLE_OUTER`).
pub(crate) const ROLE_OUTER: &str = "outer";
/// The command env var (mirrors `bash_supervisor::COMMAND_ENV`).
pub(crate) const COMMAND_ENV: &str = "RUSTX_SUPERVISOR_COMMAND";
/// The anchor-pid-file env var (mirrors `bash_supervisor::ANCHOR_PID_FILE_ENV`).
#[cfg(test)]
pub(crate) const ANCHOR_PID_FILE_ENV: &str = "RUSTX_SUPERVISOR_ANCHOR_PID_FILE";
/// The injected signal-failure env var (mirrors `bash_supervisor::FAIL_SIGNAL_ENV`).
#[cfg(test)]
pub(crate) const FAIL_SIGNAL_ENV: &str = "RUSTX_TEST_FAIL_SIGNAL";
/// The injected wait-failure env var (mirrors `bash_supervisor::FAIL_WAIT_ENV`).
#[cfg(test)]
pub(crate) const FAIL_WAIT_ENV: &str = "RUSTX_TEST_FAIL_WAIT";
/// The injected command-spawn-failure env var (mirrors `bash_supervisor::FAIL_BASH_SPAWN_ENV`).
#[cfg(test)]
pub(crate) const FAIL_COMMAND_SPAWN_ENV: &str = "RUSTX_TEST_FAIL_BASH_SPAWN";
/// The injected SIGTERM-handler-failure env var (mirrors `bash_supervisor::FAIL_SIGTERM_HANDLER_ENV`).
#[cfg(test)]
pub(crate) const FAIL_SIGTERM_HANDLER_ENV: &str = "RUSTX_TEST_FAIL_SIGTERM_HANDLER";
/// The injected anchor-loss env var (mirrors `bash_supervisor::FORCE_ANCHOR_LOSS_ENV`).
#[cfg(test)]
pub(crate) const FORCE_ANCHOR_LOSS_ENV: &str = "RUSTX_TEST_FORCE_ANCHOR_LOSS";

/// The bound on one captured output stream of the convenience runner.
///
/// The convenience path retains only bounded diagnostics; full output
/// streaming remains the consumer's concern (native Bash spools to the
/// artifact store).
pub(crate) const MAX_PROCESS_OUTPUT_BYTES: usize = 64 * 1024;

/// The explicit specification of one owned supervised command.
#[derive(Debug, Clone)]
pub(crate) struct SupervisedCommandSpec {
    /// The command executed by the owned `/bin/bash -c <command>` shell.
    pub command: String,
    /// The explicit working directory of the supervisor unit.
    pub cwd: PathBuf,
    /// The full explicit child environment (`env_clear()` + these entries).
    pub environment: Vec<(String, String)>,
    /// The finite invocation deadline; `None` means no deadline.
    pub timeout: Option<Duration>,
    /// The runtime cancellation signal owning the invocation.
    pub cancellation: CancellationSignal,
}

/// The settlement kind when cancellation/timeout owns the invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Settled {
    /// Attempt cancellation won the invocation.
    Cancelled,
    /// The invocation deadline expired.
    TimedOut,
}

/// The outcome intent of a settled invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProcessOutcomeIntent {
    /// The command completed naturally; `exit_status` carries its status.
    Completed,
    /// Attempt cancellation owned the settlement.
    Cancelled,
    /// The invocation deadline owned the settlement.
    TimedOut,
    /// A process-control/runtime failure owns the settlement.
    ProcessControlFailed(String),
}

/// The terminal outcome of one owned supervised command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessTermination {
    /// The shell's canonical exit status, when the shell exited naturally.
    pub exit_status: Option<ExitStatus>,
    /// The outcome intent owning the settlement.
    pub intent: ProcessOutcomeIntent,
}

/// A bounded captured supervised-command result (convenience path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedProcessResult {
    /// The shell exit code, when the shell exited naturally with a code.
    pub exit_code: Option<i32>,
    /// The outcome intent owning the settlement.
    pub intent: ProcessOutcomeIntent,
    /// The bounded captured stdout bytes.
    pub stdout: Vec<u8>,
    /// The bounded captured stderr bytes.
    pub stderr: Vec<u8>,
}

/// The internal non-model-facing supervised process runner boundary.
///
/// This is a real current boundary: native Bash and Skill environment
/// materialization consume the same owned process lifecycle, and
/// deterministic materialization tests inject scripted fakes instead of
/// invoking real package managers.
pub(crate) trait SupervisedProcessRunner: Send + Sync {
    /// Runs one owned supervised command to its terminal state.
    fn run(
        &self,
        spec: SupervisedCommandSpec,
        control: Option<RunnerTestControl>,
    ) -> BoxFuture<'_, Result<CapturedProcessResult, String>>;
}

/// The production supervised process runner backed by the shared runner.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RunnerBackedProcessRunner;

impl SupervisedProcessRunner for RunnerBackedProcessRunner {
    fn run(
        &self,
        spec: SupervisedCommandSpec,
        control: Option<RunnerTestControl>,
    ) -> BoxFuture<'_, Result<CapturedProcessResult, String>> {
        Box::pin(run_supervised_command(spec, control))
    }
}

/// A process-control failure of the owned invocation.
///
/// Supervisor setup, signaling, waiting, and IPC failures never silently
/// fail: a failure that undermines ownership or settlement surfaces as an
/// explicit failure, never as an ordinary success, cancellation, or timeout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProcessControlError {
    /// The supervisor exited before reporting terminal child ownership.
    UnexpectedSupervisorExit,
    /// The owned process tree did not become terminal within the bounded
    /// confirmation window after termination was requested.
    QuiescenceTimeout,
}

impl core::fmt::Display for ProcessControlError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnexpectedSupervisorExit => write!(
                f,
                "the bash supervisor exited before reporting terminal child ownership"
            ),
            Self::QuiescenceTimeout => {
                write!(f, "the owned bash process tree did not become terminal")
            }
        }
    }
}

impl std::error::Error for ProcessControlError {}

/// A supervised-command spawn failure, staged so consumers can map each
/// stage to their own diagnostic text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunnerSpawnError {
    /// The runtime child-subreaper capability could not be established.
    Subreaper(String),
    /// The supervisor control channel could not be created.
    ControlChannel(String),
    /// The control channel could not be prepared for tokio adoption.
    ControlChannelPrepare(String),
    /// The control channel could not be opened by tokio.
    ControlChannelOpen(String),
    /// The supervisor unit spawn failed.
    SupervisorSpawn(String),
    /// The test seam injected a supervisor spawn failure.
    #[cfg_attr(not(test), allow(dead_code))] // test-only failure injection
    InjectedSupervisorSpawn,
}

/// One event reported by the invocation supervisor over the control
/// channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SupervisorEvent {
    /// The inner completed setup and is gated before spawning the command.
    AnchorReady { pgid: i32 },
    /// The command was successfully spawned inside the fixed invocation
    /// group.
    OwnershipEstablished,
    /// Setup ended without ever spawning an owned execution domain.
    NoOwnership,
    /// The shell exited; `status` is its canonical exit status.
    ShellExited { status: ExitStatus },
    /// The supervisor's wait loop observed `ECHILD`: no owned child
    /// remains.
    AllChildrenReaped,
    /// A process-control failure; the message is human-readable.
    ProcessControlFailure { message: String },
    /// One attempted group signal (test observability).
    SignalAttempt {
        pgid: i32,
        signal: i32,
        emitted: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessLifecycle {
    PreOwnership,
    OwnershipPossible { pgid: i32 },
    Owned { pgid: i32 },
    Terminal,
}

impl ProcessLifecycle {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Terminal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SupervisorChannel {
    Connected,
    Lost,
}

/// One attempted process-group signal, recorded by the test seam.
///
/// `emitted` is `true` only when the signal actually reached the kernel
/// (`killpg` was invoked by the supervisor); a refused attempt (ownership
/// lost, injected failure) is recorded with `emitted == false`.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecordedSignal {
    /// The numeric process-group id the signal targeted.
    pub pgid: i32,
    /// The signal name (`SIGTERM`/`SIGKILL`).
    pub signal: &'static str,
    /// Whether the signal was actually emitted to the kernel.
    pub emitted: bool,
}

/// The exact command-exit lifecycle boundary of one invocation, observable
/// only by in-crate tests.
#[cfg_attr(not(test), allow(dead_code))] // empty in non-test builds
#[derive(Clone)]
pub(crate) struct RunnerLifecycleHook {
    #[cfg(test)]
    shell_exit_tx: tokio::sync::watch::Sender<bool>,
    #[cfg(test)]
    proceed_tx: tokio::sync::watch::Sender<bool>,
    #[cfg(test)]
    proceed_rx: tokio::sync::watch::Receiver<bool>,
}

#[cfg(test)]
impl RunnerLifecycleHook {
    pub(crate) fn new() -> Self {
        let (shell_exit_tx, _) = tokio::sync::watch::channel(false);
        let (proceed_tx, proceed_rx) = tokio::sync::watch::channel(false);
        Self {
            shell_exit_tx,
            proceed_tx,
            proceed_rx,
        }
    }

    /// Runner side: signals the exact shell-exit boundary and parks until
    /// the test releases the runner.
    async fn pause_after_shell_exit(&self) {
        let _ = self.shell_exit_tx.send(true);
        let mut proceed = self.proceed_rx.clone();
        let _ = proceed.changed().await;
    }

    /// Test side: waits until the runner provably observed the shell's
    /// natural exit and parked at the boundary.
    pub(crate) async fn await_shell_exit(&self) {
        let mut rx = self.shell_exit_tx.subscribe();
        if !*rx.borrow() {
            let _ = rx.changed().await;
        }
    }

    /// Test side: releases the parked runner.
    pub(crate) fn release(&self) {
        let _ = self.proceed_tx.send(true);
    }
}

/// Holds the authoritative terminal event outside the state machine so the
/// quiescence watchdog can expire while `children_terminal` remains false,
/// without relying on scheduler timing.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct RunnerTerminalHold {
    held_tx: tokio::sync::watch::Sender<bool>,
    held_rx: tokio::sync::watch::Receiver<bool>,
    watchdog_tx: tokio::sync::watch::Sender<bool>,
    watchdog_rx: tokio::sync::watch::Receiver<bool>,
    release_tx: tokio::sync::watch::Sender<bool>,
    release_rx: tokio::sync::watch::Receiver<bool>,
}

#[cfg(test)]
impl RunnerTerminalHold {
    pub(crate) fn new() -> Self {
        let (held_tx, held_rx) = tokio::sync::watch::channel(false);
        let (watchdog_tx, watchdog_rx) = tokio::sync::watch::channel(false);
        let (release_tx, release_rx) = tokio::sync::watch::channel(false);
        Self {
            held_tx,
            held_rx,
            watchdog_tx,
            watchdog_rx,
            release_tx,
            release_rx,
        }
    }

    pub(crate) async fn await_release(&self) {
        let mut rx = self.release_rx.clone();
        if !*rx.borrow() {
            let _ = rx.changed().await;
        }
    }

    pub(crate) async fn await_held(&self) {
        let mut rx = self.held_rx.clone();
        if !*rx.borrow() {
            let _ = rx.changed().await;
        }
    }

    pub(crate) async fn await_watchdog(&self) {
        let mut rx = self.watchdog_rx.clone();
        if !*rx.borrow() {
            let _ = rx.changed().await;
        }
    }

    pub(crate) fn release(&self) {
        let _ = self.release_tx.send(true);
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct RunnerChannelEofHook {
    seen_tx: tokio::sync::watch::Sender<bool>,
    seen_rx: tokio::sync::watch::Receiver<bool>,
    proceed_tx: tokio::sync::watch::Sender<bool>,
    proceed_rx: tokio::sync::watch::Receiver<bool>,
    timeout_tx: tokio::sync::watch::Sender<bool>,
    timeout_rx: tokio::sync::watch::Receiver<bool>,
}

#[cfg(test)]
impl RunnerChannelEofHook {
    pub(crate) fn new() -> Self {
        let (seen_tx, seen_rx) = tokio::sync::watch::channel(false);
        let (proceed_tx, proceed_rx) = tokio::sync::watch::channel(false);
        let (timeout_tx, timeout_rx) = tokio::sync::watch::channel(false);
        Self {
            seen_tx,
            seen_rx,
            proceed_tx,
            proceed_rx,
            timeout_tx,
            timeout_rx,
        }
    }

    pub(crate) async fn await_seen(&self) {
        let mut rx = self.seen_rx.clone();
        if !*rx.borrow() {
            let _ = rx.changed().await;
        }
    }

    pub(crate) fn release_emergency_containment(&self) {
        let _ = self.proceed_tx.send(true);
    }

    pub(crate) fn force_timeout(&self) {
        let _ = self.timeout_tx.send(true);
    }

    pub(crate) async fn pause_before_emergency(&self) {
        let mut rx = self.proceed_rx.clone();
        if !*rx.borrow() {
            let _ = rx.changed().await;
        }
    }
}

#[cfg(test)]
async fn wait_for_forced_timeout(control: Option<&RunnerTestControl>) {
    if let Some(hook) = control.and_then(|control| control.channel_eof.as_ref()) {
        let mut rx = hook.timeout_rx.clone();
        if !*rx.borrow() {
            let _ = rx.changed().await;
        }
    } else {
        std::future::pending::<()>().await;
    }
}

#[cfg(not(test))]
async fn wait_for_forced_timeout(_control: Option<&RunnerTestControl>) {
    std::future::pending::<()>().await;
}

#[cfg(test)]
async fn wait_for_terminal_release(control: Option<&RunnerTestControl>) {
    if let Some(hold) = control.and_then(|control| control.terminal_hold.as_ref()) {
        hold.await_release().await;
    } else {
        std::future::pending::<()>().await;
    }
}

#[cfg(not(test))]
async fn wait_for_terminal_release(_control: Option<&RunnerTestControl>) {
    std::future::pending::<()>().await;
}

/// The `TERMINATE` control-message kind (mirrors
/// `bash_supervisor::MSG_TERMINATE`).
#[cfg(unix)]
const MSG_TERMINATE: u8 = 0x10;
#[cfg(unix)]
const MSG_START: u8 = 0x11;
#[cfg(unix)]
const MSG_TERMINAL_ACK: u8 = 0x12;

/// The supervisor event kinds (mirror `bash_supervisor`).
#[cfg(unix)]
const MSG_SHELL_EXITED: u8 = 0x02;
#[cfg(unix)]
pub(crate) const MSG_ALL_CHILDREN_REAPED: u8 = 0x03;
#[cfg(unix)]
const MSG_PROCESS_CONTROL_FAILURE: u8 = 0x04;
#[cfg(unix)]
const MSG_SIGNAL_ATTEMPT: u8 = 0x05;
#[cfg(unix)]
const MSG_ANCHOR_READY: u8 = 0x06;
#[cfg(unix)]
const MSG_OWNERSHIP_ESTABLISHED: u8 = 0x07;
#[cfg(unix)]
const MSG_NO_OWNERSHIP: u8 = 0x08;

/// The test-only control seams of one owned invocation.
///
/// In non-test builds this type is an empty shell, so no production
/// behavior is affected. The seams exist so in-crate regressions can
/// observe the exact shell-exit boundary, deterministically inject
/// supervisor setup / wait / signal / command-spawn failures, model the
/// ownership release transition, record every process-group signal attempt,
/// and locate the invocation's process-group id — without an operating
/// system mocking framework.
#[cfg_attr(test, allow(clippy::struct_excessive_bools))] // a bounded test-seam bundle
#[derive(Clone)]
pub(crate) struct RunnerTestControl {
    #[cfg(test)]
    pub(crate) pause_at_shell_exit: bool,
    #[cfg(test)]
    pub(crate) lifecycle: RunnerLifecycleHook,
    #[cfg(test)]
    pub(crate) fail_supervisor_spawn: bool,
    #[cfg(test)]
    pub(crate) fail_command_spawn: bool,
    #[cfg(test)]
    pub(crate) fail_signal: bool,
    #[cfg(test)]
    pub(crate) fail_wait: bool,
    #[cfg(test)]
    pub(crate) fail_sigterm_handler: bool,
    #[cfg(test)]
    pub(crate) fail_subreaper_init: bool,
    #[cfg(test)]
    pub(crate) force_anchor_loss: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    pub(crate) force_emergency_anchor_unavailable: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    pub(crate) anchor_pid_file: Option<std::path::PathBuf>,
    #[cfg(test)]
    pub(crate) recorded_signals: Arc<Mutex<Vec<RecordedSignal>>>,
    #[cfg(test)]
    pub(crate) terminal_hold: Option<RunnerTerminalHold>,
    #[cfg(test)]
    pub(crate) channel_eof: Option<RunnerChannelEofHook>,
}

impl RunnerTestControl {
    /// A control bundle without failures.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self {
            pause_at_shell_exit: false,
            lifecycle: RunnerLifecycleHook::new(),
            fail_supervisor_spawn: false,
            fail_command_spawn: false,
            fail_signal: false,
            fail_wait: false,
            fail_sigterm_handler: false,
            fail_subreaper_init: false,
            force_anchor_loss: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            force_emergency_anchor_unavailable: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            anchor_pid_file: None,
            recorded_signals: Arc::new(Mutex::new(Vec::new())),
            terminal_hold: None,
            channel_eof: None,
        }
    }

    /// A control bundle without failures (non-test build: fieldless shell).
    #[cfg_attr(not(test), allow(dead_code))] // test-only seams
    #[must_use]
    #[cfg(not(test))]
    pub(crate) fn new() -> Self {
        Self {}
    }

    #[cfg(test)]
    pub(crate) fn record_signal(&self, recorded: RecordedSignal) {
        self.recorded_signals
            .lock()
            .expect("recorded signals lock")
            .push(recorded);
    }

    /// The recorded process-group signal attempts so far.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn recorded_signals(&self) -> Vec<RecordedSignal> {
        self.recorded_signals
            .lock()
            .expect("recorded signals lock")
            .clone()
    }
}

/// The owned supervised-command runner.
///
/// [`SupervisedCommandRunner::spawn`] spawns the supervisor unit and hands
/// the child stdout/stderr pipes to the caller; [`SupervisedCommandRunner::settle`]
/// drives the lifecycle to its terminal state and returns the outcome
/// intent. The child is reaped by the runner.
pub(crate) struct SupervisedCommandRunner {
    child: tokio::process::Child,
    stream: tokio::net::UnixStream,
    cancellation: CancellationSignal,
    timeout: Option<Duration>,
    start: tokio::time::Instant,
    lifecycle: ProcessLifecycle,
    supervisor_channel: SupervisorChannel,
    direct_child_reaped: bool,
    failure: Option<String>,
    settled: Option<Settled>,
    exit_status: Option<ExitStatus>,
    terminate_sent: bool,
    terminate_deadline: Option<tokio::time::Instant>,
    terminal_event_held: bool,
    control: Option<RunnerTestControl>,
}

impl SupervisedCommandRunner {
    /// Spawns the supervisor unit for one owned command.
    ///
    /// The runtime child-subreaper capability is a pre-ownership
    /// prerequisite, consulted lazily, one-time, idempotently (see
    /// `crate::runtime::process_supervision`) before the supervisor unit
    /// spawns, so `START` — which authorizes the command spawn — is never
    /// sent before catastrophic fallback authority exists.
    ///
    /// The returned pipes are the supervisor unit's stdout/stderr (the
    /// owned command's streams); the caller owns their capture. The runner
    /// owns the supervisor child.
    #[cfg(unix)]
    #[allow(clippy::too_many_lines)] // one coherent spawn/settle pipeline
    pub(crate) fn spawn(
        spec: &SupervisedCommandSpec,
        control: Option<RunnerTestControl>,
    ) -> Result<
        (
            Self,
            Option<tokio::process::ChildStdout>,
            Option<tokio::process::ChildStderr>,
        ),
        RunnerSpawnError,
    > {
        #[cfg(not(test))]
        let _ = control;
        #[cfg(test)]
        if let Some(control) = &control {
            if control.fail_subreaper_init {
                return Err(RunnerSpawnError::Subreaper(
                    "injected child-subreaper initialization failure".to_owned(),
                ));
            }
        }
        if let Err(error) = crate::runtime::process_supervision::ensure_child_subreaper() {
            return Err(RunnerSpawnError::Subreaper(error));
        }
        let (stream_a, stream_b) = std::os::unix::net::UnixStream::pair()
            .map_err(|error| RunnerSpawnError::ControlChannel(error.to_string()))?;

        let mut supervisor = tokio::process::Command::new(supervisor_binary());
        supervisor.current_dir(&spec.cwd);
        supervisor.env_clear();
        for (key, value) in &spec.environment {
            supervisor.env(key, value);
        }
        supervisor.env(SUPERVISOR_ROLE_ENV, ROLE_OUTER);
        supervisor.env(COMMAND_ENV, &spec.command);
        #[cfg(test)]
        if let Some(control) = &control {
            if let Some(path) = &control.anchor_pid_file {
                supervisor.env(ANCHOR_PID_FILE_ENV, path);
            }
            if control.fail_signal {
                supervisor.env(FAIL_SIGNAL_ENV, "1");
            }
            if control.fail_wait {
                supervisor.env(FAIL_WAIT_ENV, "1");
            }
            if control.fail_command_spawn {
                supervisor.env(FAIL_COMMAND_SPAWN_ENV, "1");
            }
            if control.fail_sigterm_handler {
                supervisor.env(FAIL_SIGTERM_HANDLER_ENV, "1");
            }
            if control
                .force_anchor_loss
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                supervisor.env(FORCE_ANCHOR_LOSS_ENV, "1");
            }
        }
        supervisor.stdin(Stdio::from(OwnedFd::from(stream_b)));
        supervisor.stdout(Stdio::piped());
        supervisor.stderr(Stdio::piped());
        #[cfg(test)]
        if let Some(control) = &control {
            if control.fail_supervisor_spawn {
                return Err(RunnerSpawnError::InjectedSupervisorSpawn);
            }
        }
        let mut child = match supervisor.spawn() {
            Ok(child) => child,
            Err(error) => {
                return Err(RunnerSpawnError::SupervisorSpawn(error.to_string()));
            }
        };
        // The reusable Command retains its configured child-side stdio
        // handle. Drop it after the one spawn so supervisor death is
        // observable as EOF.
        drop(supervisor);
        if let Err(error) = nix::fcntl::fcntl(
            &stream_a,
            nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
        ) {
            let _ = child.start_kill();
            std::mem::drop(child.wait());
            return Err(RunnerSpawnError::ControlChannelPrepare(error.to_string()));
        }
        let stream = match tokio::net::UnixStream::from_std(stream_a) {
            Ok(stream) => stream,
            Err(error) => {
                let _ = child.start_kill();
                std::mem::drop(child.wait());
                return Err(RunnerSpawnError::ControlChannelOpen(error.to_string()));
            }
        };
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        Ok((
            Self {
                child,
                stream,
                cancellation: spec.cancellation.clone(),
                timeout: spec.timeout,
                start: tokio::time::Instant::now(),
                lifecycle: ProcessLifecycle::PreOwnership,
                supervisor_channel: SupervisorChannel::Connected,
                direct_child_reaped: false,
                failure: None,
                settled: None,
                exit_status: None,
                terminate_sent: false,
                terminate_deadline: None,
                terminal_event_held: false,
                control,
            },
            stdout_pipe,
            stderr_pipe,
        ))
    }

    /// Drives the invocation lifecycle to its terminal state.
    ///
    /// The outcome intent and lifecycle settlement are distinct: the loop
    /// may return only when an outcome intent (failure, cancellation/
    /// timeout, or the shell's natural status) is known and the owned child
    /// set is terminal, and the direct supervisor child has been reaped.
    /// Shell-parent exit is not settlement, and neither is a detected
    /// failure: owned work must be contained and terminal before any
    /// outcome — success, failure, cancelled, or timed out — is produced.
    #[cfg(unix)]
    #[allow(clippy::too_many_lines)] // one coherent spawn/settle pipeline
    pub(crate) async fn settle(&mut self) -> ProcessTermination {
        loop {
            let outcome_intent =
                self.failure.is_some() || self.settled.is_some() || self.exit_status.is_some();
            if outcome_intent && self.lifecycle.is_terminal() {
                break;
            }
            if self.terminate_sent
                && !self.lifecycle.is_terminal()
                && self
                    .terminate_deadline
                    .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
            {
                self.failure = Some(ProcessControlError::QuiescenceTimeout.to_string());
                self.terminate_deadline = None;
                continue;
            }
            tokio::select! {
                biased;
                () = self.cancellation.cancelled(), if self.settled.is_none() && !self.terminate_sent => {
                    self.settled = Some(Settled::Cancelled);
                    send_terminate(&mut self.stream).await;
                    self.terminate_sent = true;
                    self.terminate_deadline = Some(tokio::time::Instant::now() + BASH_TERMINATION_CONFIRMATION);
                }
                () = async {
                    match self.timeout {
                        Some(timeout) => tokio::time::sleep_until(self.start + timeout).await,
                        None => std::future::pending().await,
                    }
                }, if self.settled.is_none() && !self.terminate_sent => {
                    self.settled = Some(Settled::TimedOut);
                    send_terminate(&mut self.stream).await;
                    self.terminate_sent = true;
                    self.terminate_deadline = Some(tokio::time::Instant::now() + BASH_TERMINATION_CONFIRMATION);
                }
                () = wait_for_forced_timeout(self.control.as_ref()), if self.settled.is_none() && !self.terminate_sent => {
                    self.settled = Some(Settled::TimedOut);
                    send_terminate(&mut self.stream).await;
                    self.terminate_sent = true;
                    self.terminate_deadline = Some(tokio::time::Instant::now() + BASH_TERMINATION_CONFIRMATION);
                }
                event = read_supervisor_event(&mut self.stream), if self.supervisor_channel == SupervisorChannel::Connected => match event {
                    Ok(Some(SupervisorEvent::AnchorReady { pgid })) => {
                        if pgid <= 0 || self.lifecycle != ProcessLifecycle::PreOwnership {
                            self.failure = Some("invalid Bash ownership anchor transition".to_owned());
                        } else {
                            self.lifecycle = ProcessLifecycle::OwnershipPossible { pgid };
                            if let Err(error) = send_start(&mut self.stream).await {
                                self.failure = Some(error);
                            }
                        }
                    }
                    Ok(Some(SupervisorEvent::OwnershipEstablished)) => {
                        if let ProcessLifecycle::OwnershipPossible { pgid } = self.lifecycle {
                            self.lifecycle = ProcessLifecycle::Owned { pgid };
                        } else {
                            self.failure = Some("invalid Bash ownership commit transition".to_owned());
                        }
                    }
                    Ok(Some(SupervisorEvent::NoOwnership)) => {
                        if matches!(self.lifecycle, ProcessLifecycle::PreOwnership | ProcessLifecycle::OwnershipPossible { .. }) {
                            self.lifecycle = ProcessLifecycle::Terminal;
                        } else {
                            self.failure = Some("invalid no-ownership terminal transition".to_owned());
                        }
                    }
                    Ok(Some(SupervisorEvent::ShellExited { status })) => {
                        self.exit_status = Some(status);
                        #[cfg(test)]
                        if let Some(control) = self.control.as_ref() {
                            if control.pause_at_shell_exit && self.settled.is_none() && !self.terminate_sent {
                                control.lifecycle.pause_after_shell_exit().await;
                            }
                        }
                    }
                    Ok(Some(SupervisorEvent::AllChildrenReaped)) => {
                        send_terminal_ack(&mut self.stream).await;
                        #[cfg(test)]
                        if let Some(hold) = self.control.as_ref().and_then(|control| control.terminal_hold.as_ref()) {
                            let _ = hold.held_tx.send(true);
                            self.terminal_event_held = true;
                        } else {
                            self.lifecycle = ProcessLifecycle::Terminal;
                        }
                        #[cfg(not(test))]
                        {
                            self.lifecycle = ProcessLifecycle::Terminal;
                        }
                    }
                    Ok(Some(SupervisorEvent::ProcessControlFailure { message })) => {
                        self.failure = Some(message);
                    }
                    Ok(Some(SupervisorEvent::SignalAttempt { pgid, signal, emitted })) => {
                        #[cfg(test)]
                        if let Some(control) = self.control.as_ref() {
                            use nix::sys::signal::Signal;
                            let signal_name = Signal::try_from(signal)
                                .map_or("unknown", Signal::as_str);
                            control.record_signal(RecordedSignal {
                                pgid,
                                signal: signal_name,
                                emitted,
                            });
                        }
                        #[cfg(not(test))]
                        let _ = (pgid, signal, emitted);
                    }
                    Ok(None) => {
                        #[cfg(test)]
                        if let Some(hook) = self.control.as_ref().and_then(|control| control.channel_eof.as_ref()) {
                            let _ = hook.seen_tx.send(true);
                        }
                        self.supervisor_channel = SupervisorChannel::Lost;
                        if self.terminal_event_held {
                            continue;
                        }
                        if !self.lifecycle.is_terminal() {
                            self.failure = Some(ProcessControlError::UnexpectedSupervisorExit.to_string());
                        }
                        match self.lifecycle {
                            ProcessLifecycle::PreOwnership => {
                                self.lifecycle = ProcessLifecycle::Terminal;
                            }
                            ProcessLifecycle::OwnershipPossible { pgid }
                            | ProcessLifecycle::Owned { pgid } => {
                                if self
                                    .emergency_containment_after_supervisor_loss(pgid)
                                    .await
                                {
                                    self.direct_child_reaped = true;
                                }
                            }
                            ProcessLifecycle::Terminal => {}
                        }
                    }
                    Err(error) => {
                        #[cfg(test)]
                        if let Some(hook) = self.control.as_ref().and_then(|control| control.channel_eof.as_ref()) {
                            let _ = hook.seen_tx.send(true);
                        }
                        self.failure = Some(error);
                        self.supervisor_channel = SupervisorChannel::Lost;
                        if self.terminal_event_held {
                            continue;
                        }
                        match self.lifecycle {
                            ProcessLifecycle::PreOwnership => {
                                self.lifecycle = ProcessLifecycle::Terminal;
                            }
                            ProcessLifecycle::OwnershipPossible { pgid }
                            | ProcessLifecycle::Owned { pgid } => {
                                if self
                                    .emergency_containment_after_supervisor_loss(pgid)
                                    .await
                                {
                                    self.direct_child_reaped = true;
                                }
                            }
                            ProcessLifecycle::Terminal => {}
                        }
                    }
                },
                () = wait_for_terminal_release(self.control.as_ref()), if self.terminal_event_held => {
                    self.terminal_event_held = false;
                    self.lifecycle = ProcessLifecycle::Terminal;
                }
                () = async {
                    match self.terminate_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                }, if self.terminate_sent && !self.lifecycle.is_terminal() && self.terminate_deadline.is_some() => {
                    self.failure = Some(ProcessControlError::QuiescenceTimeout.to_string());
                    self.terminate_deadline = None;
                    #[cfg(test)]
                    if let Some(hold) = self.control.as_ref().and_then(|control| control.terminal_hold.as_ref()) {
                        let _ = hold.watchdog_tx.send(true);
                    }
                }
            }
        }

        // Process terminality was proven by the outer supervisor before this
        // point. Reaping the already-terminal direct child is semantically
        // required; it is never abandoned.
        if !self.direct_child_reaped
            && let Err(error) = self.child.wait().await
        {
            self.failure = Some(format!(
                "cannot reap the terminal supervisor child: {error}"
            ));
        }

        let intent = if let Some(failure_message) = self.failure.take() {
            ProcessOutcomeIntent::ProcessControlFailed(failure_message)
        } else if let Some(settled) = self.settled {
            match settled {
                Settled::Cancelled => ProcessOutcomeIntent::Cancelled,
                Settled::TimedOut => ProcessOutcomeIntent::TimedOut,
            }
        } else {
            ProcessOutcomeIntent::Completed
        };
        ProcessTermination {
            exit_status: self.exit_status,
            intent,
        }
    }

    /// The catastrophic emergency path of an owned invocation whose
    /// supervisor unit was lost: reaps the lost outer supervisor and runs
    /// the adopted-anchor containment.
    ///
    /// Returns whether the direct outer supervisor was reaped.
    #[cfg(unix)]
    async fn emergency_containment_after_supervisor_loss(&mut self, pgid: i32) -> bool {
        #[cfg(test)]
        let control = self.control.clone();
        if let Err(error) = self.child.wait().await {
            self.failure = Some(format!("cannot reap the lost outer supervisor: {error}"));
            return false;
        }
        #[cfg(test)]
        if let Some(hook) = control
            .as_ref()
            .and_then(|control| control.channel_eof.as_ref())
        {
            hook.pause_before_emergency().await;
        }
        #[cfg(test)]
        let anchor_unavailable = control.is_some_and(|control| {
            control
                .force_emergency_anchor_unavailable
                .load(std::sync::atomic::Ordering::SeqCst)
        });
        #[cfg(not(test))]
        let anchor_unavailable = false;
        match tokio::task::spawn_blocking(move || emergency_contain_group(pgid, anchor_unavailable))
            .await
        {
            Ok(Ok(EmergencyContainment::TerminalProven)) => {
                self.lifecycle = ProcessLifecycle::Terminal;
            }
            Ok(Ok(EmergencyContainment::AnchorUnavailable)) => {
                // Anchor loss is never itself a terminal process-group
                // proof: the owned group may still exist. The lifecycle
                // remains non-terminal and the already-recorded failure
                // intent cannot commit an outcome.
            }
            Ok(Err(error)) => self.failure = Some(error),
            Err(error) => {
                self.failure = Some(format!("emergency containment task failed: {error}"));
            }
        }
        true
    }
}

/// The bounded confirmation window of the owned process tree.
///
/// Mirrors `crate::tools::limits::BASH_TERMINATION_CONFIRMATION`; the
/// runner lives below the tools layer, so the value is mirrored here.
#[cfg(unix)]
const BASH_TERMINATION_CONFIRMATION: Duration = Duration::from_secs(6);

/// The explicit outcome of catastrophic emergency containment.
///
/// `Ok(())` alone would be ambiguous (contained-and-terminal vs. no anchor
/// vs. normal path already completed), so the result distinguishes the
/// terminal proof from the unavailable-anchor state:
///
/// - [`EmergencyContainment::TerminalProven`]: the anchor was retained,
///   the fallback signal was issued while retained, the group-scoped wait
///   reached `ECHILD`, and the anchor was released — the owned invocation
///   group is terminal.
/// - [`EmergencyContainment::AnchorUnavailable`]: the anchor is not a
///   waitable child of rustX without a prior authoritative terminal event.
///   This is **not** a terminal proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmergencyContainment {
    /// The anchor was retained, the fallback signal was issued while that
    /// identity was retained, the group-scoped wait reached `ECHILD`, and
    /// the anchor was then released.
    TerminalProven,
    /// The invocation anchor is unavailable without a prior authoritative
    /// terminal proof. Never a terminal result.
    AnchorUnavailable,
}

/// Catastrophic fallback after the outer supervisor has been reaped.
///
/// rustX is a subreaper, so the dead outer's invocation descendants are now
/// rustX children. The inner leader is retained with `WNOWAIT` before its
/// numeric identity is used for `killpg`; this is the same ABA-proof anchor
/// used by the normal outer path. Only after the group-scoped child wait
/// reaches `ECHILD` is the anchor identity released and terminality proven.
///
/// The anchor is matched only by pid; the invocation group only by its
/// retained pgid. No broad wait (`waitpid(-1)`, `waitid(P_ALL)`) exists
/// here, so unrelated adopted children are never consumed.
#[cfg(target_os = "linux")]
fn emergency_contain_group(
    pgid: i32,
    anchor_unavailable: bool,
) -> Result<EmergencyContainment, String> {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, killpg};
    use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
    use nix::unistd::Pid;
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
            Ok(WaitStatus::StillAlive) => std::thread::sleep(Duration::from_millis(20)),
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
                return Err(format!("cannot retain the lost invocation anchor: {error}"));
            }
        }
    }

    match killpg(anchor, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(error) => {
            return Err(format!("cannot contain the lost invocation group: {error}"));
        }
    }
    loop {
        // The group-scoped terminal proof: no adopted child of rustX
        // remains in the invocation group. The anchor itself is released
        // (reaped) by this same wait, strictly after the fallback signal.
        match waitid(
            Id::PGid(anchor),
            WaitPidFlag::WNOHANG | WaitPidFlag::WEXITED,
        ) {
            Ok(WaitStatus::StillAlive) => std::thread::sleep(Duration::from_millis(20)),
            Ok(_) | Err(Errno::EINTR) => {}
            Err(Errno::ECHILD) => return Ok(EmergencyContainment::TerminalProven),
            Err(error) => {
                return Err(format!(
                    "cannot prove the lost invocation group terminal: {error}"
                ));
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn emergency_contain_group(
    _pgid: i32,
    _anchor_unavailable: bool,
) -> Result<EmergencyContainment, String> {
    Err("fallback containment requires Linux PR_SET_CHILD_SUBREAPER".to_owned())
}

/// The dedicated supervisor binary: `CARGO_BIN_EXE` when cargo provides it
/// (integration tests), otherwise the `bash-supervisor` sibling of the
/// current executable (production), otherwise the binary-directory sibling
/// of a test binary living under `target/debug/deps` (in-crate tests).
#[cfg(unix)]
pub(crate) fn supervisor_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_bash-supervisor") {
        return PathBuf::from(path);
    }
    let exe = std::env::current_exe().expect("current executable");
    let sibling = exe
        .parent()
        .expect("current executable directory")
        .join("bash-supervisor");
    if sibling.exists() {
        return sibling;
    }
    exe.parent()
        .expect("current executable directory")
        .parent()
        .expect("binary directory")
        .join("bash-supervisor")
}

/// Sends the one termination request to the invocation supervisor.
///
/// A write failure means the supervisor is already gone, which implies its
/// terminal child-set events are already in flight or were received; the
/// supervision loop's read side remains authoritative.
#[cfg(unix)]
async fn send_terminate(stream: &mut tokio::net::UnixStream) {
    let frame = [1u8, 0, 0, 0, MSG_TERMINATE];
    let _ = stream.write_all(&frame).await;
}

#[cfg(unix)]
async fn send_start(stream: &mut tokio::net::UnixStream) -> Result<(), String> {
    let frame = [1u8, 0, 0, 0, MSG_START];
    stream
        .write_all(&frame)
        .await
        .map_err(|error| format!("cannot acknowledge the ownership gate: {error}"))
}

#[cfg(unix)]
async fn send_terminal_ack(stream: &mut tokio::net::UnixStream) {
    let frame = [1u8, 0, 0, 0, MSG_TERMINAL_ACK];
    let _ = stream.write_all(&frame).await;
}

/// Reads exactly one supervisor control frame:
/// `[u32 LE length][kind][payload]`.
///
/// `Ok(None)` means the control channel reached EOF: the supervisor exited.
#[cfg(unix)]
pub(crate) async fn read_supervisor_event(
    stream: &mut tokio::net::UnixStream,
) -> Result<Option<SupervisorEvent>, String> {
    let mut header = [0u8; 4];
    match stream.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => {
            return Err(format!(
                "cannot read the supervisor control channel: {error}"
            ));
        }
    }
    let len = u32::from_le_bytes(header) as usize;
    let mut frame = vec![0u8; len];
    stream
        .read_exact(&mut frame)
        .await
        .map_err(|error| format!("cannot read the supervisor control frame: {error}"))?;
    let Some((&kind, payload)) = frame.split_first() else {
        return Err("empty supervisor control frame".to_owned());
    };
    match kind {
        MSG_ANCHOR_READY => {
            if payload.len() != 4 {
                return Err("malformed supervisor anchor-ready frame".to_owned());
            }
            Ok(Some(SupervisorEvent::AnchorReady {
                pgid: i32::from_le_bytes(payload.try_into().expect("four bytes")),
            }))
        }
        MSG_OWNERSHIP_ESTABLISHED => Ok(Some(SupervisorEvent::OwnershipEstablished)),
        MSG_NO_OWNERSHIP => Ok(Some(SupervisorEvent::NoOwnership)),
        MSG_SHELL_EXITED => {
            // { exit_code: i32 LE, signaled: u8, signal: i32 LE }
            if payload.len() != 9 {
                return Err("malformed supervisor shell-exit frame".to_owned());
            }
            let code = i32::from_le_bytes(payload[0..4].try_into().expect("four bytes"));
            let signaled = payload[4];
            let signal = i32::from_le_bytes(payload[5..9].try_into().expect("four bytes"));
            let status = if signaled != 0 {
                ExitStatus::from_raw(signal)
            } else {
                ExitStatus::from_raw(code << 8)
            };
            Ok(Some(SupervisorEvent::ShellExited { status }))
        }
        MSG_ALL_CHILDREN_REAPED => Ok(Some(SupervisorEvent::AllChildrenReaped)),
        MSG_PROCESS_CONTROL_FAILURE => Ok(Some(SupervisorEvent::ProcessControlFailure {
            message: String::from_utf8_lossy(payload).into_owned(),
        })),
        MSG_SIGNAL_ATTEMPT => {
            // { pgid: i32 LE, signal: i32 LE, emitted: u8 }
            if payload.len() != 9 {
                return Err("malformed supervisor signal-attempt frame".to_owned());
            }
            let pgid = i32::from_le_bytes(payload[0..4].try_into().expect("four bytes"));
            let signal = i32::from_le_bytes(payload[4..8].try_into().expect("four bytes"));
            let emitted = payload[8] != 0;
            Ok(Some(SupervisorEvent::SignalAttempt {
                pgid,
                signal,
                emitted,
            }))
        }
        other => Err(format!("unknown supervisor event kind {other:#04x}")),
    }
}

/// The convenience bounded capture path: spawns one owned supervised
/// command, captures bounded stdout/stderr concurrently, settles the owned
/// process tree, and returns the terminal outcome.
#[cfg(unix)]
pub(crate) async fn run_supervised_command(
    spec: SupervisedCommandSpec,
    control: Option<RunnerTestControl>,
) -> Result<CapturedProcessResult, String> {
    #[cfg(not(test))]
    let _ = control;
    let (mut runner, stdout_pipe, stderr_pipe) =
        SupervisedCommandRunner::spawn(&spec, control).map_err(|error| error.to_string())?;
    let stdout_task = stdout_pipe.map(|pipe| tokio::spawn(capture_bounded(pipe)));
    let stderr_task = stderr_pipe.map(|pipe| tokio::spawn(capture_bounded(pipe)));
    let termination = runner.settle().await;
    let stdout = await_capture(stdout_task).await?;
    let stderr = await_capture(stderr_task).await?;
    let exit_code = match (&termination.intent, termination.exit_status) {
        (ProcessOutcomeIntent::Completed, Some(status)) => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if status.signal().is_some() {
                    None
                } else {
                    status.code()
                }
            }
            #[cfg(not(unix))]
            {
                status.code()
            }
        }
        _ => None,
    };
    Ok(CapturedProcessResult {
        exit_code,
        intent: termination.intent,
        stdout,
        stderr,
    })
}

/// Reads one child pipe up to the bounded capture limit, retaining a
/// deterministic truncation marker when the stream exceeds it.
#[cfg(unix)]
async fn capture_bounded<R>(mut pipe: R) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let limit = MAX_PROCESS_OUTPUT_BYTES;
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let read = match pipe.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        total += read as u64;
        let take = (limit + 1).saturating_sub(output.len());
        output.extend_from_slice(&buffer[..read.min(take)]);
    }
    if total > limit as u64 {
        output.truncate(limit);
        let marker = format!("\n...[truncated {} bytes]...\n", total - limit as u64);
        output.extend_from_slice(marker.as_bytes());
    }
    output
}

#[cfg(unix)]
async fn await_capture(
    handle: Option<tokio::task::JoinHandle<Vec<u8>>>,
) -> Result<Vec<u8>, String> {
    match handle {
        Some(handle) => handle
            .await
            .map_err(|join| format!("the output reader task failed: {join}")),
        None => Ok(Vec::new()),
    }
}

/// The signal number of a signal-terminated child, where known.
#[cfg(unix)]
pub(crate) fn unix_signal_of(exit: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    exit.signal()
        .map_or_else(|| "unknown".to_owned(), |signal| signal.to_string())
}

impl core::fmt::Display for RunnerSpawnError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Subreaper(error)
            | Self::ControlChannel(error)
            | Self::ControlChannelPrepare(error)
            | Self::ControlChannelOpen(error)
            | Self::SupervisorSpawn(error) => write!(f, "{error}"),
            Self::InjectedSupervisorSpawn => write!(f, "injected supervisor spawn failure"),
        }
    }
}
