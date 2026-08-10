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
//! shell, let the supervisor own the invocation's process group to its
//! kernel-mediated terminal state, handle cancellation/timeout with a
//! `TERM` -> grace -> `KILL` sequence inside the supervisor, complete the
//! output draining, finalize the artifacts, and produce a single canonical
//! result.
//!
//! The process-ownership half of this lifecycle is the shared internal
//! supervised command runner (`crate::runtime::process_runner`): the same
//! supervisor unit, control protocol, process-group ownership, cancellation
//! settlement, and catastrophic containment that native Bash uses is reused
//! by Skill environment materialization, so package-manager work never
//! becomes a second independent subprocess hierarchy. This tool owns the
//! capture half: artifact spooling, bounded previews, and the final
//! canonical result formatting.
//!
//! Shell-parent exit is **not** by itself the Bash settlement boundary:
//! the shell may exit while a descendant still belongs to the
//! invocation-owned process group, with the output pipes either still held
//! or already redirected away. The invocation therefore settles naturally
//! only when all three of the following are true:
//!
//! - the shell's terminal status is known (the supervisor reported it);
//! - the invocation-owned process group is terminal (the supervisor's
//!   group-scoped wait reached `ECHILD` and the outer supervisor reported
//!   the authoritative `AllChildrenReaped`);
//! - the runtime-owned output capture is settled.
//!
//! Cancellation and the invocation deadline remain authoritative until the
//! complete lifecycle settles: they trigger the supervisor's
//! `TERM` -> grace -> `KILL` sequence, so a shell-parent exit can never let
//! owned group work escape the timeout/cancellation contract, even when the
//! descendant no longer holds the rustX pipes.
//!
//! # Fixed invocation process group
//!
//! A Bash invocation executes inside one fixed rustX-owned process group.
//! Process-group/session mutation from Bash descendants is rejected so the
//! ownership boundary cannot be escaped or partially hidden: the inner
//! supervisor installs a narrow inherited seccomp policy between its own
//! `setsid()` setup and the `/bin/bash` spawn that rejects `setsid(2)` and
//! `setpgid(2)` with `EPERM` (see [`bash_supervisor`]). `setsid`/`setpgid`
//! are the only syscalls that can change process-group/session membership
//! on Linux, and seccomp filters are inherited across `fork`/`exec` and can
//! only become more restrictive. A command such as `setsid sleep 30`
//! therefore fails deterministically (the utility exits non-zero) and
//! nothing leaves the invocation group.
//!
//! This restriction is what makes the supervisor's kernel child-wait
//! terminal proof complete: an in-domain descendant cannot remain hidden
//! behind an ancestor that left the domain. See the "Ownership boundary"
//! section below and [`bash_supervisor`] for the full argument.
//!
//! # Ownership boundary
//!
//! The Bash invocation's ownership boundary is its dedicated process
//! group. The invocation owns, guarantees termination of, and bases its
//! settlement on exactly the processes that remain in that group. Because
//! group membership is immutable for bash descendants, every process ever
//! spawned by the shell — background children, subshells, replacement
//! processes — remains in the invocation group for its whole lifetime:
//! **there is no way to leave the owned execution domain from inside a
//! Bash command**.
//!
//! # Runtime child-subreaper capability
//!
//! rustX's process-wide `PR_SET_CHILD_SUBREAPER` activation is a runtime
//! coordination-layer capability, not a Bash-local setting and not a
//! generic reaper: it is owned by
//! [`crate::runtime::process_supervision`], activated lazily once,
//! idempotently and sticky, before any Bash ownership exists (before
//! `START` authorizes the Bash spawn), and never toggled per invocation.
//! It exists solely so that a lost Bash supervisor unit's orphaned
//! invocation descendants reparent to the runtime process, where the
//! invocation-scoped catastrophic containment can still retain the inner
//! anchor and prove the invocation group terminal. Kernel reparenting does
//! not expand Bash semantic ownership beyond the invocation process group,
//! and rustX implements no generic unknown-child reaper: catastrophic
//! containment remains invocation-scoped (anchor pid and invocation
//! process group only — never a broad wait).
//!
//! # Terminal results
//!
//! Every Bash `ToolExecutionResult` — `Success`, `Failed`, `Cancelled`,
//! and `TimedOut` alike — is terminal with respect to the invocation-owned
//! process group: no invocation-owned Bash process remains capable of
//! executing work before any result is returned. A detected
//! process-control/runtime failure determines the eventual result status
//! but does not itself settle the invocation lifecycle: owned work is
//! contained and the owned group reaped to either the normal outer terminal
//! event or the reuse-safe catastrophic terminal point (and the capture
//! settled) before the remembered `Failed` result is returned.
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
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::io::AsyncReadExt;

use crate::message::content::FileReference;
use crate::runtime::process_runner::{
    ProcessOutcomeIntent, RunnerSpawnError, SupervisedCommandRunner, SupervisedCommandSpec,
    unix_signal_of,
};
#[cfg(test)]
use crate::runtime::process_runner::RunnerTestControl;
use crate::runtime::types::CancellationReason;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{
    BASH_STREAM_PREVIEW_BYTES, BASH_TERMINATION_CONFIRMATION, DEFAULT_FOREGROUND_BASH_TIMEOUT,
};
#[cfg(test)]
use crate::runtime::process_runner::{
    RunnerChannelEofHook as ChannelEofHook, RunnerLifecycleHook as BashLifecycleHook,
    RunnerTerminalHold as TerminalHold,
};
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
/// locate the invocation's process-group id, and park the output capture —
/// without an operating-system mocking framework.
///
/// The process-ownership seams live in the shared supervised command runner
/// (`crate::runtime::process_runner::RunnerTestControl`); this type wraps
/// them together with the Bash-local capture seams.
#[cfg_attr(test, allow(clippy::struct_excessive_bools))] // a bounded test-seam bundle
#[derive(Clone)]
pub(crate) struct BashTestControl {
    #[cfg(test)]
    runner: RunnerTestControl,
    #[cfg(test)]
    capture_hold: Option<CaptureHold>,
}

/// One attempted process-group signal, recorded by the test seam.
///
/// `emitted` is `true` only when the signal actually reached the kernel
/// (`killpg` was invoked by the supervisor); a refused attempt (ownership
/// lost, injected failure) is recorded with `emitted == false`.
#[cfg(test)]
pub(crate) use crate::runtime::process_runner::RecordedSignal;

