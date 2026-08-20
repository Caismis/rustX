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
    AppendWatch, BackgroundOutputCapture, BashProcessControlError, CapturePark, PreviewCapture,
    SpillCapture, await_drain, capture_stream, consume_background, consume_combined,
};
use super::input::BashInput;
#[cfg(all(test, target_os = "linux"))]
use crate::runtime::process_runner::RunnerLifecycleHook as BashLifecycleHook;
#[cfg(test)]
use crate::runtime::process_runner::RunnerTestControl;
use crate::runtime::process_runner::{
    ProcessOutcomeIntent, RunnerSpawnError, SupervisedCommandRunner, SupervisedCommandSpec,
    unix_signal_of,
};
#[cfg(test)]
use crate::runtime::process_runner::{
    RunnerChannelEofHook as ChannelEofHook, RunnerTerminalHold as TerminalHold,
};
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{
    BASH_STREAM_PREVIEW_BYTES, BASH_TERMINATION_CONFIRMATION, DEFAULT_FOREGROUND_BASH_TIMEOUT,
};
use crate::tools::native::support::failed_result;
use crate::tools::types::{
    ManagedOutputContinuation, ToolExecutionResult, ToolExecutionStatus, ToolInvocation,
    ToolInvocationMode, ToolResultContent, TruncationState,
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
    /// The background output-append observation seam: after every
    /// committed append to the live-output file, the cumulative appended
    /// byte count is published, so a test synchronizes on "this fragment
    /// is observable through the advertised path" without polling.
    #[cfg(test)]
    background_appends: tokio::sync::watch::Sender<u64>,
    /// The foreground spill-transition observation seam: signaled the
    /// moment the lazy result spill is allocated, so a test synchronizes
    /// on the exact overflow boundary without polling the filesystem.
    #[cfg(test)]
    spill_started: tokio::sync::watch::Sender<bool>,
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
            background_appends: tokio::sync::watch::channel(0).0,
            spill_started: tokio::sync::watch::channel(false).0,
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
    #[cfg(target_os = "linux")]
    pub(crate) fn pause_at_shell_exit(mut self) -> Self {
        self.runner.pause_at_shell_exit = true;
        self
    }

    /// The lifecycle hook; tests subscribe to the exact shell-exit
    /// boundary and release the parked runner through it.
    #[must_use]
    #[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
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

    /// Subscribes to the background output-append observation seam: every
    /// published value is the cumulative byte count provably appended to
    /// the live-output file (the append linearization point), so a test
    /// can Read the advertised path while the execution is still running
    /// without a timing assumption.
    #[must_use]
    pub(crate) fn background_append_watcher(&self) -> tokio::sync::watch::Receiver<u64> {
        self.background_appends.subscribe()
    }

    /// Subscribes to the foreground spill-transition observation seam: the
    /// published `true` proves the lazy result spill was allocated (the
    /// exact overflow boundary), so a test can act on the transition
    /// without a timing assumption.
    #[must_use]
    pub(crate) fn spill_started_watcher(&self) -> tokio::sync::watch::Receiver<bool> {
        self.spill_started.subscribe()
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
    // Background executions own a live-output file from the dispatch
    // commit point on (Issue #86): the file was allocated and advertised
    // BEFORE this executor began, so every terminal result of this
    // invocation — including failures before any process exists — must
    // structurally account for that runtime-owned output lifecycle. The
    // lifecycle is therefore established first: the sink is opened before
    // input parsing, platform checks, or spawning, and every early
    // failure below settles through the typed continuation.
    let background = matches!(invocation.mode, ToolInvocationMode::Background);
    let background_output = if background {
        let Some(execution_id) = context.execution_id else {
            // Without an execution identity no dispatch ever committed
            // and no live-output locator was allocated or advertised:
            // this is the one background path where no BackgroundOutput
            // lifecycle exists, so `managed_output: None` is truthful.
            return failed_result(
                "a background bash invocation requires the runtime execution identity of its \
                 dispatch; background dispatch goes through the conversation background registry",
            );
        };
        let locator = context.tool_output.background_output_path(execution_id);
        match context
            .tool_output
            .open_background_output_sink(execution_id)
        {
            Ok(sink) => Some(EstablishedBackgroundOutput { locator, sink }),
            Err(error) => {
                // The advertised live-output sink cannot even be opened:
                // output storage is unreliable from the start. The
                // already-advertised locator is retained structurally as
                // explicitly PARTIAL — never complete, never forgotten,
                // never hidden inside an error string.
                return failed_with_continuation(
                    format!(
                        "the background output file {} cannot be opened for appending \
                         ({error}); the command was not started",
                        locator.display()
                    ),
                    ManagedOutputContinuation::Partial {
                        locator,
                        diagnostic: format!(
                            "the advertised live-output sink cannot be opened: {error}"
                        ),
                    },
                );
            }
        }
    } else {
        None
    };
    let input = match BashInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => {
            return match background_output {
                // The command never started: no subprocess textual output
                // could exist, the advertised file exists and is empty,
                // and the open sink proves output storage healthy — the
                // empty live output is a COMPLETE observation of the
                // execution's textual output while the execution itself
                // Failed. Status and output completeness are independent
                // axes.
                Some(established) => failed_with_complete_empty_output(error, established),
                None => failed_result(error),
            };
        }
    };
    let command = input.command.as_str();
    let explicit_timeout = input.explicit_timeout();
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
        let _ = timeout;
        return match background_output {
            Some(established) => failed_with_complete_empty_output(
                "bash requires a Unix platform with /bin/bash".to_owned(),
                established,
            ),
            None => failed_result("bash requires a Unix platform with /bin/bash"),
        };
    }
    #[cfg(unix)]
    run_bash_unix(
        command,
        timeout,
        invocation.mode,
        context,
        control,
        background_output,
    )
    .await
}

