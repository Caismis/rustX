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
//! A Bash invocation is treated as one complete lifecycle: spawn the owned
//! process group, read stdout/stderr, wait for the shell/process group,
//! cancellation/timeout, TERM, grace, KILL if necessary, reap, complete the
//! output draining, finalize the artifacts, and produce a single canonical
//! result. Cancellation and the invocation timeout remain authoritative
//! until the full lifecycle settles: if the shell parent exits while a
//! descendant still belongs to the owned process group and keeps the output
//! pipes open, the drain phase still races cancellation/timeout and can
//! terminate the group, so a shell-parent exit can never let descendant
//! work escape the timeout/cancellation contract.
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
//! The cancellation path is
//!
//! ```text
//! cancellation/timeout wins
//! → TERM owned process group
//! → wait BASH_TERM_GRACE
//! → if still alive, KILL owned process group
//! → reap child
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
    BASH_STREAM_PREVIEW_BYTES, BASH_TERM_GRACE, DEFAULT_FOREGROUND_BASH_TIMEOUT,
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
pub struct BashTool;

impl ToolExecutor for BashTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move { run_bash(&invocation, &context).await })
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

/// The outcome of the output-drain phase.
enum DrainOutcome {
    Done(Box<Result<StreamReferences, String>>),
    Cancelled,
    TimedOut,
}

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

#[allow(clippy::too_many_lines)] // one coherent spawn/wait/drain/settle pipeline
async fn run_bash(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
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
    // or an expired deadline wins without starting new work).
    let start = tokio::time::Instant::now();
    let wait_outcome = if let Some(timeout) = timeout {
        tokio::select! {
            biased;
            () = context.cancellation.cancelled() => WaitOutcome::Cancelled,
            () = tokio::time::sleep(timeout) => WaitOutcome::TimedOut,
            status = child.wait() => match status {
                Ok(status) => WaitOutcome::Exit(status),
                Err(error) => {
                    terminate_process_group(pgid).await;
                    return failed_result(format!("cannot wait for /bin/bash: {error}"));
                }
            },
        }
    } else {
        tokio::select! {
            biased;
            () = context.cancellation.cancelled() => WaitOutcome::Cancelled,
            status = child.wait() => match status {
                Ok(status) => WaitOutcome::Exit(status),
                Err(error) => {
                    terminate_process_group(pgid).await;
                    return failed_result(format!("cannot wait for /bin/bash: {error}"));
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

    // When the wait phase was terminated by cancellation/timeout, the
    // owned process group is terminated and the shell reaped before the
    // output drain; the pipes then close and the readers settle.
    if settled.is_some() {
        terminate_process_group(pgid).await;
        let _ = child.wait().await;
    }

    // The drain phase: cancellation and the remaining invocation timeout
    // stay authoritative until the complete lifecycle settles. A shell
    // parent that exited while a descendant keeps the owned group and the
    // output pipes alive cannot escape: the drain phase terminates the
    // group and then re-drains.
    let drain_outcome = if settled.is_some() {
        DrainOutcome::Done(Box::new(
            await_drain(&mut stdout_task, &mut stderr_task, &mut combined_task).await,
        ))
    } else if let Some(timeout) = timeout {
        let remaining = timeout.saturating_sub(start.elapsed());
        tokio::select! {
            biased;
            () = context.cancellation.cancelled() => DrainOutcome::Cancelled,
            () = tokio::time::sleep(remaining) => DrainOutcome::TimedOut,
            result = await_drain(&mut stdout_task, &mut stderr_task, &mut combined_task) => {
                DrainOutcome::Done(Box::new(result))
            }
        }
    } else {
        tokio::select! {
            biased;
            () = context.cancellation.cancelled() => DrainOutcome::Cancelled,
            result = await_drain(&mut stdout_task, &mut stderr_task, &mut combined_task) => {
                DrainOutcome::Done(Box::new(result))
            }
        }
    };

    let capture = match drain_outcome {
        DrainOutcome::Done(result) => *result,
        DrainOutcome::Cancelled => {
            // Cancellation remains authoritative during the drain: the
            // owned group is terminated, which closes the pipes and lets
            // the readers settle. The shell parent already exited and was
            // reaped in the wait phase.
            settled = Some(Settled::Cancelled);
            terminate_process_group(pgid).await;
            await_drain(&mut stdout_task, &mut stderr_task, &mut combined_task).await
        }
        DrainOutcome::TimedOut => {
            settled = Some(Settled::TimedOut);
            terminate_process_group(pgid).await;
            await_drain(&mut stdout_task, &mut stderr_task, &mut combined_task).await
        }
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

/// Terminates the owned process group: TERM, then after
/// [`BASH_TERM_GRACE`] a KILL. A `None` group id (non-Unix platforms,
/// where no group exists) is a no-op.
async fn terminate_process_group(pgid: Option<i32>) {
    if let Some(pgid) = pgid {
        signal_process_group(pgid, true);
        tokio::time::sleep(BASH_TERM_GRACE).await;
        signal_process_group(pgid, false);
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

/// Signals the owned process group. On Unix this is a safe `killpg` over
/// the group id; the group owner is the child itself, so unrelated runtime
/// processes are never signaled.
fn signal_process_group(pgid: i32, term: bool) {
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;
        let signal = if term {
            Signal::SIGTERM
        } else {
            Signal::SIGKILL
        };
        let _ = killpg(Pid::from_raw(pgid), signal);
    }
    #[cfg(not(unix))]
    {
        let _ = (pgid, term);
    }
}

#[cfg(test)]
mod tests {
    use super::{NAME, owned_pgid, run_bash};
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::{ProgressReporter, ToolExecutionContext};
    use crate::tools::types::{
        ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolProgress,
    };
    use crate::tools::workspace::Workspace;

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

    async fn run_with(
        command: &str,
        artifacts: &ArtifactStore,
        workspace: &Workspace,
    ) -> ToolExecutionResult {
        let reporter = NoopProgress;
        let context = ToolExecutionContext {
            conversation_id: &ConversationId::new("conv-1"),
            execution_id: None,
            cancellation: CancellationSignal::new(),
            workspace,
            progress: &reporter,
            artifacts,
            environment: &ToolEnvironment::new(),
        };
        run_bash(&invocation(command), &context).await
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
}
