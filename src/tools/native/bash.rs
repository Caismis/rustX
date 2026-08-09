//! Native Bash tool (M5).
//!
//! Executes one non-interactive `/bin/bash -c <command>` invocation per
//! call. No persistent shell exists and no shell state survives between
//! calls. The current working directory is always the explicit workspace
//! root, and the child environment is explicit: `env_clear()` followed by
//! the runtime-approved basics plus explicitly authorized entries, so
//! parent-process secrets are absent unless explicitly authorized.
//!
//! # Complete lifecycle ownership
//!
//! A Bash invocation is one complete lifecycle: spawn one per-invocation
//! supervisor (see [`bash_supervisor`]), read stdout/stderr, wait for the
//! shell, let the supervisor own and reap the invocation's process tree to
//! its kernel-mediated terminal state, handle cancellation/timeout with a
//! `TERM` -> grace -> `KILL` sequence inside the supervisor, complete the
//! output draining, finalize the artifacts, and produce a single canonical
//! result.
//!
//! Shell-parent exit is **not** by itself the Bash settlement boundary:
//! the shell may exit while a descendant still belongs to the
//! invocation-owned process domain, with the output pipes either still held
//! or already redirected away. The invocation therefore settles naturally
//! only when all three of the following are true:
//!
//! - the shell's terminal status is known (the supervisor reported it);
//! - every invocation-owned descendant is terminal and reaped (the outer
//!   supervisor reached its kernel child-wait terminal condition and
//!   reported the authoritative `AllChildrenReaped`);
//! - the runtime-owned output capture is settled.
//!
//! Cancellation and the invocation deadline remain authoritative until the
//! complete lifecycle settles: they trigger the supervisor's
//! `TERM` -> grace -> `KILL` sequence, so a shell-parent exit can never let
//! descendant work escape the timeout/cancellation contract, even when the
//! descendant no longer holds the rustX pipes.
//!
//! # Invocation supervisor
//!
//! The terminal ownership proof is **never** a `/proc` scan. Each Bash
//! invocation owns one small per-invocation supervisor process unit
//! (outer supervisor + inner session/group leader, both subreapers; see
//! [`bash_supervisor`]) that spawns `/bin/bash`, reaps the shell, receives
//! shell descendants that outlive the shell through kernel reparenting, and
//! keeps reaping until its `waitpid(-1)` loop observes `ECHILD` — the
//! kernel's statement that no owned child remains. The **outer**
//! supervisor's `ECHILD` is the single authoritative terminal process-tree
//! event of the whole supervisor unit; rustX combines it with
//! output-capture settlement to produce the tool result.
//!
//! # Terminal results
//!
//! Every Bash `ToolExecutionResult` — `Success`, `Failed`, `Cancelled`,
//! and `TimedOut` alike — is terminal with respect to the
//! invocation-owned process tree: no invocation-owned Bash process remains
//! capable of executing work before any result is returned. A detected
//! process-control/runtime failure determines the eventual result status
//! but does not itself settle the invocation lifecycle: owned work is
//! contained and reaped to the outer supervisor's terminal `ECHILD` (and
//! the capture settled) before the remembered `Failed` result is returned.
//!
//! # Process-group safety
//!
//! The invocation's process group lives in its own session created by the
//! supervisor (`setsid`), so unrelated rustX/sibling processes can never
//! join it (cross-session `setpgid` fails with `EPERM`). `TERM`/`KILL` are
//! issued by the supervisor with `killpg` against the invocation group
//! while the group's numeric id is anchored by the unreaped inner
//! supervisor's own pid — the id is provably allocated to this invocation
//! while the anchor is held, and after the anchor is released no further
//! signal exists. A numeric process-group id that was released can
//! therefore never receive a foreign signal.
//!
//! # Process-control failures
//!
//! Supervisor setup, shell spawning, waiting/reaping, signaling, and IPC
//! failures never swallow their errors: they settle the invocation as an
//! explicit failed tool result. Failures are separated into two
//! categories:
//!
//! - **Before ownership exists** (no Bash process tree was established:
//!   control-channel setup failure, supervisor spawn failure, bash spawn
//!   failure): no cleanup work exists, so an immediate `Failed` is valid.
//! - **After ownership exists** (signal failure, wait/reap failure,
//!   malformed IPC, control-channel read failure, unexpected supervisor
//!   exit, rustX control-channel abandonment): the failure is remembered,
//!   the outer supervisor actively contains the invocation (one
//!   structurally-anchored fallback `SIGKILL` to the owned group), every
//!   owned process is reaped to the outer `ECHILD`, the capture is
//!   finalized, and only then is the remembered `Failed` result returned.
//!
//! If the supervisor can no longer prove that the numeric process group is
//! invocation-owned, it refuses to signal and reports the refusal
//! explicitly; the outer supervisor's fallback containment signals only
//! while its structural anchor (the unreaped inner pid, which is the group
//! id) is held. Cancellation/timeout intent that cannot be established
//! through process control is never downgraded to a silent `Success`,
//! `Cancelled`, or `TimedOut`; the failed result is consistent with the
//! background registry's rule that an explicit process-control/runtime
//! failure may override canonical cancellation settlement.
//!
//! # Output capture
//!
//! stdout, stderr, and the runtime-observed combined multiplex are captured
//! separately. Full output is spooled to the conversation artifact store
//! while bounded previews (head/tail with an explicit truncation marker) are
//! retained for the model; the stored artifact bytes are never corrupted.
//! Non-zero exits are failed tool results with the exit code preserved —
//! never attempt-level runtime failures.
//!
//! Artifact capture failures (pipe reads, artifact allocation, artifact
//! open, writes, or the combined multiplex) are never silently discarded:
//! when no cancellation/timeout owns the outcome, a capture failure is an
//! explicit failed tool result, so the runtime never reports ordinary
//! success while silently losing the promised retained output. During a
//! cancellation/timeout settlement the terminated-process capture is
//! inherently partial, so the cancellation/timeout status wins.
//!
//! [`bash_supervisor`]: crate::tools::native::bash_supervisor

use std::io::Write;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::message::content::FileReference;
use crate::runtime::types::CancellationReason;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{
    BASH_STREAM_PREVIEW_BYTES, BASH_TERMINATION_CONFIRMATION, DEFAULT_FOREGROUND_BASH_TIMEOUT,
};
#[cfg(test)]
use crate::tools::native::bash_supervisor::{
    ANCHOR_PID_FILE_ENV, FAIL_BASH_SPAWN_ENV, FAIL_SIGNAL_ENV, FAIL_WAIT_ENV, FORCE_ANCHOR_LOSS_ENV,
};
use crate::tools::native::bash_supervisor::{COMMAND_ENV, ROLE_OUTER};
use crate::tools::native::support::failed_result;
use crate::tools::native::{NativeToolPolicy, native_definition};
use crate::tools::types::{
    ToolDefinition, ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode,
    ToolResultContent, TruncationState,
};

/// The canonical model-facing name of the tool.
pub const NAME: &str = "bash";

/// The canonical business schema of the tool.
#[must_use]
pub fn definition(policy: NativeToolPolicy) -> ToolDefinition {
    native_definition(
        "tool-bash",
        NAME,
        "Run one non-interactive /bin/bash command inside the workspace with an explicit \
         environment and supervised process ownership.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout_ms": {"type": "integer", "minimum": 1}
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        policy,
    )
}

/// The native Bash executor.
pub struct BashTool {
    #[cfg(test)]
    control: Option<BashTestControl>,
}

impl BashTool {
    /// A Bash executor without test seams.
    #[must_use]
    pub fn new() -> Self {
        Self {
            #[cfg(test)]
            control: None,
        }
    }

    /// Installs the deterministic lifecycle/process-control seams used only
    /// by in-crate regressions.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_test_control(control: BashTestControl) -> Self {
        Self {
            control: Some(control),
        }
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolExecutor for BashTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        #[cfg(test)]
        let control = self.control.clone();
        #[cfg(test)]
        return Box::pin(async move { run_bash(&invocation, &context, control.as_ref()).await });
        #[cfg(not(test))]
        Box::pin(async move { run_bash(&invocation, &context, None).await })
    }
}

