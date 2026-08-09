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
//! A Bash invocation is one complete lifecycle: spawn the owned process
//! group, read stdout/stderr, wait for the shell, supervise the owned
//! process group to quiescence, handle cancellation/timeout with a
//! liveness-aware TERM -> grace -> KILL, reap, complete the output
//! draining, finalize the artifacts, and produce a single canonical
//! result.
//!
//! Shell-parent exit is **not** by itself the Bash settlement boundary:
//! the shell may exit while a descendant still belongs to the
//! invocation-owned process group, with the output pipes either still held
//! or already redirected away. The invocation therefore settles naturally
//! only when both the runtime-owned output capture is settled **and** the
//! owned process group is quiescent — or when an explicit
//! cancellation/timeout/process-control failure settles the invocation.
//! Cancellation and the invocation deadline remain authoritative until the
//! complete lifecycle settles: they terminate the owned group, so a
//! shell-parent exit can never let descendant work escape the
//! timeout/cancellation contract, even when the descendant no longer holds
//! the rustX pipes.
//!
//! # Process-group safety
//!
//! On Unix every invocation owns a distinct process group (the child is its
//! own group leader via the safe [`process_group`] API), and cancellation
//! or timeout signals the owned process group — never just the immediate
//! shell process — so descendants are terminated without signaling
//! unrelated runtime processes. The group id is derived from the spawned
//! child's own pid; a failed pid lookup/conversion produces an explicit
//! tool failure (after a best-effort kill of the direct child) and never a
//! sentinel group id, so `killpg(0, ...)` — which would signal the
//! caller's own process group — is structurally unreachable.
//!
//! # Ownership anchor
//!
//! A numeric process-group id is **not** stable ownership evidence by
//! itself: once the invocation's group ceases to exist, the kernel may
//! assign the same number to an unrelated process group, and a later
//! `killpg` would target that foreign group. rustX therefore holds a real
//! ownership anchor: the shell leader is kept **unreaped** until
//! settlement. While the leader is unreaped the kernel keeps its pid — the
//! invocation's process-group id — allocated (a zombie's pid is only
//! released at its reap), so no other process group can ever receive that
//! numeric id. Signaling is legal exactly while the anchor is held; the
//! anchor is released exactly once, by the leader's final reap at
//! settlement, and after that release the numeric process-group id is never
//! referenced again. A `killpg` can therefore never reach a foreign
//! process group: while the anchor is held the number provably belongs to
//! this invocation, and after the anchor is released no further signal
//! exists.
//!
//! # Ownership states
//!
//! ```text
//! Spawned (anchor acquired: leader = group leader)
//!   ↓ child exits, observed with waitid(WNOWAIT) without reaping
//! ShellExitedButOwnershipRetained
//!   ↓ supervisor loop: drain capture + owned-descendant membership scan
//! DescendantsQuiescent            ── or ──>  Cancellation / Timeout
//!   ↓ final reap (anchor released;       ↓ TERM/KILL only while the anchor
//!   pgid never referenced again)          ↓ is held, then quiescence
//! Settled                                  ↓ confirmation, final reap
//!                                          Settled (Cancelled/TimedOut)
//! ```
//!
//! The three concepts are kept separate:
//!
//! - *ownership anchor* — the unreaped shell leader; what makes signaling
//!   safe;
//! - *live descendant membership* — inspected with a Linux `/proc` scan of
//!   processes linked to the group (the leader excluded); what must reach
//!   zero before natural settlement or settlement after termination;
//! - *final reap* — the single point that releases the anchor and makes
//!   the numeric pgid permanently untouchable.
//!
//! # Quiescence
//!
//! Descendant quiescence is **not** observed with `killpg(pgid, 0)`: while
//! the unreaped leader anchor exists it is itself a group member, so that
//! probe always reports the group as alive and cannot distinguish "only
//! the anchor remains" from "live descendants remain". Instead, on Linux,
//! rustX enumerates `/proc` and counts processes whose process-group id
//! equals the invocation's pgid, excluding the leader. That count reaching
//! zero is a stable observation: the only remaining member would be the
//! leader zombie, and a zombie cannot fork, so no new member can ever
//! appear afterwards. Only then is the leader reaped.
//!
//! # Process-control failures
//!
//! Signaling, waiting, and group-state probing never swallow their errors:
//! every [`ProcessControlError`] settles the invocation as an explicit
//! failed tool result. If rustX can no longer prove that a numeric process
//! group is invocation-owned (or an inspection fails), it never signals
//! that numeric id again and settles as an explicit failure instead —
//! safety always precedes settlement convenience. Cancellation/timeout
//! intent that cannot be established through process control is never
//! downgraded to a silent `Success`, `Cancelled`, or `TimedOut`; the failed
//! result is consistent with the background registry's rule that an
//! explicit process-control/runtime failure may override canonical
//! cancellation settlement.
//!
//! The cancellation path is
//!
//! ```text
//! cancellation/timeout wins
//! → TERM owned process group (anchor held)
//! → poll owned-descendant membership within BASH_TERM_GRACE
//! → quiescent → done
//! → else KILL owned process group (anchor held)
//! → poll owned-descendant membership within a bounded confirmation window
//! → final leader reap (anchor released; pgid never referenced again)
//! → settle exactly once
//! ```
//!
//! Bash is Unix-first by nature; the supported platform assumption is
//! explicit (`/bin/bash` semantics are never pretended to be portable).
//!
//! [`process_group`]: std::os::unix::process::CommandExt::process_group
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