/// The established background live-output lifecycle of one invocation
/// (Issue #86): the dispatch-allocated, already-advertised absolute
/// locator plus its healthy open append sink. Once this exists, every
/// terminal result of the invocation accounts for the locator
/// structurally.
struct EstablishedBackgroundOutput {
    /// The dispatch-allocated, already-advertised absolute locator.
    locator: std::path::PathBuf,
    /// The healthy open append sink: proof that output storage works.
    #[cfg_attr(not(unix), allow(dead_code))] // only the Unix pipeline appends
    sink: crate::tools::managed_output::BackgroundOutput,
}

/// A lifecycle-aware failed Bash result: `continuation` is the typed
/// managed-output truth the caller derived from the facts of the
/// failure. Execution status and output completeness are independent
/// axes.
fn failed_with_continuation(
    error: String,
    continuation: ManagedOutputContinuation,
) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Failed { error },
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: Some(continuation),
    }
}

/// The `Failed + Complete` early-failure shape: the command never started
/// (input/setup/platform/spawn failure before any subprocess existed), the
/// advertised live-output file exists and is empty, and output storage is
/// healthy (the sink is open), so the empty file is a COMPLETE observation
/// of the execution's textual output.
fn failed_with_complete_empty_output(
    error: String,
    established: EstablishedBackgroundOutput,
) -> ToolExecutionResult {
    failed_with_continuation(
        error,
        ManagedOutputContinuation::Complete {
            locator: established.locator,
        },
    )
}