/// The test-only control seams of one Bash invocation.
///
/// In non-test builds this type is an empty shell: `BashTool` never holds a
/// control instance and `run_bash` always receives `None`, so no production
/// behavior is affected. The seams exist so in-crate regressions can
/// observe the exact shell-exit boundary, deterministically inject
/// supervisor setup / wait / signal / bash-spawn failures, model the
/// ownership release transition, record every process-group signal attempt,
/// and locate the invocation's process-group id — without an
/// operating-system mocking framework.
#[cfg_attr(test, allow(clippy::struct_excessive_bools))] // a bounded test-seam bundle
#[derive(Clone)]
pub(crate) struct BashTestControl {
    #[cfg(test)]
    lifecycle: BashLifecycleHook,
    #[cfg(test)]
    pause_at_shell_exit: bool,
    #[cfg(test)]
    fail_supervisor_spawn: bool,
    #[cfg(test)]
    fail_bash_spawn: bool,
    #[cfg(test)]
    fail_signal: bool,
    #[cfg(test)]
    fail_wait: bool,
    #[cfg(test)]
    force_anchor_loss: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    anchor_pid_file: Option<std::path::PathBuf>,
    #[cfg(test)]
    recorded_signals: Arc<Mutex<Vec<RecordedSignal>>>,
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

#[cfg(test)]
impl BashTestControl {
    /// A control bundle without failures; the lifecycle hook is present but
    /// not armed, so an executor never parks on it unless the test arms it.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            lifecycle: BashLifecycleHook::new(),
            pause_at_shell_exit: false,
            fail_supervisor_spawn: false,
            fail_bash_spawn: false,
            fail_signal: false,
            fail_wait: false,
            force_anchor_loss: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            anchor_pid_file: None,
            recorded_signals: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Arms the exact shell-exit boundary: the executor parks after the
    /// supervisor reported the shell's natural exit until the test releases
    /// it.
    #[must_use]
    pub(crate) fn pause_at_shell_exit(mut self) -> Self {
        self.pause_at_shell_exit = true;
        self
    }

    /// The lifecycle hook; tests subscribe to the exact shell-exit
    /// boundary and release the parked executor through it.
    #[must_use]
    pub(crate) fn lifecycle(&self) -> &BashLifecycleHook {
        &self.lifecycle
    }

    /// A shared handle that makes the ownership anchor read as lost from
    /// the start of the invocation: the supervisor then behaves as if the
    /// owned group's lifetime had ended and the numeric pgid might name a
    /// foreign group, so it refuses to signal it.
    #[must_use]
    pub(crate) fn force_anchor_loss_handle(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.force_anchor_loss.clone()
    }

    /// Names a file the supervisor writes the invocation's process-group id
    /// into (test observability only).
    #[must_use]
    pub(crate) fn anchor_pid_file(mut self, path: std::path::PathBuf) -> Self {
        self.anchor_pid_file = Some(path);
        self
    }

    /// The recorded process-group signal attempts so far.
    #[must_use]
    pub(crate) fn recorded_signals(&self) -> Vec<RecordedSignal> {
        self.recorded_signals
            .lock()
            .expect("recorded signals lock")
            .clone()
    }

    fn record_signal(&self, recorded: RecordedSignal) {
        self.recorded_signals
            .lock()
            .expect("recorded signals lock")
            .push(recorded);
    }

    /// Makes the supervisor spawn fail with an injected error.
    #[must_use]
    pub(crate) fn fail_supervisor_spawn(mut self) -> Self {
        self.fail_supervisor_spawn = true;
        self
    }

    /// Makes the bash spawn inside the supervisor fail with an injected
    /// error.
    #[must_use]
    pub(crate) fn fail_bash_spawn(mut self) -> Self {
        self.fail_bash_spawn = true;
        self
    }

    /// Makes every group signal in the supervisor fail with an injected
    /// error.
    #[must_use]
    pub(crate) fn fail_signal(mut self) -> Self {
        self.fail_signal = true;
        self
    }

    /// Makes the shell wait in the supervisor fail with an injected error.
    #[must_use]
    pub(crate) fn fail_wait(mut self) -> Self {
        self.fail_wait = true;
        self
    }
}

/// The exact shell-exit lifecycle boundary of one invocation, observable
/// only by in-crate tests.
///
/// The executor signals the boundary exactly once — when the supervisor
/// reported the shell's natural exit — and then parks until the test
/// releases it. Both sides are `tokio::sync::watch`-based, so the test can
/// never miss the boundary and the executor can never be released too
/// early.
#[cfg_attr(not(test), allow(dead_code))] // empty in non-test builds
#[derive(Clone)]
pub(crate) struct BashLifecycleHook {
    #[cfg(test)]
    shell_exit_tx: tokio::sync::watch::Sender<bool>,
    #[cfg(test)]
    proceed_tx: tokio::sync::watch::Sender<bool>,
    #[cfg(test)]
    proceed_rx: tokio::sync::watch::Receiver<bool>,
}

#[cfg(test)]
impl BashLifecycleHook {
    fn new() -> Self {
        let (shell_exit_tx, _) = tokio::sync::watch::channel(false);
        let (proceed_tx, proceed_rx) = tokio::sync::watch::channel(false);
        Self {
            shell_exit_tx,
            proceed_tx,
            proceed_rx,
        }
    }

    /// Executor side: signals the exact shell-exit boundary and parks until
    /// the test releases the executor.
    async fn pause_after_shell_exit(&self) {
        let _ = self.shell_exit_tx.send(true);
        let mut proceed = self.proceed_rx.clone();
        let _ = proceed.changed().await;
    }

    /// Test side: waits until the executor provably observed the shell's
    /// natural exit and parked at the boundary.
    async fn await_shell_exit(&self) {
        let mut rx = self.shell_exit_tx.subscribe();
        if !*rx.borrow() {
            let _ = rx.changed().await;
        }
    }

    /// Test side: releases the parked executor.
    fn release(&self) {
        let _ = self.proceed_tx.send(true);
    }
}

/// A process-control failure of the owned Bash invocation.
///
/// Supervisor setup, signaling, waiting, and IPC failures never silently
/// fail: a failure that undermines ownership or settlement surfaces as an
/// explicit failed tool result, never as an ordinary `Success`,
/// `Cancelled`, or `TimedOut`.
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

/// The three captured stream references of one invocation.
type StreamReferences = (
    Option<FileReference>,
    Option<FileReference>,
    Option<FileReference>,
);

/// The settlement kind when cancellation/timeout owns the invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    Cancelled,
    TimedOut,
}