use std::io::Write;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::message::content::FileReference;
use crate::runtime::types::CancellationReason;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{
    BASH_REAP_GRACE, BASH_STREAM_PREVIEW_BYTES, BASH_TERM_GRACE, DEFAULT_FOREGROUND_BASH_TIMEOUT,
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
         environment and process-group ownership.",
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
/// process-control failures, model the ownership release/reuse transition,
/// and record every process-group signal attempt without an
/// operating-system mocking framework.
#[cfg_attr(test, allow(clippy::struct_excessive_bools))] // a bounded test-seam bundle
#[derive(Clone)]
pub(crate) struct BashTestControl {
    #[cfg(test)]
    lifecycle: BashLifecycleHook,
    #[cfg(test)]
    pause_at_shell_exit: bool,
    #[cfg(test)]
    fail_signal: bool,
    #[cfg(test)]
    fail_group_probe: bool,
    #[cfg(test)]
    fail_wait: bool,
    #[cfg(test)]
    force_anchor_loss: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    recorded_signals: Arc<Mutex<Vec<RecordedSignal>>>,
}

/// One attempted process-group signal, recorded by the test seam.
///
/// `emitted` is `true` only when the signal actually reached the kernel
/// (`killpg` was invoked); a refused attempt (ownership lost, injected
/// failure) is recorded with `emitted == false`.
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
            fail_signal: false,
            fail_group_probe: false,
            fail_wait: false,
            force_anchor_loss: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            recorded_signals: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Arms the exact shell-exit boundary: the executor parks after the
    /// shell's natural exit until the test releases it.
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

    /// A shared handle that forces the ownership anchor to read as lost:
    /// the runtime then behaves as if the owned group's lifetime had ended
    /// and the numeric pgid might name a foreign group.
    #[must_use]
    pub(crate) fn force_anchor_loss_handle(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.force_anchor_loss.clone()
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

    /// Makes every group signal fail with an injected error.
    #[must_use]
    pub(crate) fn fail_signal(mut self) -> Self {
        self.fail_signal = true;
        self
    }

    /// Makes every group-state probe fail with an injected error.
    #[must_use]
    pub(crate) fn fail_group_probe(mut self) -> Self {
        self.fail_group_probe = true;
        self
    }

    /// Makes the shell wait fail with an injected error.
    #[must_use]
    pub(crate) fn fail_wait(mut self) -> Self {
        self.fail_wait = true;
        self
    }
}

/// The exact shell-exit lifecycle boundary of one invocation, observable
/// only by in-crate tests.
///
/// The executor signals the boundary exactly once — after `child.wait()`
/// returned the shell's natural exit and before any natural-settlement or
/// group-quiescence handling — and then parks until the test releases it.
/// Both sides are `tokio::sync::watch`-based, so the test can never miss
/// the boundary and the executor can never be released too early.
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
/// Signaling, waiting, and group-state probing never silently fail: a
/// failure that undermines ownership or settlement surfaces through this
/// error and settles the invocation as an explicit failed result, never as
/// an ordinary `Success`, `Cancelled`, or `TimedOut`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProcessControlError {
    /// Signaling the owned process group failed.
    Signal {
        /// The signal that could not be delivered.
        signal: &'static str,
        /// The underlying failure.
        error: String,
    },
    /// Waiting for/reaping the shell child failed.
    Wait {
        /// The underlying failure.
        error: String,
    },
    /// Probing the owned process-group state failed.
    GroupState {
        /// The underlying failure.
        error: String,
    },
    /// The ownership anchor is no longer held, so the numeric process-group
    /// id can no longer be proven to belong to this invocation; signaling
    /// it is permanently forbidden.
    OwnershipLost {
        /// The numeric process-group id whose ownership can no longer be
        /// proven.
        pgid: i32,
    },
    /// The owned process group could not be confirmed quiescent within the
    /// bounded post-KILL confirmation window.
    QuiescenceTimeout,
}

impl core::fmt::Display for ProcessControlError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Signal { signal, error } => {
                write!(
                    f,
                    "cannot send {signal} to the owned process group: {error}"
                )
            }
            Self::Wait { error } => write!(f, "cannot wait for the bash child: {error}"),
            Self::GroupState { error } => {
                write!(f, "cannot verify the owned process group: {error}")
            }
            Self::OwnershipLost { pgid } => write!(
                f,
                "cannot prove the process group {pgid} is still owned by this bash \
                 invocation; signaling it is forbidden"
            ),
            Self::QuiescenceTimeout => {
                write!(f, "the owned process group did not become quiescent")
            }
        }
    }
}

impl std::error::Error for ProcessControlError {}

/// The ownership anchor of one Bash invocation: the unreaped shell leader.
///
/// The shell is spawned as its own process-group leader, so the invocation's
/// process-group id equals the leader's pid. While the leader is unreaped,
/// the kernel keeps that pid allocated (a zombie's pid is released only at
/// its reap), so **no other process group can ever receive the invocation's
/// numeric pgid**. Signaling is legal exactly while the anchor is held; the
/// anchor is released exactly once, by the leader's final reap at
/// settlement, and after that release the numeric process-group id is never
/// referenced again.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BashOwnership {
    held: bool,
}

impl BashOwnership {
    /// Acquires the anchor: the leader is spawned but not yet reaped.
    fn acquire() -> Self {
        Self { held: true }
    }

    /// Whether the anchor is still held (signaling is provably safe).
    #[must_use]
    fn held(self) -> bool {
        self.held
    }

    /// Releases the anchor (the leader's final reap). After this point no
    /// process-group signal may ever be issued again.
    fn release(&mut self) {
        self.held = false;
    }
}

/// The outcome of the child-process wait phase.
enum WaitOutcome {
    Exit(std::process::ExitStatus),
    Cancelled,
    TimedOut,
}

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

