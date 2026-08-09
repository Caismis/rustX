//! Native Bash tool (M5).
//!
//! Executes one non-interactive `/bin/bash -c <command>` invocation per
//! call. No persistent shell exists and no shell state survives between
//! calls. The current working directory is always the explicit workspace
//! root, and the child environment is explicit: `env_clear()` followed by
//! the runtime-approved basics plus explicitly authorized entries, so
//! parent-process secrets are absent unless explicitly authorized.
//!
//! # Process ownership
//!
//! On Unix every invocation owns a distinct process group (the child is its
//! own group leader via the safe [`process_group`] API), and cancellation
//! or timeout signals the owned process group — never just the immediate
//! shell process — so descendants are terminated without signaling
//! unrelated runtime processes. The cancellation path is
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
use crate::tools::native::native_definition;
use crate::tools::native::support::failed_result;
use crate::tools::types::{
    ToolDefinition, ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode,
    ToolResultContent, TruncationState,
};

/// The canonical model-facing name of the tool.
pub const NAME: &str = "bash";

/// The canonical business schema of the tool.
#[must_use]
pub fn definition() -> ToolDefinition {
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
        let mut out = Vec::with_capacity(self.head.len() + marker.len() + self.tail.len());
        out.extend_from_slice(&self.head);
        out.extend_from_slice(marker.as_bytes());
        out.extend_from_slice(&self.tail);
        (String::from_utf8_lossy(&out).into_owned(), true)
    }
}

#[allow(clippy::too_many_lines)] // one coherent spawn/wait/capture pipeline
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
    #[cfg(unix)]
    let pgid = i32::try_from(child.id().unwrap_or(0)).unwrap_or(0);
    #[cfg(not(unix))]
    let pgid = 0;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_capture = Arc::new(Mutex::new(PreviewCapture::new(BASH_STREAM_PREVIEW_BYTES)));
    let stderr_capture = Arc::new(Mutex::new(PreviewCapture::new(BASH_STREAM_PREVIEW_BYTES)));
    let combined_capture = Arc::new(Mutex::new(PreviewCapture::new(BASH_STREAM_PREVIEW_BYTES)));
    let (combined_tx, combined_rx) = tokio::sync::mpsc::channel::<(u8, Vec<u8>)>(32);

    let store = context.artifacts.clone();
    let combined_task = tokio::spawn(consume_combined(
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

    let wait_result = if let Some(timeout) = timeout {
        tokio::select! {
            biased;
            () = context.cancellation.cancelled() => Ok(WaitOutcome::Cancelled),
            () = tokio::time::sleep(timeout) => Ok(WaitOutcome::TimedOut),
            status = child.wait() => status.map(WaitOutcome::Exit).map_err(|error| {
                format!("cannot wait for /bin/bash: {error}")
            }),
        }
    } else {
        tokio::select! {
            biased;
            () = context.cancellation.cancelled() => Ok(WaitOutcome::Cancelled),
            status = child.wait() => status.map(WaitOutcome::Exit).map_err(|error| {
                format!("cannot wait for /bin/bash: {error}")
            }),
        }
    };

    // Termination path: TERM the owned process group, wait the configured
    // grace period, KILL the group when it is still alive, and reap the
    // child — exactly once. The wait phase and this termination path are
    // mutually exclusive, so the execution settles exactly once.
    let (exit_status, settled_by_cancellation, timed_out, wait_error) = match wait_result {
        Ok(WaitOutcome::Exit(status)) => (Some(status), false, false, None),
        Ok(WaitOutcome::Cancelled) => {
            terminate_process_group(pgid).await;
            let _ = child.wait().await;
            (None, true, false, None)
        }
        Ok(WaitOutcome::TimedOut) => {
            terminate_process_group(pgid).await;
            let _ = child.wait().await;
            (None, false, true, None)
        }
        Err(error) => {
            terminate_process_group(pgid).await;
            let _ = child.wait().await;
            (None, false, false, Some(error))
        }
    };

    let stdout_reference = await_stream(stdout_task).await;
    let stderr_reference = await_stream(stderr_task).await;
    let combined_reference = combined_task.await.ok().flatten();

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

    if let Some(error) = wait_error {
        return failed_result(error);
    }
    let mut status = ToolExecutionStatus::Success;
    let mut exit_code = None;
    if let Some(exit) = exit_status {
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
    } else if timed_out {
        status = ToolExecutionStatus::TimedOut;
    } else if settled_by_cancellation {
        status = ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested,
        };
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
async fn spool_stream<R>(
    mut pipe: R,
    store: crate::tools::artifacts::ArtifactStore,
    capture: Arc<Mutex<PreviewCapture>>,
    combined_tx: tokio::sync::mpsc::Sender<(u8, Vec<u8>)>,
    stream_id: u8,
    name: &'static str,
) -> Option<FileReference>
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
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let chunk = buffer[..read].to_vec();
        capture.lock().expect("preview lock").push(&chunk);
        let _ = combined_tx.send((stream_id, chunk.clone())).await;
        let writer = if let Some(handle) = &mut artifact {
            handle
        } else {
            let Ok(id) = store.create_artifact() else {
                continue;
            };
            let Ok(writer) = store.open_writer(&id) else {
                continue;
            };
            artifact = Some((id, writer));
            artifact.as_mut().expect("inserted above")
        };
        let _ = writer.1.write_all(&chunk);
    }
    drop(combined_tx);
    artifact.map(|(id, _)| FileReference {
        artifact_id: id,
        name: Some(format!("{name}.log")),
        mime_type: Some("application/octet-stream".to_owned()),
        description: Some(format!("full {name} output of the bash execution")),
    })
}

/// Consumes the runtime-observed combined stdout/stderr multiplex and spools
/// it as one artifact while retaining a bounded preview.
async fn consume_combined(
    mut rx: tokio::sync::mpsc::Receiver<(u8, Vec<u8>)>,
    store: crate::tools::artifacts::ArtifactStore,
    capture: Arc<Mutex<PreviewCapture>>,
) -> Option<FileReference> {
    let mut artifact: Option<(
        crate::runtime::identity::ArtifactId,
        crate::tools::artifacts::ArtifactWriter,
    )> = None;
    while let Some((_stream_id, chunk)) = rx.recv().await {
        capture.lock().expect("preview lock").push(&chunk);
        let writer = if let Some(handle) = &mut artifact {
            handle
        } else {
            let Ok(id) = store.create_artifact() else {
                continue;
            };
            let Ok(writer) = store.open_writer(&id) else {
                continue;
            };
            artifact = Some((id, writer));
            artifact.as_mut().expect("inserted above")
        };
        let _ = writer.1.write_all(&chunk);
    }
    artifact.map(|(id, _)| FileReference {
        artifact_id: id,
        name: Some("combined.log".to_owned()),
        mime_type: Some("application/octet-stream".to_owned()),
        description: Some("combined stdout/stderr output of the bash execution".to_owned()),
    })
}

async fn await_stream(
    task: Option<tokio::task::JoinHandle<Option<FileReference>>>,
) -> Option<FileReference> {
    match task {
        Some(task) => task.await.ok().flatten(),
        None => None,
    }
}

/// Terminates the owned process group: TERM, then after
/// [`BASH_TERM_GRACE`] a KILL.
async fn terminate_process_group(pgid: i32) {
    signal_process_group(pgid, true);
    tokio::time::sleep(BASH_TERM_GRACE).await;
    signal_process_group(pgid, false);
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