/// One event reported by the invocation supervisor over the control
/// channel.
enum SupervisorEvent {
    /// The shell exited; `status` is its canonical exit status.
    ShellExited { status: std::process::ExitStatus },
    /// The supervisor's wait loop observed `ECHILD`: no owned child remains.
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

/// The bounded streaming preview capture of one output stream.
///
/// The capture retains a deterministic head/tail preview without holding
/// unbounded output in memory: the full stream is spooled to the artifact
/// store by the reader, and the preview is bounded by
/// [`BASH_STREAM_PREVIEW_BYTES`].
#[derive(Clone)]
struct PreviewCapture {
    head: Vec<u8>,
    tail: Vec<u8>,
    total: u64,
    limit: usize,
}

impl PreviewCapture {
    fn new(limit: usize) -> Self {
        Self {
            head: Vec::new(),
            tail: Vec::new(),
            total: 0,
            limit,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        let half = self.limit / 2;
        if self.head.len() < half {
            let take = (half - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
        }
        if bytes.len() >= half {
            self.tail.clear();
            self.tail.extend_from_slice(&bytes[bytes.len() - half..]);
        } else {
            self.tail.extend_from_slice(bytes);
            let overflow = self.tail.len().saturating_sub(half);
            if overflow > 0 {
                self.tail.drain(..overflow);
            }
        }
    }

    /// The deterministic bounded UTF-8 preview and its truncation state.
    fn finish(self) -> (String, bool) {
        if self.total <= self.limit as u64 {
            return (String::from_utf8_lossy(&self.head).into_owned(), false);
        }
        let marker = format!(
            "\n...[truncated {} bytes]...\n",
            self.total - self.limit as u64
        );
        let content = self.limit.saturating_sub(marker.len());
        let head = &self.head[..self.head.len().min(content / 2)];
        let tail_keep = content - head.len();
        let mut out = Vec::with_capacity(self.limit);
        out.extend_from_slice(head);
        out.extend_from_slice(marker.as_bytes());
        out.extend_from_slice(&self.tail[self.tail.len().saturating_sub(tail_keep)..]);
        (String::from_utf8_lossy(&out).into_owned(), true)
    }
}

/// One output reader task handle.
type StreamHandle = tokio::task::JoinHandle<Result<Option<FileReference>, String>>;

#[allow(clippy::too_many_lines)] // one coherent spawn/supervise/settle pipeline
async fn run_bash(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
    control: Option<&BashTestControl>,
) -> ToolExecutionResult {
    #[cfg(not(test))]
    let _ = control;
    let Some(object) = invocation.arguments.as_object() else {
        return failed_result("bash arguments must be an object");
    };
    let Some(command) = object.get("command").and_then(serde_json::Value::as_str) else {
        return failed_result("bash requires a string command");
    };
    if command.is_empty() {
        return failed_result("bash requires a non-empty command");
    }
    let explicit_timeout = object
        .get("timeout_ms")
        .and_then(serde_json::Value::as_u64)
        .map(Duration::from_millis);
    let timeout = match invocation.mode {
        // Foreground: the omitted timeout uses the default foreground
        // timeout; an explicit timeout overrides it.
        ToolInvocationMode::Foreground => {
            Some(explicit_timeout.unwrap_or(DEFAULT_FOREGROUND_BASH_TIMEOUT))
        }
        // Background: an omitted timeout means no implicit foreground
        // timeout; an explicit timeout may still bound the command.
        ToolInvocationMode::Background => explicit_timeout,
    };

    #[cfg(not(unix))]
    {
        let _ = (context, timeout);
        return failed_result("bash requires a Unix platform with /bin/bash");
    }
    #[cfg(unix)]
    run_bash_unix(command, timeout, context, control).await
}

/// The Unix-only half of [`run_bash`]: spawns the invocation supervisor,
/// supervises its lifecycle events, and settles the invocation.
#[cfg(unix)]
#[allow(clippy::too_many_lines)] // one coherent spawn/supervise/settle pipeline
async fn run_bash_unix(
    command: &str,
    timeout: Option<Duration>,
    context: &ToolExecutionContext<'_>,
    control: Option<&BashTestControl>,
) -> ToolExecutionResult {
    #[cfg(not(test))]
    let _ = control;
    // The invocation supervisor's control channel: one UnixStream pair. The
    // child end becomes the outer supervisor's stdin; the rustX side reads
    // supervisor lifecycle events and writes the one TERMINATE request.
    let (stream_a, stream_b) = match std::os::unix::net::UnixStream::pair() {
        Ok(pair) => pair,
        Err(error) => {
            return failed_result(format!(
                "cannot create the bash supervisor control channel: {error}"
            ));
        }
    };

    let mut supervisor = tokio::process::Command::new(supervisor_binary());
    supervisor.current_dir(context.workspace.root());
    supervisor.env_clear();
    for (key, value) in context
        .environment
        .child_environment(context.workspace.root())
    {
        supervisor.env(key, value);
    }
    supervisor.env("RUSTX_SUPERVISOR_ROLE", ROLE_OUTER);
    supervisor.env(COMMAND_ENV, command);
    #[cfg(test)]
    if let Some(control) = control {
        if let Some(path) = &control.anchor_pid_file {
            supervisor.env(ANCHOR_PID_FILE_ENV, path);
        }
        if control.fail_signal {
            supervisor.env(FAIL_SIGNAL_ENV, "1");
        }
        if control.fail_wait {
            supervisor.env(FAIL_WAIT_ENV, "1");
        }
        if control.fail_bash_spawn {
            supervisor.env(FAIL_BASH_SPAWN_ENV, "1");
        }
        if control
            .force_anchor_loss
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            supervisor.env(FORCE_ANCHOR_LOSS_ENV, "1");
        }
    }
    supervisor.stdin(Stdio::from(std::os::unix::io::OwnedFd::from(stream_b)));
    supervisor.stdout(Stdio::piped());
    supervisor.stderr(Stdio::piped());
    #[cfg(test)]
    if let Some(control) = control {
        if control.fail_supervisor_spawn {
            return failed_result("injected bash supervisor spawn failure");
        }
    }
    let mut child = match supervisor.spawn() {
        Ok(child) => child,
        Err(error) => return failed_result(format!("cannot spawn the bash supervisor: {error}")),
    };
    // The supervisor's own stream end must be non-blocking before tokio
    // adopts it.
    if let Err(error) = nix::fcntl::fcntl(
        &stream_a,
        nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
    ) {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return failed_result(format!(
            "cannot prepare the bash supervisor control channel: {error}"
        ));
    }
    let mut stream = match tokio::net::UnixStream::from_std(stream_a) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return failed_result(format!(
                "cannot open the bash supervisor control channel: {error}"
            ));
        }
    };
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_capture = Arc::new(Mutex::new(PreviewCapture::new(BASH_STREAM_PREVIEW_BYTES)));
    let stderr_capture = Arc::new(Mutex::new(PreviewCapture::new(BASH_STREAM_PREVIEW_BYTES)));
    let combined_capture = Arc::new(Mutex::new(PreviewCapture::new(BASH_STREAM_PREVIEW_BYTES)));
    let (combined_tx, combined_rx) = tokio::sync::mpsc::channel::<(u8, Vec<u8>)>(32);

    let store = context.artifacts.clone();
    let mut combined_task = tokio::spawn(consume_combined(
        combined_rx,
        store,
        combined_capture.clone(),
    ));
    let mut stdout_task = None;
    let mut stderr_task = None;
    if let Some(pipe) = stdout_pipe {
        stdout_task = Some(tokio::spawn(spool_stream(
            pipe,
            context.artifacts.clone(),
            stdout_capture.clone(),
            combined_tx.clone(),
            0,
            "stdout",
        )));
    }
    if let Some(pipe) = stderr_pipe {
        stderr_task = Some(tokio::spawn(spool_stream(
            pipe,
            context.artifacts.clone(),
            stderr_capture.clone(),
            combined_tx.clone(),
            1,
            "stderr",
        )));
    }
    drop(combined_tx);