#[allow(clippy::too_many_lines)] // one coherent spawn/wait/settle/settle pipeline
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

    let mut command_builder = Command::new("/bin/bash");
    command_builder.arg("-c").arg(command);
    command_builder.current_dir(context.workspace.root());
    command_builder.env_clear();
    for (key, value) in context
        .environment
        .child_environment(context.workspace.root())
    {
        command_builder.env(key, value);
    }
    command_builder.stdin(Stdio::null());
    command_builder.stdout(Stdio::piped());
    command_builder.stderr(Stdio::piped());
    #[cfg(unix)]
    {
        // The child becomes its own process-group leader.
        command_builder.process_group(0);
    }

    let mut child = match command_builder.spawn() {
        Ok(child) => child,
        Err(error) => return failed_result(format!("cannot spawn /bin/bash: {error}")),
    };
    // Process-group identity is derived from the spawned child's own pid.
    // A failed lookup/conversion must never reach a sentinel group id:
    // the invocation fails explicitly after a best-effort kill of the
    // direct child, and the caller's own process group can never be
    // signaled.
    #[cfg(unix)]
    let pgid = if let Some(pgid) = owned_pgid(child.id()) {
        Some(pgid)
    } else {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return failed_result(
            "cannot determine the owned process group of the bash child; \
             the invocation is aborted",
        );
    };
    #[cfg(not(unix))]
    let pgid = None;
    // The ownership anchor: the unreaped shell leader. Its pid is the
    // invocation's process-group id; while it is unreaped the numeric pgid
    // cannot be reallocated, so every signal provably targets this
    // invocation. It is released exactly once, by the leader's final reap.
    let leader_pid = pgid.unwrap_or(0);
    let mut ownership = BashOwnership::acquire();
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

    // The wait phase: the shell's own exit raced against cancellation and
    // the invocation timeout (biased: an already-observable cancellation
    // or an expired deadline wins without starting new work). The exit is
    // observed with `waitid(WNOWAIT)` — without reaping — so the ownership
    // anchor stays valid after the shell exits.
    let start = tokio::time::Instant::now();
    let mut shell_exit_observer = Box::pin(observe_shell_exit(leader_pid, control));
    let wait_outcome = if let Some(timeout) = timeout {
        tokio::select! {
            biased;
            () = context.cancellation.cancelled() => WaitOutcome::Cancelled,
            () = tokio::time::sleep(timeout) => WaitOutcome::TimedOut,
            status = &mut shell_exit_observer => match status {
                Ok(status) => WaitOutcome::Exit(status),
                Err(error) => {
                    let mut message = format!("cannot wait for /bin/bash: {error}");
                    if let Err(termination) = terminate_owned_group(
                        pgid,
                        leader_pid,
                        &mut child,
                        &mut ownership,
                        control,
                    )
                    .await
                    {
                        use std::fmt::Write as _;
                        let _ = write!(
                            message,
                            "; additionally, terminating the owned process group failed: \
                             {termination}"
                        );
                    }
                    cleanup_reap(&mut child, &mut ownership);
                    return failed_result(message);
                }
            },
        }
    } else {
        tokio::select! {
            biased;
            () = context.cancellation.cancelled() => WaitOutcome::Cancelled,
            status = &mut shell_exit_observer => match status {
                Ok(status) => WaitOutcome::Exit(status),
                Err(error) => {
                    let mut message = format!("cannot wait for /bin/bash: {error}");
                    if let Err(termination) = terminate_owned_group(
                        pgid,
                        leader_pid,
                        &mut child,
                        &mut ownership,
                        control,
                    )
                    .await
                    {
                        use std::fmt::Write as _;
                        let _ = write!(
                            message,
                            "; additionally, terminating the owned process group failed: \
                             {termination}"
                        );
                    }
                    cleanup_reap(&mut child, &mut ownership);
                    return failed_result(message);
                }
            },
        }
    };

    let mut exit_status = None;
    let mut settled = None;
    match wait_outcome {
        WaitOutcome::Exit(status) => exit_status = Some(status),
        WaitOutcome::Cancelled => settled = Some(Settled::Cancelled),
        WaitOutcome::TimedOut => settled = Some(Settled::TimedOut),
    }

    // The exact shell-exit boundary of a natural exit, observed only by
    // in-crate tests: when armed, the executor signals that `child.wait()`
    // returned the shell's natural exit and parks before any
    // natural-settlement or group-quiescence handling begins.
    #[cfg(test)]
    if let Some(control) = control {
        if control.pause_at_shell_exit && exit_status.is_some() {
            control.lifecycle.pause_after_shell_exit().await;
        }
    }

    // The natural-exit settlement supervisor: shell-parent exit is not by
    // itself the Bash settlement boundary. The invocation settles only when
    // the owned process group is quiescent (no process is linked to it) AND
    // the output capture is settled — unless cancellation, the invocation
    // deadline, or a process-control failure owns the outcome first.
    let mut drain_result: Option<Box<Result<StreamReferences, String>>> = None;
    if settled.is_none() {
        let mut drain = Box::pin(await_drain(
            &mut stdout_task,
            &mut stderr_task,
            &mut combined_task,
        ));
        let deadline = timeout.map(|timeout| start + timeout);
        loop {
            if drain_result.is_none() {
                tokio::select! {
                    biased;
                    () = context.cancellation.cancelled() => {
                        settled = Some(Settled::Cancelled);
                        break;
                    }
                    () = async {
                        match deadline {
                            Some(deadline) => tokio::time::sleep_until(deadline).await,
                            None => std::future::pending().await,
                        }
                    } => {
                        settled = Some(Settled::TimedOut);
                        break;
                    }
                    result = &mut drain => {
                        drain_result = Some(Box::new(result));
                    }
                    () = tokio::time::sleep(GROUP_POLL_INTERVAL) => {}
                }
            } else {
                // The capture is settled; only group quiescence,
                // cancellation, and the invocation deadline remain.
                tokio::select! {
                    biased;
                    () = context.cancellation.cancelled() => {
                        settled = Some(Settled::Cancelled);
                        break;
                    }
                    () = async {
                        match deadline {
                            Some(deadline) => tokio::time::sleep_until(deadline).await,
                            None => std::future::pending().await,
                        }
                    } => {
                        settled = Some(Settled::TimedOut);
                        break;
                    }
                    () = tokio::time::sleep(GROUP_POLL_INTERVAL) => {}
                }
            }
            if drain_result.is_some() {
                // Owned-descendant quiescence: no process is linked to the
                // invocation's group except the (unreaped) leader anchor.
                // While the anchor is held the numeric pgid cannot be
                // reallocated, so the scan can never observe a foreign
                // group.
                match owned_descendant_members(leader_pid, pgid.unwrap_or(0), control) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) => {
                        cleanup_reap(&mut child, &mut ownership);
                        return failed_result(format!(
                            "cannot verify the owned bash process group: {error}"
                        ));
                    }
                }
            }
        }
    }

    // When cancellation/timeout owns the outcome, the owned process group is
    // terminated (TERM -> grace -> KILL, each signal issued only while the
    // ownership anchor is held), descendant quiescence is confirmed, and the
    // leader is reaped — releasing the anchor; the numeric process-group id
    // is never referenced again. On the natural path the supervisor loop
    // already observed descendant quiescence; only the final reap remains.
    // A process-control failure here is an explicit failed result — never a
    // silent Success/Cancelled/TimedOut.
    if settled.is_some() {
        match terminate_owned_group(pgid, leader_pid, &mut child, &mut ownership, control).await {
            Ok(()) => {}
            Err(error) => {
                cleanup_reap(&mut child, &mut ownership);
                return failed_result(format!(
                    "cannot terminate the owned bash process group: {error}"
                ));
            }
        }
    } else if let Err(error) = reap_leader(&mut child, &mut ownership, control).await {
        return failed_result(format!("cannot reap /bin/bash: {error}"));
    }

    let capture = if let Some(result) = drain_result {
        // The capture already completed while the settle supervisor loop
        // was running; its result is reused — a completed JoinHandle must
        // never be polled again.
        *result
    } else {
        // The capture never completed (the pipes stayed open), so the
        // terminated group's pipes are re-drained exactly once.
        await_drain(&mut stdout_task, &mut stderr_task, &mut combined_task).await
    };

    let (stdout_reference, stderr_reference, combined_reference) = match capture {
        Ok(references) => references,
        Err(error) => {
            if settled.is_some() {
                // The cancellation/timeout owns the outcome; the capture of
                // a terminated process group is inherently partial and is
                // never reported as successful retention.
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

    let mut status = ToolExecutionStatus::Success;
    let mut exit_code = None;
    if let Some(settled) = settled {
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
/// drain future, so a terminated group can be re-drained exactly once more.
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

/// The owned process-group id of a spawned child.
///
/// Returns `None` when the child's pid cannot be determined or cannot be
/// represented as a group id; a sentinel group id (0) is never produced, so
/// the caller's own process group can never be signaled by accident.
#[cfg(unix)]
fn owned_pgid(pid: Option<u32>) -> Option<i32> {
    pid.and_then(|pid| i32::try_from(pid).ok())
}

/// The internal poll cadence of liveness-aware termination and natural
/// settlement supervision. An implementation detail of the grace period and
/// the settlement checks — never a test synchronization mechanism.
const GROUP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// Counts the invocation's owned descendants still linked to its process
/// group, excluding the leader anchor itself.
///
/// On Linux this enumerates `/proc/*/stat` and matches the process-group id
/// field (field 5) against the invocation's pgid. The leader is excluded by
/// pid: it may legitimately remain as an unreaped zombie anchor. Every
/// other match — live or zombie — is an owned descendant; counting zombies
/// conservatively never claims quiescence before init has reaped them. A
/// count of zero is stable: the only possible remaining member is the
/// leader anchor, and a zombie cannot fork, so no new member can ever
/// appear afterwards.
///
/// The scan is only meaningful while the ownership anchor is held: with the
/// leader unreaped the numeric pgid cannot be reallocated, so every process
/// the scan finds linked to the group is provably a descendant of this
/// invocation — never a foreign group.
///
/// Any inspection failure is an explicit [`ProcessControlError`]; under a
/// restricted `/proc` mount rustX fails the invocation explicitly rather
/// than risking a premature quiescence claim.
#[cfg(target_os = "linux")]
fn owned_descendant_members(
    leader_pid: i32,
    pgid: i32,
    control: Option<&BashTestControl>,
) -> Result<usize, ProcessControlError> {
    #[cfg(test)]
    if let Some(control) = control {
        if control.fail_group_probe {
            return Err(ProcessControlError::GroupState {
                error: "injected group-state probe failure".to_owned(),
            });
        }
    }
    #[cfg(not(test))]
    let _ = control;
    let entries = std::fs::read_dir("/proc").map_err(|error| ProcessControlError::GroupState {
        error: format!("cannot enumerate /proc: {error}"),
    })?;
    let mut members = 0usize;
    for entry in entries {
        // An entry that vanishes mid-scan is a process that exited; it can
        // no longer be an owned member.
        let Ok(entry) = entry else {
            continue;
        };
        let Some(entry_pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        if entry_pid == leader_pid || entry_pid <= 0 {
            continue;
        }
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(ProcessControlError::GroupState {
                    error: format!("cannot read /proc/{entry_pid}/stat: {error}"),
                });
            }
        };
        // `/proc/<pid>/stat` is `pid (comm) state ppid pgrp ...`; `comm` may
        // contain spaces and parentheses, so the fields are parsed after the
        // last `)`; the process-group id is the third field after it.
        let Some(fields) = stat.rsplit_once(')') else {
            continue;
        };
        let entry_pgrp = fields
            .1
            .split_whitespace()
            .nth(2)
            .and_then(|field| field.parse::<i32>().ok())
            // An unparseable stat is treated as an owned member: failing
            // conservatively never claims premature quiescence.
            .unwrap_or(pgid);
        if entry_pgrp == pgid {
            members += 1;
        }
    }
    Ok(members)
}

/// Non-Linux Unix has no procfs membership enumeration. The ownership
/// anchor (the unreaped leader) still makes every signal safe; descendant
/// quiescence conservatively relies on the group-existence probe. On
/// platforms where an unreaped leader zombie keeps the group alive for the
/// probe (like Linux), quiescence is never observed and the invocation
/// settles only through the deadline/cancellation paths — never with a
/// premature natural result, and never with a signal to an unproven group.
#[cfg(all(unix, not(target_os = "linux")))]
fn owned_descendant_members(
    leader_pid: i32,
    pgid: i32,
    control: Option<&BashTestControl>,
) -> Result<usize, ProcessControlError> {
    use nix::errno::Errno;
    use nix::sys::signal::killpg;
    use nix::unistd::Pid;
    #[cfg(test)]
    if let Some(control) = control {
        if control.fail_group_probe {
            return Err(ProcessControlError::GroupState {
                error: "injected group-state probe failure".to_owned(),
            });
        }
    }
    #[cfg(not(test))]
    let _ = control;
    let _ = leader_pid;
    match killpg(Pid::from_raw(pgid), None) {
        Ok(()) | Err(Errno::EPERM) => Ok(1),
        Err(Errno::ESRCH) => Ok(0),
        Err(error) => Err(ProcessControlError::GroupState {
            error: error.to_string(),
        }),
    }
}

#[cfg(not(unix))]
fn owned_descendant_members(
    _leader_pid: i32,
    _pgid: i32,
    _control: Option<&BashTestControl>,
) -> Result<usize, ProcessControlError> {
    Ok(0)
}

/// Signals the owned process group. On Unix this is a safe `killpg` over
/// the group id; the group owner is the child itself, so unrelated runtime
/// processes are never signaled.
///
/// # Signal legality
///
/// A signal is issued only while the ownership anchor is held: the leader's
/// pid — the invocation's numeric process-group id — is then still
/// allocated to this invocation, so the target cannot be a foreign process
/// group. After the anchor is released (the leader's final reap) signaling
/// is permanently forbidden and any attempt is an explicit
/// [`ProcessControlError::OwnershipLost`]. `ESRCH` from the syscall means
/// the group is already quiescent and is not an error (with the anchor held
/// this cannot occur on Linux, where the unreaped leader itself is a group
/// member). A `None` group id (non-Unix platforms, where no group exists)
/// is a no-op.
#[cfg(unix)]
fn signal_process_group(
    pgid: Option<i32>,
    signal: nix::sys::signal::Signal,
    ownership: BashOwnership,
    control: Option<&BashTestControl>,
) -> Result<(), ProcessControlError> {
    use nix::errno::Errno;
    use nix::sys::signal::killpg;
    use nix::unistd::Pid;
    let Some(pgid) = pgid else {
        return Ok(());
    };
    // INVARIANT: a signal may reach the kernel only while the ownership
    // anchor is held; afterwards the numeric pgid may name a foreign group
    // and is never referenced again.
    if !ownership.held() {
        #[cfg(test)]
        if let Some(control) = control {
            control.record_signal(RecordedSignal {
                pgid,
                signal: signal.as_str(),
                emitted: false,
            });
        }
        return Err(ProcessControlError::OwnershipLost { pgid });
    }
    #[cfg(test)]
    if let Some(control) = control {
        if control
            .force_anchor_loss
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            // The test models the owned group's lifetime having ended: the
            // same numeric pgid is now logically foreign, so the runtime
            // must refuse to signal it.
            control.record_signal(RecordedSignal {
                pgid,
                signal: signal.as_str(),
                emitted: false,
            });
            return Err(ProcessControlError::OwnershipLost { pgid });
        }
        if control.fail_signal {
            control.record_signal(RecordedSignal {
                pgid,
                signal: signal.as_str(),
                emitted: false,
            });
            return Err(ProcessControlError::Signal {
                signal: signal.as_str(),
                error: "injected signaling failure".to_owned(),
            });
        }
    }
    #[cfg(not(test))]
    let _ = control;
    let result = killpg(Pid::from_raw(pgid), Some(signal));
    #[cfg(test)]
    if let Some(control) = control {
        if matches!(result, Ok(()) | Err(Errno::ESRCH)) {
            control.record_signal(RecordedSignal {
                pgid,
                signal: signal.as_str(),
                emitted: true,
            });
        }
    }
    match result {
        // `ESRCH` means the group already quiesced: not an error.
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(ProcessControlError::Signal {
            signal: signal.as_str(),
            error: error.to_string(),
        }),
    }
}

#[cfg(not(unix))]
fn signal_process_group(
    _pgid: Option<i32>,
    _signal: nix::sys::signal::Signal,
    _ownership: BashOwnership,
    _control: Option<&BashTestControl>,
) -> Result<(), ProcessControlError> {
    Ok(())
}

/// Terminates the owned process group and completes the invocation's
/// process control:
///
/// ```text
/// TERM (anchor held => provably owned)
///   poll owned-descendant membership within BASH_TERM_GRACE
///     quiescent => done
///   KILL (anchor held)
///   poll owned-descendant membership within a bounded confirmation window
///     quiescent => done; members remain => QuiescenceTimeout
/// final leader reap (anchor released; pgid never referenced again)
/// ```
///
/// Every signal is issued only while the ownership anchor is held, so a
/// numeric pgid that was released and reused can never be signaled. The
/// leader is reaped only at the very end: while it is unreaped it is the
/// ownership anchor, and the membership scan excludes it by pid.
async fn terminate_owned_group(
    pgid: Option<i32>,
    leader_pid: i32,
    child: &mut tokio::process::Child,
    ownership: &mut BashOwnership,
    control: Option<&BashTestControl>,
) -> Result<(), ProcessControlError> {
    signal_process_group(pgid, nix::sys::signal::Signal::SIGTERM, *ownership, control)?;
    let grace_deadline = tokio::time::Instant::now() + BASH_TERM_GRACE;
    if wait_for_owned_quiescence(leader_pid, pgid, grace_deadline, control).await? {
        return reap_leader(child, ownership, control).await;
    }
    signal_process_group(pgid, nix::sys::signal::Signal::SIGKILL, *ownership, control)?;
    let confirm_deadline = tokio::time::Instant::now() + BASH_TERM_GRACE;
    if wait_for_owned_quiescence(leader_pid, pgid, confirm_deadline, control).await? {
        return reap_leader(child, ownership, control).await;
    }
    Err(ProcessControlError::QuiescenceTimeout)
}

/// Polls owned-descendant membership and the leader's own exit until the
/// invocation is quiescent or the deadline expires.
///
/// Quiescence requires **both** conditions: no owned descendant is linked
/// to the group (the membership scan excludes the leader by pid) **and**
/// the shell leader itself has exited. The leader-exit condition matters on
/// the forced path: a TERM-ignoring leader would otherwise look quiescent
/// while still running, and the final reap could never complete.
async fn wait_for_owned_quiescence(
    leader_pid: i32,
    pgid: Option<i32>,
    deadline: tokio::time::Instant,
    control: Option<&BashTestControl>,
) -> Result<bool, ProcessControlError> {
    loop {
        if owned_descendant_members(leader_pid, pgid.unwrap_or(0), control)? == 0
            && leader_exited(leader_pid)
        {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(GROUP_POLL_INTERVAL).await;
    }
}

/// Whether the shell leader has exited, observed with `waitid(WNOWAIT)` —
/// without reaping, so the ownership anchor stays valid. Inspection
/// failures conservatively report "not exited".
#[cfg(unix)]
fn leader_exited(leader_pid: i32) -> bool {
    use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
    match waitid(
        Id::Pid(nix::unistd::Pid::from_raw(leader_pid)),
        WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT | WaitPidFlag::WNOHANG,
    ) {
        Ok(WaitStatus::StillAlive) | Err(_) => false,
        Ok(_) => true,
    }
}

#[cfg(not(unix))]
fn leader_exited(_leader_pid: i32) -> bool {
    true
}

/// The final leader reap: the single ownership transition that releases the
/// anchor. Bounded by [`BASH_REAP_GRACE`]; afterwards the numeric
/// process-group id is never referenced again.
async fn reap_leader(
    child: &mut tokio::process::Child,
    ownership: &mut BashOwnership,
    control: Option<&BashTestControl>,
) -> Result<(), ProcessControlError> {
    #[cfg(test)]
    if let Some(control) = control {
        if control.fail_wait {
            return Err(ProcessControlError::Wait {
                error: "injected bash wait failure".to_owned(),
            });
        }
    }
    #[cfg(not(test))]
    let _ = control;
    let deadline = tokio::time::Instant::now() + BASH_REAP_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                ownership.release();
                return Ok(());
            }
            Ok(None) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(ProcessControlError::Wait {
                        error: "the shell leader did not exit for the final reap".to_owned(),
                    });
                }
                tokio::time::sleep(GROUP_POLL_INTERVAL).await;
            }
            Err(error) => {
                return Err(ProcessControlError::Wait {
                    error: error.to_string(),
                });
            }
        }
    }
}

