//! Server-side admission and settlement for Session-bound Agent Bash.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use rustx_execution_protocol::DecimalU64;
use rustx_execution_protocol::process::{
    BashCaptureResult, BashCaptureState, OperationId, OperationState, ProcessOperationKey,
    ProcessProgram, ProcessReply, ProcessRequest, ProcessSpec, ProcessStdin, ProcessStream,
    SettledDisposition, SettledSnapshot,
};
use sha2::{Digest as _, Sha256};

use super::BashInput;
use crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint;
use crate::runtime::cancellation::ExecutionCancellation;
use crate::runtime::identity::ConversationId;
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::native::support::failed_result;
use crate::tools::types::{
    ManagedOutputContinuation, ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus,
    ToolInvocation, TruncationState,
};
use crate::tools::workspace::Workspace;

const POLL: Duration = Duration::from_millis(20);
const EXCHANGE_BUDGET: Duration = rustx_execution_protocol::transport::PAGE_REQUEST_TIMEOUT;
const OUTPUT_ROOT: &str = "/.rustx-output/bash";

fn binding_tag(key: &ProcessOperationKey) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(key).expect("encoding process identity"))
    )
}

fn locator(key: &ProcessOperationKey) -> PathBuf {
    Path::new(OUTPUT_ROOT)
        .join(binding_tag(key))
        .join(format!("{:032x}.output", key.operation.as_u128()))
}

#[derive(Clone)]
pub(crate) struct BashOutputLocator {
    operation: OperationId,
    tag: String,
}

impl BashOutputLocator {
    pub(crate) fn key_for(
        &self,
        endpoint: &LocalExecutionEndpoint,
    ) -> io::Result<ProcessOperationKey> {
        let key = endpoint.process_key(self.operation)?;
        if self.tag != binding_tag(&key) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Bash output belongs to an earlier binding",
            ));
        }
        Ok(key)
    }
}

pub(crate) fn operation_at(path: &Path) -> Option<BashOutputLocator> {
    let relative = path.strip_prefix(OUTPUT_ROOT).ok()?.to_str()?;
    let (tag, name) = relative.split_once('/')?;
    let id = name.strip_suffix(".output")?;
    let lower_hex = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
    if tag.len() != 64
        || id.len() != 32
        || !tag.bytes().all(lower_hex)
        || !id.bytes().all(lower_hex)
    {
        return None;
    }
    Some(BashOutputLocator {
        tag: tag.to_owned(),
        operation: OperationId::from_uuid(uuid::Uuid::from_u128(
            u128::from_str_radix(id, 16).ok()?,
        )),
    })
}

pub(crate) fn discard_private_output(workspace: &Workspace, path: &Path) {
    if let Some(operation) = operation_at(path)
        && let Some(scope) = workspace.execution_scope()
        && let Ok(endpoint) = scope.endpoint()
        && let Ok(key) = operation.key_for(&endpoint)
    {
        tokio::spawn(async move {
            let _ = exchange(&endpoint, ProcessRequest::DiscardBashOutput { key }, None).await;
        });
    }
}

pub(crate) struct PreparedBash {
    operation: std::sync::Mutex<Option<RemoteBash>>,
    output: PathBuf,
}

impl PreparedBash {
    pub(crate) async fn register(
        invocation: &ToolInvocation,
        workspace: &Workspace,
        conversation: &ConversationId,
        environment: &ToolEnvironment,
        cancellation: &ExecutionCancellation,
    ) -> io::Result<Self> {
        let operation = RemoteBash::register(
            invocation,
            workspace,
            conversation,
            environment,
            cancellation,
            true,
            false,
        )
        .await?;
        Ok(Self {
            output: locator(&operation.key),
            operation: std::sync::Mutex::new(Some(operation)),
        })
    }

    pub(crate) fn set_start_gate(&self, gate: StartGate) {
        if let Some(operation) = self.operation.lock().expect("prepared Bash").as_mut() {
            operation.start_gate = Some(gate);
        }
    }