    // The lifecycle supervision loop: the supervisor's events (shell exit,
    // the outer supervisor's all-children-reaped, process-control
    // failures, signal attempts) race cancellation and the invocation
    // timeout (biased: an already observable cancellation or an expired
    // deadline wins without starting new work). Shell-parent exit is not
    // settlement, and neither is a detected failure: the invocation
    // settles only when the capture is settled AND the owned child set is
    // terminal (the outer supervisor's authoritative AllChildrenReaped)
    // AND some outcome intent is known. A failure determines the eventual
    // result status but never settles the invocation lifecycle by itself:
    // owned work must be contained and terminal before any result —
    // `Success`, `Failed`, `Cancelled`, or `TimedOut` — is returned.
    let start = tokio::time::Instant::now();
    let mut exit_status = None;
    let mut children_terminal = false;
    let mut failure = None;
    let mut settled = None;
    let mut drain_result: Option<Box<Result<StreamReferences, String>>> = None;
    let mut terminate_sent = false;
    let mut terminate_deadline: Option<tokio::time::Instant> = None;
    loop {
        // Outcome intent and lifecycle settlement are distinct: the loop
        // may break only when the capture is settled, the owned child set
        // is terminal, and an outcome intent (failure, cancellation/
        // timeout, or the shell's natural status) is known.
        let outcome_intent = failure.is_some() || settled.is_some() || exit_status.is_some();
        if drain_result.is_some() && children_terminal && outcome_intent {
            break;
        }
        // After TERMINATE the supervisor performs TERM -> grace -> KILL;
        // the owned tree must reach its terminal child set within the
        // bounded confirmation window, or the settlement is an explicit
        // failure — never a premature Success/Cancelled/TimedOut.
        if terminate_sent
            && !children_terminal
            && terminate_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
        {
            failure = Some(ProcessControlError::QuiescenceTimeout.to_string());
            continue;
        }
        // The drain future polls the output reader tasks; a completed
        // JoinHandle must never be polled again, so the drain arm is
        // disabled once the capture settled (the future is still
        // constructed for the borrows but never polled).
        let mut drain = Box::pin(await_drain(
            &mut stdout_task,
            &mut stderr_task,
            &mut combined_task,
        ));
        tokio::select! {
            biased;
            () = context.cancellation.cancelled(), if settled.is_none() && !terminate_sent => {
                settled = Some(Settled::Cancelled);
                send_terminate(&mut stream).await;
                terminate_sent = true;
                terminate_deadline = Some(tokio::time::Instant::now() + BASH_TERMINATION_CONFIRMATION);
            }
            () = async {
                match timeout {
                    Some(timeout) => tokio::time::sleep_until(start + timeout).await,
                    None => std::future::pending().await,
                }
            }, if settled.is_none() && !terminate_sent => {
                settled = Some(Settled::TimedOut);
                send_terminate(&mut stream).await;
                terminate_sent = true;
                terminate_deadline = Some(tokio::time::Instant::now() + BASH_TERMINATION_CONFIRMATION);
            }
            event = read_supervisor_event(&mut stream) => match event {
                Ok(Some(SupervisorEvent::ShellExited { status })) => {
                    exit_status = Some(status);
                    // The exact shell-exit boundary of a natural exit,
                    // observed only by in-crate tests: the executor signals
                    // that the supervisor reported the shell's natural exit
                    // and parks before any settlement handling begins.
                    #[cfg(test)]
                    if let Some(control) = control {
                        if control.pause_at_shell_exit && settled.is_none() && !terminate_sent {
                            control.lifecycle.pause_after_shell_exit().await;
                        }
                    }
                }
                Ok(Some(SupervisorEvent::AllChildrenReaped)) => {
                    children_terminal = true;
                }
                Ok(Some(SupervisorEvent::ProcessControlFailure { message })) => {
                    failure = Some(message);
                }
                Ok(Some(SupervisorEvent::SignalAttempt { pgid, signal, emitted })) => {
                    #[cfg(test)]
                    if let Some(control) = control {
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
                    // The supervisor unit exited (control channel EOF). The
                    // outer supervisor is the sole terminal-report
                    // authority: it exits only after its own ECHILD (having
                    // reported AllChildrenReaped) or before any owned
                    // process tree existed. A missing terminal report here
                    // therefore proves no invocation-owned process remains
                    // in the unit's child domain; the unit is terminal
                    // either way.
                    if !children_terminal {
                        failure =
                            Some(ProcessControlError::UnexpectedSupervisorExit.to_string());
                        children_terminal = true;
                    } else if !outcome_intent {
                        failure = Some(ProcessControlError::UnexpectedSupervisorExit.to_string());
                    }
                }
                Err(error) => {
                    failure = Some(error);
                }
            },
            result = &mut drain, if drain_result.is_none() => {
                drain_result = Some(Box::new(result));
            }
        }
    }

    // The supervisor unit is terminal by now: its pipes closed (which
    // settled the capture) or the control channel reached EOF. Reap the
    // direct child; the bound is a deadlock guard only — the outer
    // supervisor exits promptly after its terminal report.
    let _ = tokio::time::timeout(BASH_TERMINATION_CONFIRMATION, child.wait()).await;

    // The supervisor exits only after its terminal child set, closing the
    // capture pipes; the bounded drain re-drain completes the capture.
    let capture = if let Some(result) = drain_result {
        // The capture already completed while the supervision loop was
        // running; its result is reused — a completed JoinHandle must never
        // be polled again.
        *result
    } else {
        match tokio::time::timeout(
            BASH_TERMINATION_CONFIRMATION,
            await_drain(&mut stdout_task, &mut stderr_task, &mut combined_task),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                Err("the bash output capture did not settle within the bounded window".to_owned())
            }
        }
    };

    let (stdout_reference, stderr_reference, combined_reference) = match capture {
        Ok(references) => references,
        Err(error) => {
            // The outcome is already owned (failure or cancellation/
            // timeout): the capture of a terminated process tree is
            // inherently partial and is never reported as successful
            // retention. The root-cause failure is never overwritten by the
            // later capture condition; at most the capture detail is
            // appended to it.
            if let Some(message) = failure.as_mut() {
                message.push_str("; output capture: ");
                message.push_str(&error);
            }
            if settled.is_some() || failure.is_some() {
                (None, None, None)
            } else {
                return failed_result(format!("bash output capture failed: {error}"));
            }
        }
    };

    let stdout = stdout_capture
        .lock()
        .expect("preview lock")
        .clone()
        .finish();
    let stderr = stderr_capture
        .lock()
        .expect("preview lock")
        .clone()
        .finish();
    let combined = combined_capture
        .lock()
        .expect("preview lock")
        .clone()
        .finish();

    // Outcome precedence: an explicit process-control/runtime failure wins
    // over cancellation/timeout intent, which wins over the natural shell
    // result — but in every case the owned process tree is already
    // terminal, so the returned status is terminal with respect to the
    // invocation-owned process tree.
    let mut status = ToolExecutionStatus::Success;
    let mut exit_code = None;
    if let Some(failure_message) = failure {
        status = ToolExecutionStatus::Failed {
            error: format!("bash process control failed: {failure_message}"),
        };
    } else if let Some(settled) = settled {
        // Cancellation/timeout owns settlement and wins over any partial
        // natural exit data.
        status = match settled {
            Settled::Cancelled => ToolExecutionStatus::Cancelled {
                reason: CancellationReason::UserRequested,
            },
            Settled::TimedOut => ToolExecutionStatus::TimedOut,
        };
    } else if let Some(exit) = exit_status {
        if exit.success() {
            status = ToolExecutionStatus::Success;
            exit_code = exit.code();
        } else if let Some(code) = exit.code() {
            status = ToolExecutionStatus::Failed {
                error: format!("command exited with code {code}"),
            };
            exit_code = Some(code);
        } else {
            let signal = unix_signal_of(exit);
            status = ToolExecutionStatus::Failed {
                error: format!("command terminated by signal {signal}"),
            };
        }
    }

    let truncated = stdout.1 || stderr.1 || combined.1;
    ToolExecutionResult {
        status,
        content: vec![ToolResultContent::Json {
            value: serde_json::json!({
                "exit_code": exit_code,
                "stdout": stdout.0,
                "stderr": stderr.0,
                "combined": combined.0,
            }),
        }],
        duration_ms: 0,
        exit_code,
        artifacts: vec![stdout_reference, stderr_reference, combined_reference]
            .into_iter()
            .flatten()
            .collect(),
        truncation: truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: None,
        }),
    }
}

/// The dedicated supervisor binary: `CARGO_BIN_EXE` when cargo provides it
/// (integration tests), otherwise the `bash-supervisor` sibling of the
/// current executable (production), otherwise the binary-directory sibling
/// of a test binary living under `target/debug/deps` (in-crate tests).
#[cfg(unix)]
fn supervisor_binary() -> std::path::PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_bash-supervisor") {
        return std::path::PathBuf::from(path);
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

/// The `TERMINATE` control-message kind (mirrors
/// `bash_supervisor::MSG_TERMINATE`).
#[cfg(unix)]
const MSG_TERMINATE: u8 = 0x10;