/// Best-effort one-shot reap used on failure paths: reaps the leader when
/// it is already a zombie and releases the anchor, so a leaked zombie is
/// avoided whenever possible.
fn cleanup_reap(child: &mut tokio::process::Child, ownership: &mut BashOwnership) {
    if let Ok(Some(_)) = child.try_wait() {
        ownership.release();
    }
}

/// Observes the shell leader's exit **without reaping it**: `waitid` with
/// `WNOWAIT` reports the exit while the leader remains a zombie, so the
/// ownership anchor stays valid. Polled with the internal cadence; the
/// returned status is later consumed by the final reap.
async fn observe_shell_exit(
    leader_pid: i32,
    control: Option<&BashTestControl>,
) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(test)]
    if let Some(control) = control {
        if control.fail_wait {
            return Err(std::io::Error::other("injected bash wait failure"));
        }
    }
    #[cfg(not(test))]
    let _ = control;
    loop {
        use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
        match waitid(
            Id::Pid(nix::unistd::Pid::from_raw(leader_pid)),
            WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT | WaitPidFlag::WNOHANG,
        ) {
            Ok(WaitStatus::StillAlive) => {}
            Ok(status) => {
                return exit_status_of(status).ok_or_else(|| {
                    std::io::Error::other("unexpected waitid status of the shell leader")
                });
            }
            Err(error) => {
                return Err(std::io::Error::from_raw_os_error(error as i32));
            }
        }
        tokio::time::sleep(GROUP_POLL_INTERVAL).await;
    }
}