    pub(crate) fn output(&self) -> &Path {
        &self.output
    }
}

impl ToolExecutor for PreparedBash {
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }

    fn start<'a>(
        &'a self,
        _invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let operation = self.operation.lock().expect("prepared Bash").take();
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                match operation {
                    Some(operation) => operation.run(&context).await,
                    None => failed_result("the Agent Bash registration has already been consumed"),
                }
            }),
            cancellation,
        )
    }
}

pub(super) async fn run(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    match RemoteBash::register(
        invocation,
        context.workspace,
        context.conversation_id,
        context.environment,
        &context.cancellation,
        false,
        context.stdout_capture.is_some(),
    )
    .await
    {
        Ok(operation) => operation.run(context).await,
        Err(error) if error.kind() == io::ErrorKind::Interrupted => terminal(
            context
                .cancellation
                .native_status(ToolCancellationPhase::BeforeStart),
        ),
        Err(error) => failed_result(error.to_string()),
    }
}

type StartGate = Arc<dyn Fn() -> bool + Send + Sync>;

struct RemoteBash {
    endpoint: Arc<LocalExecutionEndpoint>,
    key: ProcessOperationKey,
    live_output: bool,
    cleanup: bool,
    start_gate: Option<StartGate>,
}

impl RemoteBash {
    #[allow(clippy::too_many_arguments)]
    async fn register(
        invocation: &ToolInvocation,
        workspace: &Workspace,
        conversation: &ConversationId,
        environment: &ToolEnvironment,
        cancellation: &ExecutionCancellation,
        live_output: bool,
        raw_stdout: bool,
    ) -> io::Result<Self> {
        if cancellation.is_cancelled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let input = BashInput::parse(&invocation.arguments).map_err(io::Error::other)?;
        let timeout_ms = input
            .explicit_timeout()
            .filter(|_| !live_output)
            .map_or(0, |timeout| {
                u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)
            });
        let endpoint = workspace
            .execution_scope()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotConnected,
                    "Session execution unavailable: no admitted Execution Client",
                )
            })?
            .endpoint()?;
        if workspace.root().as_os_str() != endpoint.workspace().as_os_str() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the admitted Client owns a different workspace root",
            ));
        }
        let key = endpoint.process_key(OperationId::new())?;
        let spec = ProcessSpec {
            cwd: PathBuf::new(),
            program: ProcessProgram::Shell {
                command: input.command,
                conversation: conversation.as_str().to_owned(),
                live_output,
            },
            env: environment
                .authorized_entries()
                .iter()
                .map(|(key, value)| (key.clone(), Some(value.clone())))
                .collect(),
            timeout_ms,
            pty: None,
            stdin: ProcessStdin::Closed,
            capture: raw_stdout,
            output_cap: if raw_stdout {
                u32::try_from(crate::runtime::workflow::MAX_VALUE_BYTES)
                    .expect("stdout capture bound")
            } else {
                0
            },
        };
        spec.validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let mut operation = Self {
            endpoint,
            key,
            live_output,
            cleanup: true,
            start_gate: None,
        };
        match operation
            .exchange(
                ProcessRequest::RegisterAgentBash {
                    key: operation.key.clone(),
                    fingerprint: spec.fingerprint(),
                    spec,
                },
                Some(cancellation),
            )
            .await?
        {
            ProcessReply::Registered { .. } => Ok(operation),
            ProcessReply::Refused { reason, .. } => {
                operation.cleanup = false;
                Err(io::Error::other(reason))
            }
            _ => Err(io::Error::other(
                "the Execution Client did not register Agent Bash",
            )),
        }
    }

    async fn run(mut self, context: &ToolExecutionContext<'_>) -> ToolExecutionResult {
        let cancellation = context.cancellation.clone();
        let stop = tokio_util::sync::CancellationToken::new();
        let _stop_relay_on_drop = stop.clone().drop_guard();
        let settled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let relay = tokio::spawn({
            let endpoint = self.endpoint.clone();
            let key = self.key.clone();
            let stop = stop.clone();
            let settled = settled.clone();
            let cancellation = cancellation.clone();
            async move {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => {},
                    () = cancellation.cancelled() => {
                        deliver_cancel(endpoint, key, settled).await;
                    }
                }
            }
        });
        let outcome = self.run_to_settlement(context, settled.as_ref()).await;
        stop.cancel();
        let _ = relay.await;
        match outcome {
            Ok(result) => {
                self.cleanup = false;
                result
            }
            Err(error) => terminal(ToolExecutionStatus::OutcomeUnknown {
                detail: format!(
                    "Agent Bash could not confirm settlement from its Execution Client: {error}"
                ),
            }),
        }
    }

    async fn run_to_settlement(
        &self,
        context: &ToolExecutionContext<'_>,
        settled: &std::sync::atomic::AtomicBool,
    ) -> io::Result<ToolExecutionResult> {
        let mut before_start = false;
        let started = self
            .exchange(
                ProcessRequest::Start {
                    key: self.key.clone(),
                },
                Some(&context.cancellation),
            )
            .await;
        let mut state = match started {
            Ok(ProcessReply::State { state, .. }) => state,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                before_start = true;
                OperationState::Registered
            }
            Ok(_) => {
                return Err(io::Error::other(
                    "the Client could not identify the registered operation",
                ));
            }
            Err(error) => return Err(error),
        };
        let snapshot = loop {
            if let OperationState::Settled(snapshot) = state {
                break snapshot;
            }
            tokio::time::sleep(POLL).await;
            state = match self
                .exchange(
                    ProcessRequest::Query {
                        key: self.key.clone(),
                    },
                    None,
                )
                .await?
            {
                ProcessReply::State { state, .. } => state,
                _ => {
                    return Err(io::Error::other(
                        "the Client could not account for the operation",
                    ));
                }
            };
        };
        settled.store(true, std::sync::atomic::Ordering::Release);
        let mut capture = match self
            .exchange(
                ProcessRequest::BashCapture {
                    key: self.key.clone(),
                },
                None,
            )
            .await
        {
            Ok(ProcessReply::BashCapture { capture, .. }) => capture,
            Ok(ProcessReply::Unknown { .. })
                if snapshot.disposition == SettledDisposition::Cancelled
                    && snapshot.stdout_total.get() == 0
                    && snapshot.stderr_total.get() == 0 =>
            {
                None
            }
            _ => Some(unavailable_capture()),
        };
        if let Some(stdout) = &context.stdout_capture
            && self.receive_raw_stdout(stdout, &snapshot).await.is_err()
        {
            self.endpoint.cut_pair();
        }
        if !self.release(&snapshot).await {
            // Release does not rewrite the known business outcome. Retiring
            // the pair does make the continuation unavailable.
            self.endpoint.cut_pair();
            if let Some(projection) = capture.as_mut() {
                if projection.output != BashCaptureState::None {
                    projection.output = BashCaptureState::Unavailable;
                    projection.total = None;
                    projection.truncated = true;
                }
            } else if self.live_output {
                capture = Some(unavailable_capture());
            }
        }
        Ok(project(
            snapshot,
            capture,
            before_start,
            self.live_output,
            &self.key,
            &context.cancellation,
        ))
    }

    async fn receive_raw_stdout(
        &self,
        capture: &crate::tools::output::BoundedStdoutCapture,
        snapshot: &SettledSnapshot,
    ) -> io::Result<()> {
        if snapshot.disposition == SettledDisposition::Known
            && capture.finish_if_overflowed(snapshot.stdout_total.get())
        {
            return Ok(());
        }
        let mut cursor = 0;
        let mut stdout = 0;
        while cursor < snapshot.final_sequence.get() {
            match self
                .exchange(
                    ProcessRequest::OutputPage {
                        key: self.key.clone(),
                        cursor: DecimalU64::new(cursor),
                        max_bytes: 8192,
                    },
                    None,
                )
                .await?
            {
                ProcessReply::Output {
                    stream,
                    offset,
                    delta_base64,
                    sequence,
                    ..
                } => {
                    if stream == ProcessStream::Stdout {
                        if offset.get() != stdout {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "discontinuous Bash stdout",
                            ));
                        }
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(delta_base64)
                            .map_err(io::Error::other)?;
                        capture.push(&bytes);
                        stdout += bytes.len() as u64;
                    }
                    cursor = sequence.get();
                }
                _ => return Err(io::Error::other("Bash stdout could not be read")),
            }
        }
        if cursor == snapshot.final_sequence.get()
            && stdout == snapshot.stdout_total.get()
            && !snapshot.stdout_truncated
            && snapshot.disposition == SettledDisposition::Known
        {
            capture.finish();
        }
        Ok(())
    }

    async fn release(&self, snapshot: &SettledSnapshot) -> bool {
        matches!(
            self.exchange(
                ProcessRequest::ReleaseOutput {
                    key: self.key.clone(),
                    final_sequence: snapshot.final_sequence,
                },
                None
            )
            .await,
            Ok(ProcessReply::OutputReleased { .. })
        )
    }

    async fn exchange(
        &self,
        request: ProcessRequest,
        cancellation: Option<&ExecutionCancellation>,
    ) -> io::Result<ProcessReply> {
        let gate = matches!(request, ProcessRequest::Start { .. })
            .then(|| self.start_gate.clone())
            .flatten();
        exchange_with_gate(&self.endpoint, request, cancellation, gate).await
    }
}