/// The Unix-only half of [`run_bash`]: spawns the invocation supervisor
/// through the shared supervised command runner, supervises the capture,
/// and settles the invocation.
#[cfg(unix)]
#[allow(clippy::too_many_lines)] // one coherent spawn/capture/settle pipeline
async fn run_bash_unix(
    command: &str,
    timeout: Option<Duration>,
    mode: ToolInvocationMode,
    context: &ToolExecutionContext<'_>,
    control: Option<&BashTestControl>,
    background_output: Option<EstablishedBackgroundOutput>,
) -> ToolExecutionResult {
    #[cfg(not(test))]
    let _ = control;
    let background = matches!(mode, ToolInvocationMode::Background);
    // The background live-output lifecycle was established by the caller
    // before any failure path (Issue #86): every decoded output fragment
    // is appended to the dispatch-allocated file from the first byte on,
    // so the model can Read/Grep the output while the execution runs.
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
        cancellation: context.cancellation.signal(),
    };
    let (mut runner, stdout_pipe, stderr_pipe) =
        match SupervisedCommandRunner::spawn(&spec, runner_control) {
            Ok(parts) => parts,
            Err(error) => {
                return match background_output {
                    // Every spawn error is raised before the supervisor
                    // process exists, so no subprocess textual output
                    // could have been produced: with the sink already
                    // open (healthy storage), the advertised empty file
                    // is the complete observation of textual output.
                    Some(established) => failed_with_complete_empty_output(
                        supervisor_spawn_failure(&error),
                        established,
                    ),
                    None => failed_result(supervisor_spawn_failure(&error)),
                };
            }
        };

    let stdout_capture = Arc::new(Mutex::new(PreviewCapture::new(BASH_STREAM_PREVIEW_BYTES)));
    let stderr_capture = Arc::new(Mutex::new(PreviewCapture::new(BASH_STREAM_PREVIEW_BYTES)));
    let (combined_tx, combined_rx) = tokio::sync::mpsc::channel::<(u8, String)>(32);

    // The combined multiplex storage policy is mode-dependent: foreground
    // output spills lazily only on overflow (small output creates no
    // file); background output streams into the live-output file that the
    // dispatch allocated and advertised, from the first fragment on.
    let mut foreground_capture = None;
    let mut background_capture = None;
    let mut combined_task = if let Some(established) = background_output {
        let sink = established.sink;
        #[cfg(test)]
        let append_watch: AppendWatch = control.map(|control| control.background_appends.clone());
        #[cfg(not(test))]
        let append_watch: AppendWatch = None;
        let capture = Arc::new(Mutex::new(BackgroundOutputCapture::new(
            BASH_STREAM_PREVIEW_BYTES,
            sink,
            append_watch,
        )));
        background_capture = Some(capture.clone());
        tokio::spawn(consume_background(combined_rx, capture))
    } else {
        let spill_capture = SpillCapture::new(BASH_STREAM_PREVIEW_BYTES);
        #[cfg(test)]
        let spill_capture = match control {
            Some(control) => spill_capture.with_spill_started_watch(control.spill_started.clone()),
            None => spill_capture,
        };
        let capture = Arc::new(Mutex::new(spill_capture));
        foreground_capture = Some(capture.clone());
        tokio::spawn(consume_combined(
            combined_rx,
            context.tool_output.clone(),
            capture,
        ))
    };
    let mut stdout_task = None;
    let mut stderr_task = None;
    if let Some(pipe) = stdout_pipe {
        #[cfg(test)]
        let stdout_park: CapturePark = control
            .and_then(|control| control.capture_hold())
            .map(CaptureHold::reader);
        #[cfg(not(test))]
        let stdout_park: CapturePark = None;
        stdout_task = Some(tokio::spawn(capture_stream(
            pipe,
            stdout_capture.clone(),
            combined_tx.clone(),
            0,
            "stdout",
            stdout_park,
        )));
    }
    if let Some(pipe) = stderr_pipe {
        let stderr_park: CapturePark = None;
        stderr_task = Some(tokio::spawn(capture_stream(
            pipe,
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
    // Whether the capture settled completely: every output fragment
    // provably reached the capture. Only a complete capture may advertise
    // an output locator as the complete output; a background output file
    // whose storage failed is honestly labelled partial, never complete.
    let mut capture_error: Option<String> = None;
    // The deferred foreground capture failure (see below): the failed
    // result is returned only after the capture settled and cleaned up.
    let mut foreground_unowned_failure: Option<String> = None;
    if let Err(error) = *capture {
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
        if !outcome_owned {
            if background {
                // The output sink of an already-advertised background path
                // failed while the process itself succeeded: the execution
                // still settles as an explicit failure — a zero exit code
                // never papers over lost output storage — and the settled
                // result below names the partial file honestly.
                capture_failure = Some(format!("bash output capture failed: {error}"));
            } else {
                // Defer the failed result until the capture has settled
                // below: `finish(false)` owns the partial spill cleanup, so
                // a failed foreground capture never leaks a partial file.
                foreground_unowned_failure = Some(format!("bash output capture failed: {error}"));
            }
        }
        capture_error = Some(error);
    }

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
    let combined = match (foreground_capture, background_capture) {
        (Some(capture), None) => {
            // The consume task already settled (the drain above), so the
            // executor holds the only reference.
            Arc::try_unwrap(capture)
                .expect("the combined capture task settled")
                .into_inner()
                .expect("combined capture lock")
                .finish(capture_error.is_none())
        }
        (None, Some(capture)) => Arc::try_unwrap(capture)
            .expect("the background capture task settled")
            .into_inner()
            .expect("background capture lock")
            .finish(capture_error.is_none()),
        _ => unreachable!("exactly one combined capture exists per invocation"),
    };
    if let Some(message) = foreground_unowned_failure {
        // The capture has settled and `finish(false)` discarded the partial
        // spill best-effort: the failure is explicit and no partial file or
        // locator survives.
        return failed_result(message);
    }

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
            reason: context.cancellation.reason(),
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

    // An incomplete capture is partial data, never a complete record: it
    // always counts as truncated, and the complete byte count is unknown.
    let truncated = stdout.1 || stderr.1 || combined.truncated || !combined.complete;
    // Textual output stays textual in both modes (Issue #86): the bounded
    // previews are the canonical record and no `FileReference` is ever
    // produced for execution output. The complete-vs-partial output truth
    // is runtime-owned typed metadata (`managed_output`), never magic
    // properties of the tool-owned JSON: a foreground result references
    // its lazy spill only when the output crossed the preview bound and
    // the capture is complete; a background result always names its
    // dispatch-allocated live-output file — as the complete output when
    // the capture settled completely, or as honestly partial running
    // output when output storage failed.
    let managed_output = match (background, combined.complete, combined.output_locator) {
        (_, true, Some(path)) => Some(ManagedOutputContinuation::Complete { locator: path }),
        (true, false, Some(path)) => Some(ManagedOutputContinuation::Partial {
            locator: path,
            diagnostic: capture_error
                .clone()
                .unwrap_or_else(|| "unknown capture failure".to_owned()),
        }),
        (false, _, None) => capture_error
            .clone()
            .map(|diagnostic| ManagedOutputContinuation::Unavailable { diagnostic }),
        (false, false, Some(_)) => {
            unreachable!("a foreground capture never advertises a partial spill")
        }
        (true, _, None) => unreachable!("a background capture always owns its live-output file"),
    };
    // The model must be able to locate the managed output of a FOREGROUND
    // result from the result content itself, so Bash presents its own
    // continuation as an ordinary tool-owned text block. A background
    // result needs no such block: the accepted dispatch result already
    // advertised the live-output path, and the generic background terminal
    // publication renders the typed continuation structurally.
    let mut result_content = vec![ToolResultContent::Json {
        value: serde_json::json!({
            "exit_code": exit_code,
            "stdout": stdout.0,
            "stderr": stderr.0,
            "combined": combined.preview,
        }),
    }];
    if !background && let Some(continuation) = &managed_output {
        result_content.push(ToolResultContent::Text(
            crate::message::content::TextBlock {
                text: continuation.render(),
            },
        ));
    }
    ToolExecutionResult {
        status,
        content: result_content,
        duration_ms: 0,
        exit_code,
        artifacts: Vec::new(),
        truncation: truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: combined.complete.then_some(combined.total_bytes),
        }),
        managed_output,
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