/// Converts a `WNOWAIT` exit observation into the canonical exit status.
#[cfg(unix)]
fn exit_status_of(status: nix::sys::wait::WaitStatus) -> Option<std::process::ExitStatus> {
    use nix::sys::wait::WaitStatus;
    use std::os::unix::process::ExitStatusExt;
    match status {
        WaitStatus::Exited(_, code) => Some(std::process::ExitStatus::from_raw(code << 8)),
        WaitStatus::Signaled(_, signal, _) => {
            Some(std::process::ExitStatus::from_raw(signal as i32))
        }
        _ => None,
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
    use super::{BashTestControl, BashTool, NAME, owned_pgid};
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

    /// Whether a specific process still exists, using the same
    /// non-destructive signal-0 probe as the production group liveness
    /// check.
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

    /// Polls the owned process group until it is provably gone, using the
    /// non-destructive `killpg(pgid, 0)` probe, with a strict deadlock
    /// guard. Valid only after the invocation has settled: the final reap
    /// has then removed the leader anchor, so `ESRCH` is the true group
    /// state.
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

    /// The process-group id is derived from the spawned child's own pid;
    /// a failed lookup or conversion never produces a sentinel group id.
    #[test]
    fn owned_pgid_never_produces_a_sentinel() {
        assert_eq!(owned_pgid(None), None, "an unknown pid fails explicitly");
        assert_eq!(
            owned_pgid(Some(u32::MAX)),
            None,
            "an unrepresentable pid fails explicitly"
        );
        assert_eq!(
            owned_pgid(Some(i32::MAX as u32 + 1)),
            None,
            "a pid beyond the group-id range fails explicitly"
        );
        assert_eq!(owned_pgid(Some(1234)), Some(1234));
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
    /// owned process group (`sleep 30 >/dev/null 2>&1 & exit 0`) cannot
    /// settle the invocation: the descendant no longer holds the rustX
    /// pipes, so the capture alone would finish — but the invocation stays
    /// active until the owned process group is quiescent or the timeout
    /// settles it.
    #[cfg(unix)]
    #[tokio::test]
    async fn redirected_descendant_does_not_escape_the_owned_group() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let desc_pid_file = root.join("desc.pid");
        let command = format!(
            "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
            shell_pid_file.display(),
            desc_pid_file.display()
        );
        let result = tokio::time::timeout(
            Duration::from_secs(20),
            run_with_control(
                command,
                BashTestControl::new(),
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
        let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
            .expect("shell pid file")
            .trim()
            .parse()
            .expect("shell pid");
        let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
            .expect("descendant pid file")
            .trim()
            .parse()
            .expect("descendant pid");
        // The owned process group is quiescent and the descendant is gone.
        wait_for_group_death(shell_pid).await;
        wait_for_process_death(descendant_pid).await;
        let _ = dir;
    }

    /// The exact shell-exit boundary regression: the executor provably
    /// observed the shell parent's natural exit and parked before any
    /// settlement handling; the descendant is provably alive at that
    /// boundary; only then does cancellation become observable. The result
    /// is `Cancelled` and the owned group is terminated.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_after_exact_shell_exit_boundary_terminates_the_owned_group() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let desc_pid_file = root.join("desc.pid");
        let command = format!(
            "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
            shell_pid_file.display(),
            desc_pid_file.display()
        );
        let control = BashTestControl::new().pause_at_shell_exit();
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
        //    is parked before natural settlement/group-quiescence handling.
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
        let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
            .expect("shell pid file")
            .trim()
            .parse()
            .expect("shell pid");
        wait_for_group_death(shell_pid).await;
        wait_for_process_death(descendant_pid).await;
        let _ = dir;
    }

    /// Natural settlement requires group quiescence: at the exact
    /// shell-exit boundary the invocation is provably not yet settled while
    /// the descendant is alive; once the descendant exits naturally and the
    /// owned group quiesces, the shell's natural successful exit settles
    /// the invocation as `Success`.
    #[cfg(unix)]
    #[tokio::test]
    async fn natural_success_requires_group_quiescence() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let desc_pid_file = root.join("desc.pid");
        let command = format!(
            "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; exit 0",
            shell_pid_file.display(),
            desc_pid_file.display()
        );
        let control = BashTestControl::new().pause_at_shell_exit();
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
        let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
            .expect("shell pid file")
            .trim()
            .parse()
            .expect("shell pid");
        assert!(process_alive(descendant_pid));
        // 2. The test terminates the descendant directly (test-side
        //    process control, deterministic: no timing assumption). The
        //    leader anchor stays unreaped until the executor settles, so
        //    group death is observed through the descendant's own death.
        nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(shell_pid),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("test kills the owned group");
        wait_for_process_death(descendant_pid).await;
        // 3. The executor resumes and observes the quiescent group.
        hook.release();
        let result = tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("the invocation settles exactly once")
            .expect("executor task");
        assert_eq!(
            result.status,
            ToolExecutionStatus::Success,
            "once the owned group is quiescent, the shell's natural exit settles Success"
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
    /// result: cancellation intent that cannot be established through
    /// process control is never downgraded to a silent `Cancelled`.
    #[cfg(unix)]
    #[tokio::test]
    async fn signal_failure_settles_as_an_explicit_failed_result() {
        let (_dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let desc_pid_file = root.join("desc.pid");
        // The shell records its own pid and the descendant's pid so the
        // test can clean up the group after the injected signaling failure
        // leaves it running.
        let command = format!(
            "echo $$ > {}; sleep 30 & echo $! > {}; wait",
            shell_pid_file.display(),
            desc_pid_file.display()
        );
        let cancellation = CancellationSignal::new();
        let cancelling = cancellation.clone();
        let task = tokio::spawn(run_with_control(
            command,
            BashTestControl::new().fail_signal(),
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
        // The injected failure is the very condition under test, so the
        // test itself terminates the abandoned group as cleanup. The
        // shell leader zombie stays unreaped (no signal ever reached it),
        // so the group cannot be observed as gone; the descendant's death
        // is the observable cleanup proof.
        let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
            .expect("shell pid file")
            .trim()
            .parse()
            .expect("shell pid");
        let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
            .expect("descendant pid file")
            .trim()
            .parse()
            .expect("descendant pid");
        nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(shell_pid),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("test cleanup kills the abandoned group");
        wait_for_process_death(descendant_pid).await;
    }

    /// A group-state probe failure is an explicit failed result: the
    /// runtime cannot establish settlement, so it never reports an ordinary
    /// natural outcome.
    #[cfg(unix)]
    #[tokio::test]
    async fn group_probe_failure_settles_as_an_explicit_failed_result() {
        let (_dir, artifacts, workspace) = fixture();
        let result = run_with_control(
            "exit 0".to_owned(),
            BashTestControl::new().fail_group_probe(),
            CancellationSignal::new(),
            artifacts,
            workspace,
            None,
        )
        .await;
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "an injected group-probe failure must be an explicit failed result, got {:?}",
            result.status
        );
        assert!(
            !matches!(result.status, ToolExecutionStatus::Success),
            "natural settlement requires verified group quiescence"
        );
    }

    /// A reaping/wait failure is an explicit failed result.
    #[tokio::test]
    async fn wait_failure_settles_as_an_explicit_failed_result() {
        let (_dir, artifacts, workspace) = fixture();
        let result = run_with_control(
            "echo hi".to_owned(),
            BashTestControl::new().fail_wait(),
            CancellationSignal::new(),
            artifacts,
            workspace,
            None,
        )
        .await;
        assert!(
            matches!(result.status, ToolExecutionStatus::Failed { .. }),
            "an injected wait failure must be an explicit failed result, got {:?}",
            result.status
        );
    }

    /// The PGID-reuse fail-safe regression: the ownership anchor is
    /// provably held while the owned group exists; the test then models the
    /// owned group's lifetime ending with the same numeric PGID logically
    /// foreign, and proves the runtime emits **zero** signals against that
    /// numeric PGID afterwards — it settles explicitly `Failed` instead.
    ///
    /// The transition is driven by the exact test seam, never by a
    /// probabilistic kernel PID reuse.
    #[cfg(unix)]
    #[tokio::test]
    async fn no_signals_are_issued_after_ownership_loss() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let desc_pid_file = root.join("desc.pid");
        let command = format!(
            "echo $$ > {}; sleep 30 >/dev/null 2>&1 & echo $! > {}; wait",
            shell_pid_file.display(),
            desc_pid_file.display()
        );
        let control = BashTestControl::new();
        let force_loss = control.force_anchor_loss_handle();
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
        // 1. The owned group provably exists: the shell is running.
        for _ in 0..1000 {
            if shell_pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(shell_pid_file.exists(), "the shell pid file never appeared");
        // 2. The owned group's lifetime ends: the same numeric pgid is now
        //    logically foreign.
        force_loss.store(true, Ordering::SeqCst);
        // 3. Cancellation becomes observable; the runtime must refuse to
        //    signal the (now foreign) numeric pgid and settle Failed.
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
        // 4. No signal was ever emitted after the ownership release: every
        //    recorded attempt was refused (emitted == false) and targeted
        //    the numeric pgid under question.
        let recorded = control.recorded_signals();
        assert!(
            !recorded.is_empty(),
            "a cancellation was attempted and must have reached the signal path"
        );
        let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
            .expect("shell pid file")
            .trim()
            .parse()
            .expect("shell pid");
        for attempt in &recorded {
            assert_eq!(
                attempt.pgid, shell_pid,
                "every refused attempt targets the numeric pgid under question"
            );
            assert!(
                !attempt.emitted,
                "no signal may be emitted after ownership is lost: {attempt:?}"
            );
        }
        // The group was never signaled, so the test terminates it as
        // cleanup; the descendant's death is the observable proof.
        let descendant_pid: i32 = std::fs::read_to_string(&desc_pid_file)
            .expect("descendant pid file")
            .trim()
            .parse()
            .expect("descendant pid");
        nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(shell_pid),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("test cleanup kills the abandoned group");
        wait_for_process_death(descendant_pid).await;
        let _ = dir;
    }

    /// The positive counterpart: during a real cancellation every emitted
    /// process-group signal targets exactly the invocation's own pgid (the
    /// shell leader's pid) and only occurs while the ownership anchor is
    /// held.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_signals_only_target_the_owned_group() {
        let (dir, artifacts, workspace) = fixture();
        let root = workspace.root().to_path_buf();
        let shell_pid_file = root.join("shell.pid");
        let command = format!(
            "trap '' TERM; echo $$ > {}; sleep 30",
            shell_pid_file.display()
        );
        let control = BashTestControl::new();
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
        let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
            .expect("shell pid file")
            .trim()
            .parse()
            .expect("shell pid");
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
                attempt.pgid, shell_pid,
                "every emitted signal targets the owned process-group id"
            );
        }
        wait_for_group_death(shell_pid).await;
        let _ = dir;
    }
}