impl Drop for RemoteBash {
    fn drop(&mut self) {
        if !self.cleanup {
            return;
        }
        // Before ownership commits, dropping the gated executor rolls back
        // the registration. If a driver was abandoned, the same cleanup also
        // fences a late START and waits for Client-owned shutdown on failure.
        let endpoint = self.endpoint.clone();
        let key = self.key.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                deliver_cancel(endpoint.clone(), key.clone(), Arc::default()).await;
                if let Ok(ProcessReply::State {
                    state: OperationState::Settled(snapshot),
                    ..
                }) = exchange(&endpoint, ProcessRequest::Query { key: key.clone() }, None).await
                {
                    let _ = exchange(
                        &endpoint,
                        ProcessRequest::ReleaseOutput {
                            key: key.clone(),
                            final_sequence: snapshot.final_sequence,
                        },
                        None,
                    )
                    .await;
                    let _ =
                        exchange(&endpoint, ProcessRequest::DiscardBashOutput { key }, None).await;
                } else {
                    endpoint.cut_pair();
                }
            });
        } else {
            endpoint.cut_pair();
        }
    }
}

async fn exchange(
    endpoint: &Arc<LocalExecutionEndpoint>,
    request: ProcessRequest,
    cancellation: Option<&ExecutionCancellation>,
) -> io::Result<ProcessReply> {
    exchange_with_gate(endpoint, request, cancellation, None).await
}