#[cfg(test)]
impl BashTestControl {
    /// A control bundle without failures; the lifecycle hook is present but
    /// not armed, so an executor never parks on it unless the test arms it.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            runner: RunnerTestControl::new(),
            capture_hold: None,
        }
    }

    /// The shared supervised-runner control seams.
    #[must_use]
    pub(crate) fn runner_control(&self) -> RunnerTestControl {
        self.runner.clone()
    }

    /// Arms the exact shell-exit boundary: the runner parks after the
    /// supervisor reported the shell's natural exit until the test releases
    /// it.
    #[must_use]
    pub(crate) fn pause_at_shell_exit(mut self) -> Self {
        self.runner.pause_at_shell_exit = true;
        self
    }

    /// The lifecycle hook; tests subscribe to the exact shell-exit
    /// boundary and release the parked runner through it.
    #[must_use]
    pub(crate) fn lifecycle(&self) -> &BashLifecycleHook {
        &self.runner.lifecycle
    }

    /// A shared handle that makes the ownership anchor read as lost from
    /// the start of the invocation: the supervisor then behaves as if the
    /// owned group's lifetime had ended and the numeric pgid might name a
    /// foreign group, so it refuses to signal it.
    #[must_use]
    pub(crate) fn force_anchor_loss_handle(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.runner.force_anchor_loss.clone()
    }

    /// A shared handle that makes the catastrophic emergency containment
    /// observe the invocation anchor as unavailable (not a waitable child
    /// of rustX) without a prior authoritative terminal event. The
    /// semantic state is enough: the regression proves that
    /// `AnchorUnavailable` never settles the invocation as terminal.
    #[must_use]
    pub(crate) fn force_emergency_anchor_unavailable_handle(
        &self,
    ) -> Arc<std::sync::atomic::AtomicBool> {
        self.runner.force_emergency_anchor_unavailable.clone()
    }

    /// Names a file the supervisor writes the invocation's process-group id
    /// into (test observability only).
    #[must_use]
    pub(crate) fn anchor_pid_file(mut self, path: std::path::PathBuf) -> Self {
        self.runner.anchor_pid_file = Some(path);
        self
    }

    /// The recorded process-group signal attempts so far.
    #[must_use]
    pub(crate) fn recorded_signals(&self) -> Vec<RecordedSignal> {
        self.runner.recorded_signals()
    }

    /// Makes the supervisor spawn fail with an injected error.
    #[must_use]
    pub(crate) fn fail_supervisor_spawn(mut self) -> Self {
        self.runner.fail_supervisor_spawn = true;
        self
    }

    /// Makes the bash spawn inside the supervisor fail with an injected
    /// error.
    #[must_use]
    pub(crate) fn fail_bash_spawn(mut self) -> Self {
        self.runner.fail_command_spawn = true;
        self
    }

    /// Makes every group signal in the supervisor fail with an injected
    /// error.
    #[must_use]
    pub(crate) fn fail_signal(mut self) -> Self {
        self.runner.fail_signal = true;
        self
    }

    /// Makes the shell wait in the supervisor fail with an injected error.
    #[must_use]
    pub(crate) fn fail_wait(mut self) -> Self {
        self.runner.fail_wait = true;
        self
    }

    /// Makes the inner supervisor's SIGTERM handler installation fail with
    /// an injected error (a pre-ownership setup failure).
    #[must_use]
    pub(crate) fn fail_sigterm_handler(mut self) -> Self {
        self.runner.fail_sigterm_handler = true;
        self
    }

    /// Makes the runtime child-subreaper initialization fail with an
    /// injected error (a pre-ownership setup failure: no supervisor unit
    /// is spawned, so no Bash tree can exist).
    #[must_use]
    pub(crate) fn fail_subreaper_init(mut self) -> Self {
        self.runner.fail_subreaper_init = true;
        self
    }

    /// Arms the deterministic stuck-capture seam: the stdout output reader
    /// provably parks after EOF and stays open until the invocation's
    /// bounded settlement path force-finalizes it. Test-only; never a
    /// production configuration.
    #[must_use]
    pub(crate) fn hold_stdout_capture(mut self) -> Self {
        self.capture_hold = Some(CaptureHold::new());
        self
    }

    /// The armed capture-hold seam handle (test side).
    #[must_use]
    pub(crate) fn capture_hold(&self) -> Option<&CaptureHold> {
        self.capture_hold.as_ref()
    }

    /// Holds the authoritative terminal event outside the state machine so
    /// the quiescence watchdog can expire while `children_terminal` remains
    /// false, without relying on scheduler timing.
    #[must_use]
    pub(crate) fn hold_terminal_event(mut self) -> Self {
        self.runner.terminal_hold = Some(TerminalHold::new());
        self
    }

    #[must_use]
    pub(crate) fn terminal_hold(&self) -> Option<&TerminalHold> {
        self.runner.terminal_hold.as_ref()
    }

    #[must_use]
    pub(crate) fn observe_channel_eof(mut self) -> Self {
        self.runner.channel_eof = Some(ChannelEofHook::new());
        self
    }

    pub(crate) fn channel_eof(&self) -> Option<&ChannelEofHook> {
        self.runner.channel_eof.as_ref()
    }
}

/// The test-only seam that holds one output reader task open: the stdout
/// reader parks after EOF until the invocation's bounded settlement path
/// force-finalizes it. This is how the regressions prove that a wedged
/// capture can never turn the bounded confirmation contract into an
/// unbounded wait.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct CaptureHold {
    parked_tx: tokio::sync::watch::Sender<bool>,
    parked_rx: tokio::sync::watch::Receiver<bool>,
}

#[cfg(test)]
impl CaptureHold {
    fn new() -> Self {
        let (parked_tx, parked_rx) = tokio::sync::watch::channel(false);
        Self {
            parked_tx,
            parked_rx,
        }
    }

    /// The reader-side handle handed to the stdout spool task.
    fn reader(&self) -> CaptureHoldReader {
        CaptureHoldReader {
            parked: self.parked_tx.clone(),
        }
    }

    /// Test side: waits until the stdout reader provably parked after EOF.
    async fn await_parked(&self) {
        let mut rx = self.parked_rx.clone();
        if !*rx.borrow() {
            let _ = rx.changed().await;
        }
    }
}

/// The reader-side capture-hold handle: parks the stdout spool task after
/// EOF until the bounded settlement path aborts it.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct CaptureHoldReader {
    parked: tokio::sync::watch::Sender<bool>,
}

/// The capture-park handle passed to the output readers: `Some` only in
/// test builds. In non-test builds the seam type is uninhabited, so no
/// reader can ever park.
#[cfg(test)]
type CapturePark = Option<CaptureHoldReader>;
/// See [`CapturePark`]: the non-test seam is uninhabited.
#[cfg(not(test))]
type CapturePark = Option<std::convert::Infallible>;

