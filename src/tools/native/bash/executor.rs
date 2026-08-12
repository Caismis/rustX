//! The native Bash executor: the complete invocation lifecycle.
//!
//! See the [module documentation](super) for the ownership, settlement, and
//! capture contracts this executor implements.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;

#[cfg(test)]
use super::capture::CaptureHold;
use super::capture::{
    BashProcessControlError, CapturePark, PreviewCapture, await_drain, consume_combined,
    spool_stream,
};
#[cfg(test)]
use crate::runtime::process_runner::RunnerTestControl;
use crate::runtime::process_runner::{
    ProcessOutcomeIntent, RunnerSpawnError, SupervisedCommandRunner, SupervisedCommandSpec,
    unix_signal_of,
};
#[cfg(test)]
use crate::runtime::process_runner::{
    RunnerChannelEofHook as ChannelEofHook, RunnerLifecycleHook as BashLifecycleHook,
    RunnerTerminalHold as TerminalHold,
};
use crate::runtime::types::CancellationReason;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{
    BASH_STREAM_PREVIEW_BYTES, BASH_TERMINATION_CONFIRMATION, DEFAULT_FOREGROUND_BASH_TIMEOUT,
};
use crate::tools::native::support::failed_result;
use crate::tools::types::{
    ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode,
    ToolResultContent, TruncationState,
};

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
        environment: context
            .environment
            .child_environment(context.workspace.root()),
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
    let capture = if let Ok(result) =
        tokio::time::timeout(BASH_TERMINATION_CONFIRMATION, drain_future).await
    {
        Box::new(result)
    } else {
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
        RunnerSpawnError::SupervisorSpawn(message) => {
            format!("cannot spawn the bash supervisor: {message}")
        }
        RunnerSpawnError::InjectedSupervisorSpawn => {
            "injected bash supervisor spawn failure".to_owned()
        }
    }
}