async fn exchange_with_gate(
    endpoint: &Arc<LocalExecutionEndpoint>,
    request: ProcessRequest,
    cancellation: Option<&ExecutionCancellation>,
    start_gate: Option<StartGate>,
) -> io::Result<ProcessReply> {
    let deadline = Instant::now() + EXCHANGE_BUDGET;
    let request = Arc::new(request);
    loop {
        if cancellation.is_some_and(ExecutionCancellation::is_cancelled) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let endpoint = endpoint.clone();
        let request = request.clone();
        let cancellation = cancellation.cloned();
        let start_gate = start_gate.clone();
        let result = tokio::task::spawn_blocking(move || {
            endpoint.agent_process(&request, deadline, || {
                if start_gate.is_some_and(|gate| !gate()) {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                if let Some(cancellation) = cancellation {
                    cancellation
                        .with_active(|| ())
                        .ok_or(io::ErrorKind::Interrupted.into())
                } else {
                    Ok(())
                }
            })
        })
        .await
        .map_err(io::Error::other)?;
        match result {
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                tokio::time::sleep(POLL).await;
            }
            result => return result,
        }
    }
}

async fn deliver_cancel(
    endpoint: Arc<LocalExecutionEndpoint>,
    key: ProcessOperationKey,
    settled: Arc<std::sync::atomic::AtomicBool>,
) {
    let _ = tokio::task::spawn_blocking(move || {
        let deadline =
            Instant::now() + rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT;
        endpoint.cancel_operation(
            &key,
            deadline,
            settled.load(std::sync::atomic::Ordering::Acquire),
        );
    })
    .await;
}