/// Reads exactly one supervisor control frame:
/// `[u32 LE length][kind][payload]`.
///
/// `Ok(None)` means the control channel reached EOF: the supervisor exited.
#[cfg(unix)]
async fn read_supervisor_event(
    stream: &mut tokio::net::UnixStream,
) -> Result<Option<SupervisorEvent>, String> {
    let mut header = [0u8; 4];
    match stream.read_exact(&mut header).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => {
            return Err(format!(
                "cannot read the bash supervisor control channel: {error}"
            ));
        }
    }
    let len = u32::from_le_bytes(header) as usize;
    let mut frame = vec![0u8; len];
    stream
        .read_exact(&mut frame)
        .await
        .map_err(|error| format!("cannot read the bash supervisor control frame: {error}"))?;
    let Some((&kind, payload)) = frame.split_first() else {
        return Err("empty bash supervisor control frame".to_owned());
    };
    match kind {
        MSG_SHELL_EXITED => {
            // { exit_code: i32 LE, signaled: u8, signal: i32 LE }
            if payload.len() != 9 {
                return Err("malformed bash supervisor shell-exit frame".to_owned());
            }
            let code = i32::from_le_bytes(payload[0..4].try_into().expect("four bytes"));
            let signaled = payload[4];
            let signal = i32::from_le_bytes(payload[5..9].try_into().expect("four bytes"));
            let status = if signaled != 0 {
                std::process::ExitStatus::from_raw(signal)
            } else {
                std::process::ExitStatus::from_raw(code << 8)
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
                return Err("malformed bash supervisor signal-attempt frame".to_owned());
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
        other => Err(format!("unknown bash supervisor event kind {other:#04x}")),
    }
}

/// The supervisor event kinds (mirror `bash_supervisor`).
#[cfg(unix)]
const MSG_SHELL_EXITED: u8 = 0x02;
#[cfg(unix)]
const MSG_ALL_CHILDREN_REAPED: u8 = 0x03;
#[cfg(unix)]
const MSG_PROCESS_CONTROL_FAILURE: u8 = 0x04;
#[cfg(unix)]
const MSG_SIGNAL_ATTEMPT: u8 = 0x05;

/// Streams one child pipe into its preview capture and the combined
/// multiplex, spooling the full raw bytes into the artifact store.
///
/// Any capture failure (pipe read, artifact allocation, artifact open, or
/// write) is returned explicitly; it is never silently discarded.
async fn spool_stream<R>(
    mut pipe: R,
    store: crate::tools::artifacts::ArtifactStore,
    capture: Arc<Mutex<PreviewCapture>>,
    combined_tx: tokio::sync::mpsc::Sender<(u8, Vec<u8>)>,
    stream_id: u8,
    name: &'static str,
) -> Result<Option<FileReference>, String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut artifact: Option<(
        crate::runtime::identity::ArtifactId,
        crate::tools::artifacts::ArtifactWriter,
    )> = None;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = match pipe.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                return Err(format!("cannot read the {name} stream: {error}"));
            }
        };
        let chunk = buffer[..read].to_vec();
        capture.lock().expect("preview lock").push(&chunk);
        combined_tx
            .send((stream_id, chunk.clone()))
            .await
            .map_err(|_| format!("the combined {name} capture is unavailable"))?;
        let writer = if let Some(handle) = &mut artifact {
            handle
        } else {
            let id = store
                .create_artifact()
                .map_err(|error| format!("cannot allocate the {name} artifact: {error}"))?;
            let writer = store
                .open_writer(&id)
                .map_err(|error| format!("cannot open the {name} artifact: {error}"))?;
            artifact = Some((id, writer));
            artifact.as_mut().expect("inserted above")
        };
        writer
            .1
            .write_all(&chunk)
            .map_err(|error| format!("cannot write the {name} artifact: {error}"))?;
    }
    drop(combined_tx);
    Ok(artifact.map(|(id, _)| FileReference {
        artifact_id: id,
        name: Some(format!("{name}.log")),
        mime_type: Some("application/octet-stream".to_owned()),
        description: Some(format!("full {name} output of the bash execution")),
    }))
}

/// Consumes the runtime-observed combined stdout/stderr multiplex and spools
/// it as one artifact while retaining a bounded preview.
async fn consume_combined(
    mut rx: tokio::sync::mpsc::Receiver<(u8, Vec<u8>)>,
    store: crate::tools::artifacts::ArtifactStore,
    capture: Arc<Mutex<PreviewCapture>>,
) -> Result<Option<FileReference>, String> {
    let mut artifact: Option<(
        crate::runtime::identity::ArtifactId,
        crate::tools::artifacts::ArtifactWriter,
    )> = None;
    while let Some((_stream_id, chunk)) = rx.recv().await {
        capture.lock().expect("preview lock").push(&chunk);
        let writer = if let Some(handle) = &mut artifact {
            handle
        } else {
            let id = store
                .create_artifact()
                .map_err(|error| format!("cannot allocate the combined artifact: {error}"))?;
            let writer = store
                .open_writer(&id)
                .map_err(|error| format!("cannot open the combined artifact: {error}"))?;
            artifact = Some((id, writer));
            artifact.as_mut().expect("inserted above")
        };
        writer
            .1
            .write_all(&chunk)
            .map_err(|error| format!("cannot write the combined artifact: {error}"))?;
    }
    Ok(artifact.map(|(id, _)| FileReference {
        artifact_id: id,
        name: Some("combined.log".to_owned()),
        mime_type: Some("application/octet-stream".to_owned()),
        description: Some("combined stdout/stderr output of the bash execution".to_owned()),
    }))
}

/// Awaits every output reader task; the handles stay usable after a dropped
/// drain future, so a terminated tree can be re-drained exactly once more.
async fn await_drain(
    stdout_task: &mut Option<StreamHandle>,
    stderr_task: &mut Option<StreamHandle>,
    combined_task: &mut StreamHandle,
) -> Result<
    (
        Option<FileReference>,
        Option<FileReference>,
        Option<FileReference>,
    ),
    String,
> {
    let stdout = await_handle(stdout_task).await?;
    let stderr = await_handle(stderr_task).await?;
    let combined = combined_task
        .await
        .map_err(|join| format!("the combined output reader task failed: {join}"))??;
    Ok((stdout, stderr, combined))
}

async fn await_handle(handle: &mut Option<StreamHandle>) -> Result<Option<FileReference>, String> {
    match handle {
        Some(handle) => handle
            .await
            .map_err(|join| format!("the output reader task failed: {join}"))?,
        None => Ok(None),
    }
}