/// A process-control failure of the owned Bash invocation.
///
/// The ownership-half failure kinds are owned by the shared supervised
/// command runner (`crate::runtime::process_runner::ProcessControlError`);
/// the Bash-local half is the bounded capture settlement failure.
///
/// Supervisor setup, signaling, waiting, and IPC failures never silently
/// fail: a failure that undermines ownership or settlement surfaces as an
/// explicit failed tool result, never as an ordinary `Success`,
/// `Cancelled`, or `TimedOut`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BashProcessControlError {
    /// The output capture did not settle within the bounded confirmation
    /// window after the owned process tree reached its terminal state.
    /// This is the bounded settlement escape hatch for a wedged capture:
    /// the reader tasks are force-finalized and the invocation settles as
    /// an explicit bounded failure — the confirmation contract is a real
    /// bound, never an unbounded wait.
    CaptureTimeout,
}

impl core::fmt::Display for BashProcessControlError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CaptureTimeout => write!(
                f,
                "the bash output capture did not settle within the bounded confirmation window"
            ),
        }
    }
}

/// The three captured stream references of one invocation.
type StreamReferences = (
    Option<FileReference>,
    Option<FileReference>,
    Option<FileReference>,
);

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

/// The Unix-only half of [`run_bash`]: spawns the invocation supervisor
/// through the shared supervised command runner, supervises the capture,
/// and settles the invocation.
#[cfg(unix)]
#[allow(clippy::too_many_lines)] // one coherent spawn/capture/settle pipeline
async fn run_bash_unix(
    command: &str,
    timeout: Option<Duration>,
    context: &ToolExecutionContext<'_>,
    control: Option<&BashTestControl>,
) -> ToolExecutionResult {
    #[cfg(not(test))]
    let _ = control;
    // The process-ownership half of the invocation (supervisor spawn, the
    // control protocol, cancellation/timeout settlement, catastrophic
    // containment, and the direct-child reap) lives in the shared internal
    // supervised command runner; this tool owns the capture and the
    // canonical result formatting.
    #[cfg(test)]
    let runner_control = control.map(BashTestControl::runner_control);
    #[cfg(not(test))]
    let runner_control = None;
    let spec = SupervisedCommandSpec {
        command: command.to_owned(),
        cwd: context.workspace.root().to_path_buf(),
        environment: context.environment.child_environment(context.workspace.root()),
        timeout,
        cancellation: context.cancellation.clone(),
    };
    let (mut runner, stdout_pipe, stderr_pipe) =
        match SupervisedCommandRunner::spawn(&spec, runner_control) {
            Ok(parts) => parts,
            Err(error) => return failed_result(supervisor_spawn_failure(&error)),
        };

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
        #[cfg(test)]
        let stdout_park: CapturePark = control
            .and_then(|control| control.capture_hold())
            .map(CaptureHold::reader);
        #[cfg(not(test))]
        let stdout_park: CapturePark = None;
        stdout_task = Some(tokio::spawn(spool_stream(
            pipe,
            context.artifacts.clone(),
            stdout_capture.clone(),
            combined_tx.clone(),
            0,
            "stdout",
            stdout_park,
        )));
    }
    if let Some(pipe) = stderr_pipe {
        let stderr_park: CapturePark = None;
        stderr_task = Some(tokio::spawn(spool_stream(
            pipe,
            context.artifacts.clone(),
            stderr_capture.clone(),
            combined_tx.clone(),
            1,
            "stderr",
            stderr_park,
        )));
    }
    drop(combined_tx);

    // The runner drives the owned process tree to its terminal state. The
    // outcome intent and lifecycle settlement are distinct: the runner
    // returns only when an outcome intent (failure, cancellation/timeout,
    // or the shell's natural status) is known and the owned child set is
    // terminal. Shell-parent exit is not settlement, and neither is a
    // detected failure: owned work must be contained and terminal before
    // any result — `Success`, `Failed`, `Cancelled`, or `TimedOut` — is
    // returned.
    let termination = runner.settle().await;

    // Once the outcome intent is known and the owned child set is
    // terminal, the output capture must settle within the same bounded
    // window: a capture wedged on reader or artifact I/O must not turn
    // the contract into an unbounded wait.
    let mut capture_failure: Option<String> = None;
    let drain_future = await_drain(&mut stdout_task, &mut stderr_task, &mut combined_task);
    let capture = match tokio::time::timeout(BASH_TERMINATION_CONFIRMATION, drain_future).await {
        Ok(result) => Box::new(result),
        Err(_) => {
            capture_failure = Some(BashProcessControlError::CaptureTimeout.to_string());
            if let Some(handle) = &stdout_task {
                handle.abort();
            }
            if let Some(handle) = &stderr_task {
                handle.abort();
            }
            combined_task.abort();
            Box::new(Err(
                "the bash output capture was force-finalized after the bounded settlement window"
                    .to_owned(),
            ))
        }
    };

    let mut process_failure = match &termination.intent {
        ProcessOutcomeIntent::ProcessControlFailed(message) => Some(message.clone()),
        _ => None,
    };
    let (stdout_reference, stderr_reference, combined_reference) = match *capture {
        Ok(references) => references,
        Err(error) => {
            // The outcome is already owned (failure or cancellation/
            // timeout): the capture of a terminated process tree is
            // inherently partial and is never reported as successful
            // retention. The root-cause failure is never overwritten by the
            // later capture condition; at most the capture detail is
            // appended to it.
            if let Some(message) = process_failure.as_mut() {
                message.push_str("; output capture: ");
                message.push_str(&error);
            } else if let Some(message) = capture_failure.as_mut() {
                message.push_str("; output capture: ");
                message.push_str(&error);
            }
            let outcome_owned = process_failure.is_some()
                || capture_failure.is_some()
                || !matches!(termination.intent, ProcessOutcomeIntent::Completed);
            if outcome_owned {
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
    if let Some(failure_message) = process_failure {
        status = ToolExecutionStatus::Failed {
            error: format!("bash process control failed: {failure_message}"),
        };
    } else if let Some(failure_message) = capture_failure {
        status = ToolExecutionStatus::Failed {
            error: format!("bash process control failed: {failure_message}"),
        };
    } else if matches!(termination.intent, ProcessOutcomeIntent::Cancelled) {
        // Cancellation/timeout owns settlement and wins over any partial
        // natural exit data.
        status = ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested,
        };
    } else if matches!(termination.intent, ProcessOutcomeIntent::TimedOut) {
        status = ToolExecutionStatus::TimedOut;
    } else if let Some(exit) = termination.exit_status {
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

/// The canonical failed-result message of one supervised-command spawn
/// failure, preserving the M5 diagnostic text.
fn supervisor_spawn_failure(error: &RunnerSpawnError) -> String {
    match error {
        RunnerSpawnError::Subreaper(message) => {
            format!("cannot establish rustX Bash fallback containment: {message}")
        }
        RunnerSpawnError::ControlChannel(message) => {
            format!("cannot create the bash supervisor control channel: {message}")
        }
        RunnerSpawnError::ControlChannelPrepare(message) => {
            format!("cannot prepare the bash supervisor control channel: {message}")
        }
        RunnerSpawnError::ControlChannelOpen(message) => {
            format!("cannot open the bash supervisor control channel: {message}")
        }
        RunnerSpawnError::SupervisorSpawn(message) => {
            format!("cannot spawn the bash supervisor: {message}")
        }
        RunnerSpawnError::InjectedSupervisorSpawn => "injected bash supervisor spawn failure".to_owned(),
    }
}

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
    park: CapturePark,
) -> Result<Option<FileReference>, String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    #[cfg(not(test))]
    let _ = park;
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
    #[cfg(test)]
    if let Some(park) = park {
        // The deterministic stuck-capture seam: park provably after EOF and
        // stay open until the bounded settlement path force-finalizes
        // (aborts) this task.
        park.parked.send(true).ok();
        std::future::pending::<()>().await;
    }
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
) -> Result<StreamReferences, String> {
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


#[cfg(test)]
mod tests {
    use super::{BashTestControl, BashTool, NAME};
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
    use crate::runtime::process_runner::{
        MSG_ALL_CHILDREN_REAPED, SupervisorEvent, read_supervisor_event,
    };
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
    use crate::tools::types::{
        ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolProgress,
    };
    use crate::tools::workspace::Workspace;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

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

    /// The process-group id of a fixture process, from `/proc/<pid>/stat`
    /// (test-only fixture-topology inspection; `/proc` is never the
    /// production ownership authority).
    #[cfg(unix)]
    fn pgrp_of(pid: i32) -> Option<i32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let close = stat.rfind(')')?;
        let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
        fields.get(2)?.parse().ok()
    }

    #[cfg(target_os = "linux")]
    fn process_capable_of_executing(pid: i32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        let Some(close) = stat.rfind(')') else {
            return false;
        };
        !matches!(stat[close + 1..].split_whitespace().next(), Some("Z" | "X"))
    }

    #[cfg(target_os = "linux")]
    async fn wait_until_not_executing(pid: i32) {
        for _ in 0..1000 {
            if !process_capable_of_executing(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("process {pid} remains capable of executing");
    }

    #[cfg(target_os = "linux")]
    async fn start_supervisor_loss_fixture(
        cancellation: CancellationSignal,
    ) -> (
        tempfile::TempDir,
        tokio::task::JoinHandle<ToolExecutionResult>,
        super::ChannelEofHook,
        i32,
        i32,
        i32,
    ) {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let inner_pid_file = root.join("inner.pid");
        let outer_pid_file = root.join("outer.pid");
        let ready_file = root.join("ready");
        let command = format!(
            "inner=$PPID; outer=$(awk '/^PPid:/ {{print $2}}' /proc/$inner/status); \
             echo $$ > {}; echo $inner > {}; echo $outer > {}; touch {}; \
             exec >/dev/null 2>&1; kill -KILL $outer; kill -KILL $inner; sleep 30",
            shell_pid_file.display(),
            inner_pid_file.display(),
            outer_pid_file.display(),
            ready_file.display()
        );
        let control = BashTestControl::new().observe_channel_eof();
        let eof = control.channel_eof().expect("EOF hook").clone();
        let task = tokio::spawn(run_with_control(
            command,
            control,
            cancellation,
            artifacts,
            workspace,
            None,
        ));
        for _ in 0..1000 {
            if ready_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ready_file.exists(), "supervisor-loss fixture not ready");
        let read_pid = |path: &std::path::Path| {
            std::fs::read_to_string(path)
                .expect("pid file")
                .trim()
                .parse::<i32>()
                .expect("pid")
        };
        let shell_pid = read_pid(&shell_pid_file);
        let inner_pid = read_pid(&inner_pid_file);
        let outer_pid = read_pid(&outer_pid_file);
        wait_until_not_executing(inner_pid).await;
        wait_until_not_executing(outer_pid).await;
        tokio::time::timeout(Duration::from_secs(8), eof.await_seen())
            .await
            .expect("control EOF was not observed");
        assert!(process_alive(shell_pid));
        assert_eq!(pgrp_of(shell_pid), Some(inner_pid));
        (dir, task, eof, shell_pid, inner_pid, outer_pid)
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

    /// A SIGTERM handler-installation failure inside the supervisor is a
    /// pre-ownership setup failure: no bash tree exists, so the explicit
    /// failed result is the correct settlement.
    #[tokio::test]
    async fn sigterm_handler_setup_failure_settles_as_an_explicit_failed_result() {
        let (_dir, artifacts, workspace) = fixture();
        let result = run_with_control(
            "echo hi".to_owned(),
            BashTestControl::new().fail_sigterm_handler(),
            CancellationSignal::new(),
            artifacts,
            workspace,
            None,
        )
        .await;
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "an injected SIGTERM handler failure must be an explicit failed result, got {:?}",
            result.status
        );
        assert!(
            !matches!(result.status, ToolExecutionStatus::Success),
            "a failed SIGTERM handler setup must never be reported as success"
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

    /// The direct `setsid` escape-attempt regression: membership mutation
    /// is rejected for bash descendants (the inherited syscall restriction),
    /// so `setsid sleep 30` cannot leave the invocation group. The `setsid`
    /// utility fails deterministically with `EPERM` and exits non-zero; the
    /// recorded pid is provably terminal afterwards — nothing escaped the
    /// owned domain — and the shell's natural exit settles ordinary
    /// `Success` once the owned group is terminal.
    #[cfg(unix)]
    #[tokio::test]
    async fn setsid_escape_attempt_is_rejected_and_nothing_escapes() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let attempt_pid_file = root.join("attempt.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let command = format!(
            "echo $$ > {}; setsid sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
            shell_pid_file.display(),
            attempt_pid_file.display()
        );
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            run_with_control(
                command,
                BashTestControl::new().anchor_pid_file(anchor_pid_file.clone()),
                CancellationSignal::new(),
                artifacts,
                workspace,
                Some(10_000),
            ),
        )
        .await
        .expect("the invocation settles exactly once (bounded)");
        assert_eq!(
            result.status,
            ToolExecutionStatus::Success,
            "the rejected escape attempt leaves the owned group terminal; the natural exit settles Success, got {:?}",
            result.status
        );
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
            .expect("shell pid file")
            .trim()
            .parse()
            .expect("shell pid");
        let attempt_pid: i32 = std::fs::read_to_string(&attempt_pid_file)
            .expect("attempt pid file")
            .trim()
            .parse()
            .expect("attempt pid");
        // The owned group is terminal and the shell is gone; the escaped
        // `sleep` never came into existence — the recorded attempt pid is
        // provably dead instead of alive out of domain.
        wait_for_group_death(anchor_pid).await;
        wait_for_process_death(shell_pid).await;
        wait_for_process_death(attempt_pid).await;
        let _ = dir;
    }

    /// The direct `setsid` escape-attempt timeout regression: the shell
    /// stays alive in the owned group while the rejected `setsid` attempt
    /// cannot leave it. The invocation timeout owns the outcome and settles
    /// `TimedOut` in bounded time with the whole owned group terminal; the
    /// recorded attempt pid is provably dead afterwards.
    #[cfg(unix)]
    #[tokio::test]
    async fn setsid_escape_attempt_times_out_with_the_owned_group() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let attempt_pid_file = root.join("attempt.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let command = format!(
            "echo $$ > {}; setsid sleep 30 >/dev/null 2>&1 & echo $! > {}; sleep 30",
            shell_pid_file.display(),
            attempt_pid_file.display()
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
        .expect("the invocation settles exactly once (bounded)");
        assert_eq!(
            result.status,
            ToolExecutionStatus::TimedOut,
            "the timeout owns the owned group; the rejected attempt must not leave anything, got {:?}",
            result.status
        );
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
            .expect("shell pid file")
            .trim()
            .parse()
            .expect("shell pid");
        let attempt_pid: i32 = std::fs::read_to_string(&attempt_pid_file)
            .expect("attempt pid file")
            .trim()
            .parse()
            .expect("attempt pid");
        wait_for_group_death(anchor_pid).await;
        wait_for_process_death(shell_pid).await;
        wait_for_process_death(attempt_pid).await;
        let _ = dir;
    }

    /// The direct `setsid` escape-attempt cancellation regression:
    /// cancellation terminates the owned group — the rejected attempt
    /// cannot survive it — and settles `Cancelled` in bounded time with the
    /// recorded attempt pid provably dead.
    #[cfg(unix)]
    #[tokio::test]
    async fn setsid_escape_attempt_cancels_with_the_owned_group() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let attempt_pid_file = root.join("attempt.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let command = format!(
            "echo $$ > {}; setsid sleep 30 >/dev/null 2>&1 & echo $! > {}; sleep 30",
            shell_pid_file.display(),
            attempt_pid_file.display()
        );
        let cancellation = CancellationSignal::new();
        let cancelling = cancellation.clone();
        let task = tokio::spawn(run_with_control(
            command,
            BashTestControl::new().anchor_pid_file(anchor_pid_file.clone()),
            cancellation,
            artifacts.clone(),
            workspace.clone(),
            None,
        ));
        // The shell provably started before cancellation becomes observable.
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
            .expect("the invocation settles exactly once (bounded)")
            .expect("executor task");
        assert!(
            matches!(result.status, ToolExecutionStatus::Cancelled { .. }),
            "cancellation owns the owned group; the rejected attempt must not survive it, got {:?}",
            result.status
        );
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        let attempt_pid: i32 = std::fs::read_to_string(&attempt_pid_file)
            .expect("attempt pid file")
            .trim()
            .parse()
            .expect("attempt pid");
        wait_for_group_death(anchor_pid).await;
        wait_for_process_death(attempt_pid).await;
        let _ = dir;
    }

    /// The mandatory hidden-grandchild regression (reproducer): A (a
    /// subshell) creates B (a redirected descendant), A itself attempts to
    /// leave the invocation group/session via `exec setsid`, the main shell
    /// exits. The invocation must NOT settle while B is owned: no canonical
    /// result may become terminal while any process still belongs to the
    /// invocation-owned process group. At the exact shell-exit boundary the
    /// test proves the fixture topology before evaluating settlement.
    #[cfg(unix)]
    #[tokio::test]
    async fn hidden_group_descendant_cannot_be_hidden_by_a_setsid_escape_attempt() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let a_pid_file = root.join("a.pid");
        let b_pid_file = root.join("b.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let command = format!(
            "echo $$ > {}; sh -c 'sleep 30 >/dev/null 2>&1 & echo $! > {}; \
             exec setsid sleep 30 >/dev/null 2>&1' & echo $! > {}; exit 0",
            shell_pid_file.display(),
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
        // 1. The exact shell-exit boundary: the main shell exited after
        //    backgrounding A; the executor is parked before any settlement
        //    handling.
        tokio::time::timeout(Duration::from_secs(15), hook.await_shell_exit())
            .await
            .expect("the shell-exit boundary is observed");
        // 2. A and B provably exist (A creates B before its own escape
        //    attempt). The poll queries the fixture's own pid files — the
        //    authoritative process state — with a strict deadlock guard.
        for _ in 0..1000 {
            if a_pid_file.exists() && b_pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
            .expect("shell pid file")
            .trim()
            .parse()
            .expect("shell pid");
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
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        assert!(
            !process_alive(shell_pid),
            "the shell parent must be terminal at the boundary"
        );
        assert!(
            process_alive(b_pid),
            "B must still be alive at the shell-exit boundary"
        );
        // 3. The authoritative fixture topology: B belongs to the
        //    invocation-owned process group, and A — if it is still alive —
        //    must belong to it too. No state may exist where A is out of
        //    group while a live B remains hidden inside the owned group.
        assert_eq!(
            pgrp_of(b_pid),
            Some(anchor_pid),
            "B must still belong to the invocation-owned process group"
        );
        if process_alive(a_pid) {
            assert_eq!(
                pgrp_of(a_pid),
                Some(anchor_pid),
                "A must still belong to the invocation-owned process group"
            );
        }
        // 4. The executor resumes; the invocation must NOT settle while B
        //    is owned — only the invocation timeout can settle it.
        hook.release();
        let result = tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("the invocation settles exactly once")
            .expect("executor task");
        assert_eq!(
            result.status,
            ToolExecutionStatus::TimedOut,
            "the invocation must stay active while B is owned, got {:?}",
            result.status
        );
        // 5. After the timeout-driven termination the whole owned domain is
        //    terminal: B and the group are gone (A either died in the group
        //    or was terminated with it).
        wait_for_process_death(b_pid).await;
        wait_for_group_death(anchor_pid).await;
        let _ = dir;
    }

    /// The bounded-settlement regression: the stdout capture reader is held
    /// open deterministically past the bounded confirmation window (the
    /// test-only seam, never a production configuration). The owned process
    /// tree is already terminal, so only the capture can be wedged; the
    /// state machine must still settle within the strict outer bound — the
    /// capture is force-finalized (the reader task is aborted) and the
    /// invocation settles as an explicit bounded `Failed`. No unbounded
    /// wait remains.
    #[cfg(unix)]
    #[tokio::test]
    async fn stuck_capture_settles_boundedly_as_an_explicit_failure() {
        let (dir, artifacts, workspace) = fixture();
        let control = BashTestControl::new().hold_stdout_capture();
        let hold = control.capture_hold().expect("capture hold seam").clone();
        let task = tokio::spawn(run_with_control(
            "echo hello".to_owned(),
            control,
            CancellationSignal::new(),
            artifacts.clone(),
            workspace.clone(),
            None,
        ));
        // The stdout reader provably parked after EOF; the shell exited and
        // the owned group is terminal, so only the capture can be wedged.
        tokio::time::timeout(Duration::from_secs(15), hold.await_parked())
            .await
            .expect("the stdout reader parks after EOF");
        // The bounded confirmation window expires into an explicit bounded
        // failure within the strict outer bound.
        let result = tokio::time::timeout(Duration::from_secs(25), task)
            .await
            .expect("the invocation settles within the strict outer bound")
            .expect("executor task");
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "a wedged capture must settle as an explicit bounded failure, got {:?}",
            result.status
        );
        let _ = dir;
    }

    /// The process-confirmation watchdog records failure intent but cannot
    /// commit a result before the authoritative terminal event is admitted.
    #[cfg(unix)]
    #[tokio::test]
    async fn quiescence_watchdog_cannot_bypass_process_terminality() {
        let (dir, artifacts, workspace) = fixture();
        let ready = workspace.root().join("ready");
        let control = BashTestControl::new().hold_terminal_event();
        let hold = control.terminal_hold().expect("terminal hold seam").clone();
        let cancellation = CancellationSignal::new();
        let task = tokio::spawn(run_with_control(
            format!("touch {}; sleep 30", ready.display()),
            control,
            cancellation.clone(),
            artifacts,
            workspace,
            None,
        ));
        for _ in 0..1000 {
            if ready.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ready.exists(), "the Bash fixture never became ready");
        cancellation.cancel();
        tokio::time::timeout(Duration::from_secs(15), hold.await_held())
            .await
            .expect("the authoritative terminal event is held");
        tokio::time::timeout(Duration::from_secs(15), hold.await_watchdog())
            .await
            .expect("the quiescence watchdog expires");
        assert!(
            !task.is_finished(),
            "no ToolExecutionResult may commit while children_terminal is false"
        );
        hold.release();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the invocation settles after terminality is released")
            .expect("executor task");
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { ref error }
                if error.contains("did not become terminal")),
            "quiescence failure must outrank cancellation after terminality, got {:?}",
            result.status
        );
        let _ = dir;
    }

    /// The stopped-anchor regression: a `SIGSTOP` of the inner supervisor
    /// freezes the whole containment chain (TERMINATE is never processed).
    /// The outer supervisor detects the frozen anchor, un-wedges it with
    /// `SIGKILL`, contains the invocation group, and the cancellation
    /// settles normally with the owned group terminal — the bounded
    /// confirmation path is never reached.
    #[cfg(unix)]
    #[tokio::test]
    async fn stopped_anchor_supervisor_is_contained_by_the_outer() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let anchor_pid_file = root.join("anchor.pid");
        // The fixture freezes its own supervisor: bash's parent is the
        // inner supervisor (the invocation's anchor). `sleep 30` keeps the
        // owned group alive while the anchor is stopped.
        let command = format!(
            "echo $$ > {}; kill -STOP $PPID; sleep 30",
            shell_pid_file.display()
        );
        let cancellation = CancellationSignal::new();
        let cancelling = cancellation.clone();
        let task = tokio::spawn(run_with_control(
            command,
            BashTestControl::new().anchor_pid_file(anchor_pid_file.clone()),
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
            "a frozen anchor must still settle the owned group as Cancelled, got {:?}",
            result.status
        );
        let anchor_pid: i32 = std::fs::read_to_string(&anchor_pid_file)
            .expect("anchor pid file")
            .trim()
            .parse()
            .expect("anchor pid");
        wait_for_group_death(anchor_pid).await;
        let _ = dir;
    }

    /// Catastrophic supervisor-loss reproducer. The shell kills both
    /// supervisors after recording the fixed-group topology, redirects its
    /// pipes, and remains alive. EOF must not settle the invocation.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn supervisor_chain_loss_does_not_make_an_owned_process_terminal() {
        let (dir, task, eof, shell_pid, _, _) =
            start_supervisor_loss_fixture(CancellationSignal::new()).await;
        assert!(
            !task.is_finished(),
            "control EOF fabricated process terminality while owned work was alive"
        );
        eof.release_emergency_containment();
        let result = tokio::time::timeout(Duration::from_secs(8), task)
            .await
            .expect("emergency containment did not settle")
            .expect("Bash task panicked");
        assert!(
            matches!(
            result.status,
            ToolExecutionStatus::Failed { ref error }
                if error.contains("exited before reporting terminal child ownership")
            ),
            "unexpected result: {:?}",
            result.status
        );
        wait_for_process_death(shell_pid).await;
        let _ = dir;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancellation_during_supervisor_loss_settles_failed_after_containment() {
        let cancellation = CancellationSignal::new();
        let (dir, task, eof, shell_pid, _, _) =
            start_supervisor_loss_fixture(cancellation.clone()).await;
        cancellation.cancel();
        assert!(!task.is_finished());
        assert!(process_capable_of_executing(shell_pid));
        eof.release_emergency_containment();
        let result = tokio::time::timeout(Duration::from_secs(8), task)
            .await
            .expect("containment settles")
            .expect("executor task");
        assert!(matches!(
            result.status,
            ToolExecutionStatus::Failed { ref error }
                if error.contains("exited before reporting terminal child ownership")
        ));
        wait_for_process_death(shell_pid).await;
        let _ = dir;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn timeout_during_supervisor_loss_settles_failed_after_containment() {
        let (dir, task, eof, shell_pid, _, _) =
            start_supervisor_loss_fixture(CancellationSignal::new()).await;
        eof.force_timeout();
        assert!(!task.is_finished());
        assert!(process_capable_of_executing(shell_pid));
        eof.release_emergency_containment();
        let result = tokio::time::timeout(Duration::from_secs(8), task)
            .await
            .expect("containment settles")
            .expect("executor task");
        assert!(matches!(
            result.status,
            ToolExecutionStatus::Failed { ref error }
                if error.contains("exited before reporting terminal child ownership")
        ));
        wait_for_process_death(shell_pid).await;
        let _ = dir;
    }

    /// The runtime child-subreaper initialization is a pre-ownership
    /// prerequisite: a failure settles `Failed` with no Bash tree spawned —
    /// catastrophic fallback containment is never assumed after the runtime
    /// once failed to become a subreaper, and `START` can never be sent
    /// without it. The injected failure proves the exact gate: the command
    /// never runs, so its marker file never appears and no process group
    /// signal is ever attempted.
    #[cfg(unix)]
    #[tokio::test]
    async fn subreaper_initialization_failure_is_a_pre_ownership_setup_failure() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let ready = root.join("ready");
        let control = BashTestControl::new().fail_subreaper_init();
        let recorded = control.recorded_signals();
        let result = run_with_control(
            format!("touch {}", ready.display()),
            control,
            CancellationSignal::new(),
            artifacts,
            workspace,
            None,
        )
        .await;
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { ref error }
                if error.contains("fallback containment")),
            "a failed child-subreaper initialization must be an explicit pre-ownership failure, got {:?}",
            result.status
        );
        assert!(
            !ready.exists(),
            "no Bash tree may be spawned after a child-subreaper initialization failure"
        );
        assert!(
            recorded.is_empty(),
            "no process-group signal may be attempted without subreaper authority"
        );
        let _ = dir;
    }

    /// The mandatory emergency-anchor-unavailable regression: catastrophic
    /// emergency containment starts with `process_lifecycle == Owned`, the
    /// anchor unavailable, and no prior `AllChildrenReaped`. The emergency
    /// containment must NOT return `TerminalProven` (anchor `ECHILD` is
    /// never a terminal process-group proof), so no `ToolExecutionResult`
    /// may commit while the owned group still executes. The semantic state
    /// is enough — no actual pid reuse is required.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn emergency_anchor_unavailable_never_settles_an_owned_invocation() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let inner_pid_file = root.join("inner.pid");
        let outer_pid_file = root.join("outer.pid");
        let anchor_pid_file = root.join("anchor.pid");
        let ready_file = root.join("ready");
        // The fixture kills both supervisors, then becomes a single
        // long-lived owned process (`exec sleep 30`: the shell replaces
        // itself, so the group holds exactly one process with a known pid).
        let command = format!(
            "inner=$PPID; outer=$(awk '/^PPid:/ {{print $2}}' /proc/$inner/status); \
             echo $$ > {}; echo $inner > {}; echo $outer > {}; touch {}; \
             exec >/dev/null 2>&1; kill -KILL $outer; kill -KILL $inner; exec sleep 30",
            shell_pid_file.display(),
            inner_pid_file.display(),
            outer_pid_file.display(),
            ready_file.display()
        );
        let control = BashTestControl::new()
            .observe_channel_eof()
            .anchor_pid_file(anchor_pid_file.clone());
        control
            .force_emergency_anchor_unavailable_handle()
            .store(true, Ordering::SeqCst);
        let eof = control.channel_eof().expect("EOF hook").clone();
        let mut task = tokio::spawn(run_with_control(
            command,
            control.clone(),
            CancellationSignal::new(),
            artifacts,
            workspace,
            None,
        ));
        for _ in 0..1000 {
            if ready_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ready_file.exists(), "the fixture never became ready");
        let read_pid = |path: &std::path::Path| {
            std::fs::read_to_string(path)
                .expect("pid file")
                .trim()
                .parse::<i32>()
                .expect("pid")
        };
        let shell_pid = read_pid(&shell_pid_file);
        let inner_pid = read_pid(&inner_pid_file);
        wait_until_not_executing(inner_pid).await;
        tokio::time::timeout(Duration::from_secs(8), eof.await_seen())
            .await
            .expect("control EOF was not observed");
        assert!(
            process_capable_of_executing(shell_pid),
            "the owned group must still be executing when emergency containment runs"
        );
        // Emergency containment runs with the anchor unavailable; the
        // seam'd semantic state is the deterministic proof.
        eof.release_emergency_containment();
        // The invocation must NOT settle: `AnchorUnavailable` is never a
        // terminal proof and the lifecycle stays non-terminal.
        let still_pending = tokio::time::timeout(Duration::from_secs(2), &mut task)
            .await
            .is_err();
        assert!(
            still_pending,
            "an unavailable emergency anchor must never settle the owned invocation"
        );
        assert!(
            control.recorded_signals().is_empty(),
            "no process-group signal may be issued when the anchor is unavailable"
        );
        // Test-side cleanup (the invocation itself is provably non-terminal
        // by design in this state): terminate the owned group and reap the
        // adopted processes so no fixture process survives the test. The
        // emergency path correctly never consumed them, so the test reaps
        // them directly.
        nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(inner_pid),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("test terminates the owned group");
        nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(inner_pid), None)
            .expect("reap the adopted anchor");
        nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(shell_pid), None)
            .expect("reap the adopted shell");
        task.abort();
        let _ = dir;
    }

    /// The normal-terminal-before-EOF regression: the authoritative
    /// `AllChildrenReaped` frame is parsed first, then EOF follows (the
    /// outer exits after the terminal acknowledgement). The invocation is
    /// already terminal: the late EOF and the intentionally released anchor
    /// behind it never trigger emergency containment and never override the
    /// natural result with a failure.
    #[cfg(unix)]
    #[tokio::test]
    async fn terminal_frame_then_eof_never_overrides_terminality() {
        let (dir, artifacts, workspace) = fixture();
        let control = BashTestControl::new()
            .hold_terminal_event()
            .observe_channel_eof();
        let hold = control.terminal_hold().expect("terminal hold").clone();
        let eof = control.channel_eof().expect("EOF hook").clone();
        let task = tokio::spawn(run_with_control(
            "echo hello".to_owned(),
            control.clone(),
            CancellationSignal::new(),
            artifacts,
            workspace,
            None,
        ));
        // 1. The authoritative terminal frame is parsed (its state
        //    transition is test-held only).
        tokio::time::timeout(Duration::from_secs(15), hold.await_held())
            .await
            .expect("the terminal frame is parsed");
        // 2. EOF provably arrives while terminality is already admitted;
        //    the EOF branch must skip emergency containment entirely.
        tokio::time::timeout(Duration::from_secs(15), eof.await_seen())
            .await
            .expect("EOF is observed after the terminal frame");
        assert!(!task.is_finished());
        // 3. Release: the invocation settles with the shell's natural
        //    result — no failure override merely because EOF followed the
        //    terminal frame.
        hold.release();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the invocation settles")
            .expect("executor task");
        assert_eq!(
            result.status,
            ToolExecutionStatus::Success,
            "the terminal frame remains authoritative; late EOF must not override it, got {:?}",
            result.status
        );
        assert!(
            control.recorded_signals().is_empty(),
            "no containment signal may follow an already-admitted terminal frame"
        );
        let _ = dir;
    }

    /// The concurrent catastrophic isolation regression: two independent
    /// Bash invocations (A and B) both lose their supervisor units while
    /// live owned descendants remain. Emergency containment of A must
    /// signal and reap only group A: B's process group stays alive and
    /// untouched, and only B's own emergency containment terminates it.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn concurrent_supervisor_loss_containment_is_isolated() {
        let (dir_a, task_a, eof_a, shell_a, inner_a, _) =
            start_supervisor_loss_fixture(CancellationSignal::new()).await;
        let (dir_b, task_b, eof_b, shell_b, inner_b, _) =
            start_supervisor_loss_fixture(CancellationSignal::new()).await;
        assert!(process_capable_of_executing(shell_a));
        assert!(process_capable_of_executing(shell_b));
        // Contain A: B must remain completely untouched.
        eof_a.release_emergency_containment();
        let result_a = tokio::time::timeout(Duration::from_secs(8), task_a)
            .await
            .expect("invocation A settles")
            .expect("executor task A");
        assert!(matches!(
            result_a.status,
            ToolExecutionStatus::Failed { ref error }
                if error.contains("exited before reporting terminal child ownership")
        ));
        wait_for_process_death(shell_a).await;
        wait_for_group_death(inner_a).await;
        assert!(
            process_capable_of_executing(shell_b),
            "containing invocation A must never signal or reap invocation B"
        );
        // Contain B: only now does B become terminal.
        eof_b.release_emergency_containment();
        let result_b = tokio::time::timeout(Duration::from_secs(8), task_b)
            .await
            .expect("invocation B settles")
            .expect("executor task B");
        assert!(matches!(
            result_b.status,
            ToolExecutionStatus::Failed { ref error }
                if error.contains("exited before reporting terminal child ownership")
        ));
        wait_for_process_death(shell_b).await;
        wait_for_group_death(inner_b).await;
        let _ = (dir_a, dir_b);
    }

    /// The foreign-adopted-child negative isolation regression. U is a
    /// **test-created foreign/unregistered hierarchy**: kernel subreaper
    /// adoption makes the runtime process its OS parent, but U is outside
    /// Bash semantic ownership and is not a supported production
    /// rustX-owned execution in M5. The regression proves that catastrophic
    /// Bash containment for invocation group G never touches U — it
    /// signals only G's pgid and reaps only G's adopted children, never a
    /// broad wait. U's cleanup is intentionally owned by the test (rustX
    /// does not claim to generically reap unknown adopted children), and
    /// the test reaps U before returning.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn bash_catastrophic_containment_does_not_touch_foreign_adopted_child() {
        crate::runtime::process_supervision::ensure_child_subreaper()
            .expect("the runtime process is a child subreaper");
        let (dir, _artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let u_pid_file = root.join("u.pid");
        // U: a test-created foreign hierarchy whose parent exits
        // immediately, so U orphans and reparents to the runtime process
        // (the nearest subreaper ancestor — the test binary itself).
        let mut sh = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "sleep 30 >/dev/null 2>&1 & echo $! > {}",
                u_pid_file.display()
            ))
            .spawn()
            .expect("spawn U's parent");
        let status = sh.wait().expect("U's parent exits");
        assert!(status.success());
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let u_pid: i32 = loop {
            if let Ok(content) = std::fs::read_to_string(&u_pid_file) {
                break content.trim().parse().expect("u pid");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "U's pid file never appeared"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        // U is adopted by the runtime process (test-only /proc fixture
        // inspection; /proc is never the production ownership authority).
        let self_pid = i32::try_from(std::process::id()).expect("pid fits i32");
        loop {
            let parent = std::fs::read_to_string(format!("/proc/{u_pid}/stat"))
                .ok()
                .and_then(|stat| {
                    let close = stat.rfind(')')?;
                    stat[close + 1..].split_whitespace().nth(1)?.parse().ok()
                });
            if parent == Some(self_pid) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "U was never adopted by the runtime process (parent: {parent:?})"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(process_alive(u_pid), "U must be alive before containment");
        // The catastrophic Bash invocation G loses both supervisors with
        // live owned work.
        let (dir_g, task, eof, shell_pid, inner_pid, _) =
            start_supervisor_loss_fixture(CancellationSignal::new()).await;
        eof.release_emergency_containment();
        let result = tokio::time::timeout(Duration::from_secs(8), task)
            .await
            .expect("invocation G settles")
            .expect("executor task G");
        assert!(matches!(
            result.status,
            ToolExecutionStatus::Failed { ref error }
                if error.contains("exited before reporting terminal child ownership")
        ));
        wait_for_process_death(shell_pid).await;
        wait_for_group_death(inner_pid).await;
        // U is untouched: still alive and still adopted by the runtime
        // process. Bash containment is scoped; M5 deliberately does not
        // reap foreign adopted children.
        assert!(
            process_alive(u_pid),
            "Bash catastrophic containment must never signal or reap a foreign adopted child"
        );
        // Test-side cleanup of U: the test is U's cleanup owner. This is
        // not missing production behavior — rustX does not generically
        // reap unknown adopted children in M5.
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(u_pid),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("test terminates U");
        nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(u_pid), None).expect("reap U");
        let _ = (dir, dir_g);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminal_frame_is_parsed_before_buffered_eof() {
        let (mut writer, mut reader) = tokio::net::UnixStream::pair().expect("socket pair");
        writer
            .write_all(&[1, 0, 0, 0, MSG_ALL_CHILDREN_REAPED])
            .await
            .expect("terminal frame");
        drop(writer);
        assert!(matches!(
            read_supervisor_event(&mut reader).await,
            Ok(Some(SupervisorEvent::AllChildrenReaped))
        ));
        assert!(matches!(read_supervisor_event(&mut reader).await, Ok(None)));
    }
}