fn unavailable_capture() -> BashCaptureResult {
    BashCaptureResult {
        stdout: String::new(),
        stderr: String::new(),
        combined: String::new(),
        truncated: true,
        total: None,
        output: BashCaptureState::Unavailable,
        started: true,
        signal: None,
    }
}

fn project(
    snapshot: SettledSnapshot,
    capture: Option<BashCaptureResult>,
    before_start: bool,
    live_output: bool,
    key: &ProcessOperationKey,
    cancellation: &ExecutionCancellation,
) -> ToolExecutionResult {
    let before_start = before_start || capture.as_ref().is_none_or(|capture| !capture.started);
    let capture_failed = capture.as_ref().is_some_and(|capture| {
        matches!(
            capture.output,
            BashCaptureState::Partial | BashCaptureState::Unavailable
        )
    });
    let status = match snapshot.disposition {
        SettledDisposition::Known => match snapshot.exit_code {
            Some(0) => ToolExecutionStatus::Success,
            Some(code) => ToolExecutionStatus::Failed {
                error: format!("command exited with code {code}"),
            },
            None => ToolExecutionStatus::Failed {
                error: capture
                    .as_ref()
                    .and_then(|capture| capture.signal)
                    .map_or_else(
                        || "command exited without an exit status".into(),
                        |signal| format!("command terminated by signal {signal}"),
                    ),
            },
        },
        SettledDisposition::Cancelled => cancellation.native_status(if before_start {
            ToolCancellationPhase::BeforeStart
        } else {
            ToolCancellationPhase::DuringExecution
        }),
        SettledDisposition::TimedOut => ToolExecutionStatus::TimedOut,
        SettledDisposition::Unknown => ToolExecutionStatus::OutcomeUnknown {
            detail: "the Execution Client could not prove the outcome of Agent Bash".into(),
        },
        SettledDisposition::Failed => ToolExecutionStatus::Failed {
            error: if capture_failed {
                "bash output capture failed"
            } else {
                "bash process control failed"
            }
            .into(),
        },
    };
    let exit_code = match snapshot.disposition {
        SettledDisposition::Known | SettledDisposition::Failed => snapshot.exit_code,
        _ => None,
    };
    let mut result = terminal(status);
    result.exit_code = exit_code;
    let path = locator(key);
    let output = capture.as_ref().map_or(
        if live_output {
            BashCaptureState::Complete
        } else {
            BashCaptureState::None
        },
        |capture| capture.output,
    );
    let diagnostic = "the Client could not retain all Bash output".to_owned();
    result.managed_output = match output {
        BashCaptureState::None => None,
        BashCaptureState::Complete => Some(ManagedOutputContinuation::Complete { locator: path }),
        BashCaptureState::Partial => Some(ManagedOutputContinuation::Partial {
            locator: path,
            diagnostic,
        }),
        BashCaptureState::Unavailable => {
            Some(ManagedOutputContinuation::Unavailable { diagnostic })
        }
    }
    .map(Box::new);
    if let Some(capture) = capture {
        result.truncation = capture.truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: capture.total.map(DecimalU64::get),
        });
        result
            .content
            .push(crate::tools::types::ToolResultContent::Json {
                value: serde_json::json!({
                    "exit_code": exit_code,
                    "stdout": capture.stdout,
                    "stderr": capture.stderr,
                    "combined": capture.combined,
                }),
            });
    }
    result
}

fn terminal(status: ToolExecutionStatus) -> ToolExecutionResult {
    ToolExecutionResult {
        deliveries: Vec::new(),
        status,
        content: Vec::new(),
        duration_ms: 0,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}