/// The signal number of a signal-terminated child, where known.
fn unix_signal_of(exit: std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        exit.signal()
            .map_or_else(|| "unknown".to_owned(), |signal| signal.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = exit;
        "unknown".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{BashTestControl, BashTool, NAME};
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
    use crate::tools::types::{
        ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolProgress,
    };
    use crate::tools::workspace::Workspace;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    struct NoopProgress;

    impl ProgressReporter for NoopProgress {
        fn report(&self, _progress: ToolProgress) {}
    }

    fn fixture() -> (tempfile::TempDir, ArtifactStore, Workspace) {
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace_root = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace");
        let artifacts =
            ArtifactStore::new(ConversationId::new("conv-1"), dir.path().join("artifacts"))
                .expect("artifacts");
        let workspace = Workspace::new(&workspace_root).expect("workspace");
        (dir, artifacts, workspace)
    }

    fn invocation(command: &str) -> ToolInvocation {
        ToolInvocation {
            call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-bash"),
            tool_name: NAME.to_owned(),
            mode: ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"command": command}),
        }
    }

    fn invocation_with_timeout(command: &str, timeout_ms: u64) -> ToolInvocation {
        ToolInvocation {
            call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-bash"),
            tool_name: NAME.to_owned(),
            mode: ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"command": command, "timeout_ms": timeout_ms}),
        }
    }

    async fn run_with(
        command: &str,
        artifacts: &ArtifactStore,
        workspace: &Workspace,
    ) -> ToolExecutionResult {
        run_with_control(
            command.to_owned(),
            BashTestControl::new(),
            CancellationSignal::new(),
            artifacts.clone(),
            workspace.clone(),
            None,
        )
        .await
    }

    /// Executes one invocation through the executor with explicit test
    /// control seams and a caller-controlled cancellation signal. Takes
    /// owned values so it can be spawned without borrowing.
    #[allow(clippy::too_many_arguments)] // a bounded test-only fixture surface
    async fn run_with_control(
        command: String,
        control: BashTestControl,
        cancellation: CancellationSignal,
        artifacts: ArtifactStore,
        workspace: Workspace,
        timeout_ms: Option<u64>,
    ) -> ToolExecutionResult {
        let tool = BashTool::with_test_control(control);
        let reporter = NoopProgress;
        let context = ToolExecutionContext {
            conversation_id: &ConversationId::new("conv-1"),
            execution_id: None,
            cancellation,
            workspace: &workspace,
            progress: &reporter,
            artifacts: &artifacts,
            environment: &ToolEnvironment::new(),
        };
        let invocation = match timeout_ms {
            Some(ms) => invocation_with_timeout(&command, ms),
            None => invocation(&command),
        };
        tool.execute(invocation, context).await
    }

    /// Whether a specific process still exists (signal-0 probe).
    #[cfg(unix)]
    fn process_alive(pid: i32) -> bool {
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
            Err(nix::errno::Errno::ESRCH) => false,
            Ok(()) | Err(_) => true,
        }
    }

    /// Polls a process until it is provably gone (ESRCH), with a strict
    /// deadlock guard. Polling the authoritative OS process state with a
    /// deadline is the test's proof; there is no assumed interleaving.
    #[cfg(unix)]
    async fn wait_for_process_death(pid: i32) {
        for _ in 0..1000 {
            if !process_alive(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("process {pid} is still alive after the deadline");
    }

    /// Polls the invocation's process group until it is provably gone, with
    /// a strict deadlock guard. Valid only after the invocation has
    /// settled: the supervisor's final reap has then removed the group.
    #[cfg(unix)]
    async fn wait_for_group_death(pgid: i32) {
        use nix::errno::Errno;
        use nix::sys::signal::killpg;
        use nix::unistd::Pid;
        for _ in 0..1000 {
            match killpg(Pid::from_raw(pgid), None) {
                Ok(()) | Err(Errno::EPERM) => {}
                Err(_) => return,
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("process group {pgid} is still alive after the deadline");
    }

    /// An artifact write failure is represented explicitly: the invocation
    /// fails instead of reporting ordinary success while losing the
    /// promised retained output.
    #[tokio::test]
    async fn artifact_write_failure_fails_the_invocation_explicitly() {
        let (_dir, artifacts, workspace) = fixture();
        artifacts.set_force_write_failures(true);
        let result = run_with("echo hello", &artifacts, &workspace).await;
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "an artifact capture failure must be an explicit failed result, got {:?}",
            result.status
        );
        assert!(
            !matches!(result.status, ToolExecutionStatus::Success),
            "successful retention must never be reported while full output is lost"
        );
    }

    /// An artifact allocation failure (sequence exhaustion) is represented
    /// explicitly as well.
    #[tokio::test]
    async fn artifact_allocation_failure_fails_the_invocation_explicitly() {
        let (_dir, artifacts, workspace) = fixture();
        artifacts.exhaust_sequence();
        let result = run_with("echo hello", &artifacts, &workspace).await;
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "an artifact allocation failure must be an explicit failed result, got {:?}",
            result.status
        );
        assert!(
            !matches!(result.status, ToolExecutionStatus::Success),
            "successful retention must never be reported while full output is lost"
        );
    }

    /// A shell parent that exits while a redirected descendant stays in the
    /// owned process domain (`sleep 30 >/dev/null 2>&1 & exit 0`) cannot
    /// settle the invocation: the descendant no longer holds the rustX
    /// pipes, so the capture alone would finish — but the supervisor still
    /// owns the descendant and the invocation stays active until the
    /// timeout settles it.
    #[cfg(unix)]
    #[tokio::test]
    async fn redirected_descendant_does_not_escape_the_owned_domain() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let desc_pid_file = root.join("desc.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let command = format!(
            "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
            shell_pid_file.display(),
            desc_pid_file.display()
        );
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            run_with_control(
                command,
                BashTestControl::new().anchor_pid_file(anchor_pid_file.clone()),
                CancellationSignal::new(),
                artifacts,
                workspace,
                Some(500),
            ),
        )
        .await
        .expect("the invocation settles exactly once");
        assert_eq!(
            result.status,
            ToolExecutionStatus::TimedOut,
            "a redirected descendant must not let the invocation settle as Success"
        );
        let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
            .expect("descendant pid file")
            .trim()
            .parse()
            .expect("descendant pid");
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        // The owned process group is quiescent and the descendant is gone.
        wait_for_group_death(anchor_pid).await;
        wait_for_process_death(descendant_pid).await;
        let _ = dir;
    }

    /// The exact shell-exit boundary regression: the executor provably
    /// observed the shell parent's natural exit (the supervisor's report)
    /// and parked before any settlement handling; the descendant is
    /// provably alive at that boundary; only then does cancellation become
    /// observable. The result is `Cancelled` and the owned group is
    /// terminated.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_after_exact_shell_exit_boundary_terminates_the_owned_group() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let desc_pid_file = root.join("desc.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let command = format!(
            "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
            shell_pid_file.display(),
            desc_pid_file.display()
        );
        let control = BashTestControl::new()
            .pause_at_shell_exit()
            .anchor_pid_file(anchor_pid_file.clone());
        let hook = control.lifecycle().clone();
        let cancellation = CancellationSignal::new();
        let cancelling = cancellation.clone();
        let task = tokio::spawn(run_with_control(
            command,
            control,
            cancellation,
            artifacts.clone(),
            workspace.clone(),
            None,
        ));
        // 1. The exact boundary: the shell parent exited and the executor
        //    is parked before natural settlement handling.
        tokio::time::timeout(Duration::from_secs(15), hook.await_shell_exit())
            .await
            .expect("the shell-exit boundary is observed");
        // 2. The descendant is provably still alive at the boundary.
        let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
            .expect("descendant pid file")
            .trim()
            .parse()
            .expect("descendant pid");
        assert!(
            process_alive(descendant_pid),
            "the descendant must still be alive at the exact shell-exit boundary"
        );
        // 3. Cancellation becomes observable after the boundary.
        cancelling.cancel();
        // 4. The executor resumes.
        hook.release();
        let result = tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("the invocation settles exactly once")
            .expect("executor task");
        assert!(
            matches!(result.status, ToolExecutionStatus::Cancelled { .. }),
            "late cancellation after the shell-parent exit must be Cancelled, got {:?}",
            result.status
        );
        // 5. The owned group is terminated and quiescent; the descendant is
        //    gone.
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        wait_for_group_death(anchor_pid).await;
        wait_for_process_death(descendant_pid).await;
        let _ = dir;
    }

    /// Natural settlement requires the supervisor's terminal child set: at
    /// the exact shell-exit boundary the invocation is provably not yet
    /// settled while the descendant is alive; once the descendant exits
    /// naturally and the supervisor reaps it, the shell's natural
    /// successful exit settles the invocation as `Success`.
    #[cfg(unix)]
    #[tokio::test]
    async fn natural_success_requires_terminal_child_ownership() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let desc_pid_file = root.join("desc.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let command = format!(
            "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
            shell_pid_file.display(),
            desc_pid_file.display()
        );
        let control = BashTestControl::new()
            .pause_at_shell_exit()
            .anchor_pid_file(anchor_pid_file.clone());
        let hook = control.lifecycle().clone();
        let task = tokio::spawn(run_with_control(
            command,
            control,
            CancellationSignal::new(),
            artifacts.clone(),
            workspace.clone(),
            None,
        ));
        // 1. The exact boundary: shell exited, executor parked, descendant
        //    still alive — the invocation must not have settled yet.
        tokio::time::timeout(Duration::from_secs(15), hook.await_shell_exit())
            .await
            .expect("the shell-exit boundary is observed");
        let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
            .expect("descendant pid file")
            .trim()
            .parse()
            .expect("descendant pid");
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        assert!(process_alive(descendant_pid));
        // 2. The test terminates the invocation group directly (test-side
        //    process control, deterministic: no timing assumption). The
        //    inner supervisor dies with it; its children reparent to the
        //    outer supervisor, which reaps them to its ECHILD.
        nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(anchor_pid),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("test kills the owned group");
        wait_for_process_death(descendant_pid).await;
        // 3. The executor resumes and observes the terminal child set.
        hook.release();
        let result = tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("the invocation settles exactly once")
            .expect("executor task");
        assert_eq!(
            result.status,
            ToolExecutionStatus::Success,
            "once the owned child set is terminal, the shell's natural exit settles Success"
        );
        let exit_code = result
            .content
            .iter()
            .find_map(|content| match content {
                crate::tools::types::ToolResultContent::Json { value } => {
                    value["exit_code"].as_i64()
                }
                _ => None,
            })
            .expect("exit code in the JSON result");
        assert_eq!(exit_code, 0);
        let _ = dir;
    }

    /// A signaling failure during cancellation is an explicit failed
    /// result — and the failure is terminal with respect to the owned
    /// process tree. The inner supervisor refuses the group signal and
    /// escalates containment to the outer supervisor, which emits exactly
    /// one structurally-anchored fallback `SIGKILL` against the owned
    /// group; `Failed` is returned only after the shell, the descendant,
    /// and the whole group are provably gone. No test-side process control
    /// is involved after the result settles.
    #[cfg(unix)]
    #[tokio::test]
    async fn signal_failure_settles_as_an_explicit_failed_result() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let desc_pid_file = root.join("desc.pid");
        let anchor_pid_file = root.join("anchor.pid");
        // The shell records its own pid and the descendant's pid so the
        // test can prove both are terminal when the result exists.
        let command = format!(
            "echo $$ > {}; sleep 30 & echo $! > {}; wait",
            shell_pid_file.display(),
            desc_pid_file.display()
        );
        let cancellation = CancellationSignal::new();
        let cancelling = cancellation.clone();
        let control = BashTestControl::new()
            .fail_signal()
            .anchor_pid_file(anchor_pid_file.clone());
        let task = tokio::spawn(run_with_control(
            command,
            control.clone(),
            cancellation,
            artifacts.clone(),
            workspace.clone(),
            None,
        ));
        // The shell provably started (its pid file exists) before the
        // cancellation becomes observable.
        for _ in 0..1000 {
            if shell_pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(shell_pid_file.exists(), "the shell pid file never appeared");
        cancelling.cancel();
        let result = tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("the invocation settles")
            .expect("executor task");
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "an injected signaling failure must be an explicit failed result, got {:?}",
            result.status
        );
        // The failure was terminal: the shell, the descendant, and the
        // whole owned group are provably gone by the time the result
        // exists. The outer supervisor's fallback containment did the
        // work; there is no test-side kill to perform.
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
            .expect("descendant pid file")
            .trim()
            .parse()
            .expect("descendant pid");
        let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
            .expect("shell pid file")
            .trim()
            .parse()
            .expect("shell pid");
        wait_for_group_death(anchor_pid).await;
        wait_for_process_death(shell_pid).await;
        wait_for_process_death(descendant_pid).await;
        // The containment path is the recorded proof: the inner refused
        // the group TERM (emitted == false), and the outer's fallback
        // emitted exactly one SIGKILL against exactly the anchored pgid.
        let recorded = control.recorded_signals();
        let refusals: Vec<_> = recorded.iter().filter(|attempt| !attempt.emitted).collect();
        assert!(
            !refusals.is_empty(),
            "the injected signaling failure must have refused the group TERM"
        );
        let kills: Vec<_> = recorded.iter().filter(|attempt| attempt.emitted).collect();
        assert_eq!(
            kills.len(),
            1,
            "fallback containment emits exactly one SIGKILL, got: {recorded:?}"
        );
        assert_eq!(kills[0].signal, "SIGKILL");
        assert_eq!(kills[0].pgid, anchor_pid);
        let _ = dir;
    }

    /// A supervisor setup failure is an explicit failed result: the
    /// invocation never claims a lifecycle it cannot establish.
    #[tokio::test]
    async fn supervisor_setup_failure_settles_as_an_explicit_failed_result() {
        let (_dir, artifacts, workspace) = fixture();
        let result = run_with_control(
            "echo hi".to_owned(),
            BashTestControl::new().fail_supervisor_spawn(),
            CancellationSignal::new(),
            artifacts,
            workspace,
            None,
        )
        .await;
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "an injected supervisor setup failure must be an explicit failed result, got {:?}",
            result.status
        );
        assert!(
            !matches!(result.status, ToolExecutionStatus::Success),
            "a failed supervisor setup must never be reported as success"
        );
    }

    /// A bash spawn failure inside the supervisor is an explicit failed
    /// result as well.
    #[tokio::test]
    async fn bash_spawn_failure_settles_as_an_explicit_failed_result() {
        let (_dir, artifacts, workspace) = fixture();
        let result = run_with_control(
            "echo hi".to_owned(),
            BashTestControl::new().fail_bash_spawn(),
            CancellationSignal::new(),
            artifacts,
            workspace,
            None,
        )
        .await;
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "an injected bash spawn failure must be an explicit failed result, got {:?}",
            result.status
        );
        assert!(
            !matches!(result.status, ToolExecutionStatus::Success),
            "a failed bash spawn must never be reported as success"
        );
    }

    /// A wait/reap failure after ownership is established is an explicit
    /// failed result — and the failure is terminal with respect to the
    /// owned process tree. The fixture has a real descendant: the inner
    /// supervisor fails the shell wait and escalates containment to the
    /// outer supervisor, which terminates the owned group; `Failed` is
    /// returned only after the descendant and the group are provably gone.
    /// No test-side process control follows the result.
    #[cfg(unix)]
    #[tokio::test]
    async fn wait_failure_settles_as_an_explicit_failed_result() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let desc_pid_file = root.join("desc.pid");
        let anchor_pid_file = root.join("anchor.pid");
        // The shell exits immediately, so the injected wait failure fires
        // while the redirected descendant is still owned and alive.
        let command = format!(
            "sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
            desc_pid_file.display()
        );
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            run_with_control(
                command,
                BashTestControl::new()
                    .fail_wait()
                    .anchor_pid_file(anchor_pid_file.clone()),
                CancellationSignal::new(),
                artifacts,
                workspace,
                None,
            ),
        )
        .await
        .expect("the invocation settles exactly once");
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "an injected wait failure must be an explicit failed result, got {:?}",
            result.status
        );
        // The failure was terminal: the descendant and the whole owned
        // group are provably gone by the time the result exists, with no
        // test-side kill.
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
            .expect("descendant pid file")
            .trim()
            .parse()
            .expect("descendant pid");
        wait_for_group_death(anchor_pid).await;
        wait_for_process_death(descendant_pid).await;
        let _ = dir;
    }

    /// The PGID-reuse fail-safe regression, strengthened for fallback
    /// containment: the inner supervisor's ownership anchor reads as lost
    /// (the exact test seam, never a probabilistic PID reuse), so the
    /// inner refuses every group signal. Containment escalates to the
    /// outer supervisor, whose structural anchor — the un-reaped inner
    /// pid, which is the invocation's process-group id — is still provably
    /// held, and which emits exactly one fallback `SIGKILL` against
    /// exactly that anchored pgid. The owned tree dies with it and the
    /// invocation settles `Failed`; no foreign process group is ever
    /// signaled and no test-side kill is involved.
    #[cfg(unix)]
    #[tokio::test]
    async fn no_signals_are_issued_after_ownership_loss() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let desc_pid_file = root.join("desc.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let command = format!(
            "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; wait",
            shell_pid_file.display(),
            desc_pid_file.display()
        );
        let control = BashTestControl::new().anchor_pid_file(anchor_pid_file.clone());
        // 1. The ownership anchor reads as lost from the start: the inner
        //    supervisor behaves as if the owned group's lifetime had ended
        //    and the numeric pgid might name a foreign group.
        control
            .force_anchor_loss_handle()
            .store(true, Ordering::SeqCst);
        let cancellation = CancellationSignal::new();
        let cancelling = cancellation.clone();
        let task = tokio::spawn(run_with_control(
            command,
            control.clone(),
            cancellation,
            artifacts.clone(),
            workspace.clone(),
            None,
        ));
        // 2. The owned group provably exists: the shell is running.
        for _ in 0..1000 {
            if shell_pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(shell_pid_file.exists(), "the shell pid file never appeared");
        // 3. Cancellation becomes observable; the inner supervisor refuses
        //    to signal the (per its seam, possibly foreign) numeric pgid.
        cancelling.cancel();
        let result = tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("the invocation settles")
            .expect("executor task");
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "lost ownership must settle explicitly Failed, got {:?}",
            result.status
        );
        // 4. The inner supervisor emitted zero signals: every inner attempt
        //    was refused (emitted == false) and targeted the numeric pgid
        //    under question. The single emitted signal is the outer
        //    supervisor's fallback containment SIGKILL against exactly the
        //    structurally anchored pgid, issued only after the inner's
        //    refusals.
        let recorded = control.recorded_signals();
        assert!(
            !recorded.is_empty(),
            "a cancellation was attempted and must have reached the signal path"
        );
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        for attempt in &recorded {
            assert_eq!(
                attempt.pgid, anchor_pid,
                "every attempt targets the numeric pgid under question"
            );
        }
        let first_emitted = recorded
            .iter()
            .position(|attempt| attempt.emitted)
            .expect("the outer fallback containment must emit exactly one signal");
        assert_eq!(
            recorded.iter().filter(|attempt| attempt.emitted).count(),
            1,
            "the only emitted signal is the outer's structural fallback containment, got: {recorded:?}"
        );
        assert_eq!(recorded[first_emitted].signal, "SIGKILL");
        assert!(
            recorded[..first_emitted]
                .iter()
                .all(|attempt| !attempt.emitted),
            "every inner attempt before the fallback containment was refused"
        );
        // 5. The owned group was contained and is terminal: the group and
        //    the descendant are provably gone without any test-side kill.
        wait_for_group_death(anchor_pid).await;
        let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
            .expect("descendant pid file")
            .trim()
            .parse()
            .expect("descendant pid");
        wait_for_process_death(descendant_pid).await;
        let _ = dir;
    }

    /// The control-channel abandonment regression: the rustX-side owner of
    /// the invocation disappears (the execution future is dropped, closing
    /// the rustX end of the control channel) while the owned tree is
    /// running. The inner supervisor interprets the channel EOF as a
    /// fail-safe instruction to contain the invocation, and the outer
    /// supervisor terminates the owned group. Dropping the rustX-side
    /// execution future can therefore never detach an uncontrolled Bash
    /// tree; the test performs no process control of its own.
    #[cfg(unix)]
    #[tokio::test]
    async fn control_channel_abandonment_contains_the_owned_tree() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let desc_pid_file = root.join("desc.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let command = format!(
            "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; wait",
            shell_pid_file.display(),
            desc_pid_file.display()
        );
        let task = tokio::spawn(run_with_control(
            command,
            BashTestControl::new().anchor_pid_file(anchor_pid_file.clone()),
            CancellationSignal::new(),
            artifacts,
            workspace,
            None,
        ));
        // The owned tree provably exists before the owner disappears.
        for _ in 0..1000 {
            if shell_pid_file.exists() && anchor_pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(shell_pid_file.exists(), "the shell pid file never appeared");
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
            .expect("descendant pid file")
            .trim()
            .parse()
            .expect("descendant pid");
        assert!(
            process_alive(descendant_pid),
            "the descendant must be alive when the owner disappears"
        );
        // The execution owner disappears: the future is dropped with no
        // cancellation request and no result. The test is about ownership
        // containment, not about a returned ToolExecutionResult.
        task.abort();
        let _ = task.await;
        // The supervisor fail-safe-contained the invocation: the
        // descendant and the whole owned group are provably gone without
        // any test-side kill.
        wait_for_process_death(descendant_pid).await;
        wait_for_group_death(anchor_pid).await;
        let _ = dir;
    }

    /// The fallback-containment counterpart of the unrelated-process
    /// regression: when the inner supervisor fails to signal and the outer
    /// supervisor must contain the invocation, only the invocation's own
    /// session-isolated process group is terminated; an unrelated process
    /// in the test's own process group survives.
    #[cfg(unix)]
    #[tokio::test]
    async fn fallback_containment_does_not_kill_unrelated_processes() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let anchor_pid_file = root.join("anchor.pid");
        // An unrelated process in the test's own process group.
        let unrelated = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("unrelated sleep");
        let unrelated_pid = unrelated.id();
        let command = format!("echo $$ > {}; sleep 30", shell_pid_file.display());
        let cancellation = CancellationSignal::new();
        let cancelling = cancellation.clone();
        let task = tokio::spawn(run_with_control(
            command,
            BashTestControl::new()
                .fail_signal()
                .anchor_pid_file(anchor_pid_file.clone()),
            cancellation,
            artifacts.clone(),
            workspace.clone(),
            None,
        ));
        // The owned shell provably started before cancellation.
        for _ in 0..1000 {
            if shell_pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(shell_pid_file.exists(), "the shell pid file never appeared");
        cancelling.cancel();
        let result = tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("the invocation settles")
            .expect("executor task");
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "an injected signaling failure must be an explicit failed result, got {:?}",
            result.status
        );
        // The unrelated process in the test's own process group survived
        // the fallback containment of the invocation's session-isolated
        // group.
        let mut unrelated = unrelated;
        assert!(
            unrelated.try_wait().expect("try_wait").is_none(),
            "the unrelated process (pid {unrelated_pid}) must survive fallback containment"
        );
        let _ = unrelated.kill();
        let _ = unrelated.wait();
        // The owned group is terminal.
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        wait_for_group_death(anchor_pid).await;
        let _ = dir;
    }

    /// The positive counterpart: during a real cancellation every emitted
    /// process-group signal targets exactly the invocation's own pgid (the
    /// inner supervisor's pid) and only occurs while the ownership anchor
    /// is held.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_signals_only_target_the_owned_group() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let command = format!(
            "trap '' TERM; echo $$ > {}; sleep 30",
            shell_pid_file.display()
        );
        let control = BashTestControl::new().anchor_pid_file(anchor_pid_file.clone());
        let cancellation = CancellationSignal::new();
        let cancelling = cancellation.clone();
        let task = tokio::spawn(run_with_control(
            command,
            control.clone(),
            cancellation,
            artifacts.clone(),
            workspace.clone(),
            None,
        ));
        for _ in 0..1000 {
            if shell_pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(shell_pid_file.exists(), "the shell pid file never appeared");
        cancelling.cancel();
        let result = tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("the invocation settles")
            .expect("executor task");
        assert!(
            matches!(result.status, ToolExecutionStatus::Cancelled { .. }),
            "a TERM-ignoring shell is KILLed and the cancellation settles, got {:?}",
            result.status
        );
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        let recorded = control.recorded_signals();
        assert!(
            !recorded.is_empty(),
            "the termination path must have emitted TERM and/or KILL"
        );
        for attempt in &recorded {
            assert!(
                attempt.emitted,
                "every attempt during a held anchor is emitted: {attempt:?}"
            );
            assert_eq!(
                attempt.pgid, anchor_pid,
                "every emitted signal targets the owned process-group id"
            );
        }
        wait_for_group_death(anchor_pid).await;
        let _ = dir;
    }

    /// The descendant-replacement race regression: A (a subshell) creates B
    /// (a redirected descendant), A exits, B remains owned. At the exact
    /// shell-exit boundary the executor is parked with B provably alive, so
    /// the invocation cannot settle; only the invocation timeout can settle
    /// it, and only after the supervisor has reaped B. This is the race the
    /// old `/proc` walk could not prove: settlement is gated on the
    /// supervisor's kernel child-wait terminal state, not on an
    /// observational membership scan.
    #[cfg(unix)]
    #[tokio::test]
    async fn descendant_replacement_keeps_the_invocation_active_until_reaped() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let a_pid_file = root.join("a.pid");
        let b_pid_file = root.join("b.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let command = format!(
            "(sleep 30 >/dev/null 2>&1 & echo $! > {}) & echo $! > {}; wait; exit 0",
            b_pid_file.display(),
            a_pid_file.display()
        );
        let control = BashTestControl::new()
            .pause_at_shell_exit()
            .anchor_pid_file(anchor_pid_file.clone());
        let hook = control.lifecycle().clone();
        let task = tokio::spawn(run_with_control(
            command,
            control,
            CancellationSignal::new(),
            artifacts.clone(),
            workspace.clone(),
            Some(800),
        ));
        // 1. The exact boundary: the shell exited after waiting for A; the
        //    executor is parked before any settlement handling.
        tokio::time::timeout(Duration::from_secs(15), hook.await_shell_exit())
            .await
            .expect("the shell-exit boundary is observed");
        // 2. A is gone and B (A's replacement) is provably still alive and
        //    owned by the invocation supervisor.
        let a_pid: i32 = std::fs::read_to_string(&a_pid_file)
            .expect("a pid file")
            .trim()
            .parse()
            .expect("a pid");
        let b_pid: i32 = std::fs::read_to_string(&b_pid_file)
            .expect("b pid file")
            .trim()
            .parse()
            .expect("b pid");
        assert!(
            !process_alive(a_pid),
            "A must be terminal at the shell-exit boundary"
        );
        assert!(
            process_alive(b_pid),
            "B must still be owned and alive at the shell-exit boundary"
        );
        // 3. The executor resumes; the invocation must NOT settle while B is
        //    owned — only the invocation timeout can settle it.
        hook.release();
        let result = tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("the invocation settles exactly once")
            .expect("executor task");
        assert_eq!(
            result.status,
            ToolExecutionStatus::TimedOut,
            "the invocation must stay active while the supervisor owns B"
        );
        // 4. After the termination the whole owned domain is terminal.
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        wait_for_group_death(anchor_pid).await;
        wait_for_process_death(b_pid).await;
        let _ = dir;
    }
}
