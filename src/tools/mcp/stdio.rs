//! Server-side logical peer for one Client-owned stdio connection.

use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rmcp::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    service::{ClientTransportAdmission, RoleClient},
    transport::Transport,
};
use rustx_execution_protocol::{
    MAX_RAW_CHUNK_BYTES,
    mcp::{Failure, Launch, MAX_RECORD_BYTES, MAX_SEND_BYTES, Reply, Request},
};
use tokio::sync::mpsc;

use crate::{
    local_runtime::session_runtime_manager::{SessionExecutionScope, StdioMcpBinding},
    runtime::{
        cancellation::{CancellationSignal, ExecutionCancellation},
        types::CancellationReason,
    },
};

use super::McpError;
use super::framing::ProtocolViolationRecorder;

const POLL: Duration = Duration::from_millis(25);
const MAX_IDLE_POLL: Duration = Duration::from_millis(250);
const EXCHANGE: Duration = Duration::from_secs(5);
const STARTUP: Duration = Duration::from_secs(22);
const SHUTDOWN: Duration = Duration::from_secs(5);
const ADMISSION_BYTES: usize = 64 * 1024;

pub(super) struct Session {
    binding: StdioMcpBinding,
    closed: AtomicBool,
    violation: Arc<ProtocolViolationRecorder>,
}

pub(super) struct PeerTransport {
    owner: Arc<Session>,
    stop: CancellationSignal,
    receive: mpsc::Receiver<ServerJsonRpcMessage>,
    pump: Option<tokio::task::JoinHandle<()>>,
}

pub(super) fn client_relative_cwd(
    workspace: &Path,
    cwd: Option<&Path>,
) -> Result<std::path::PathBuf, McpError> {
    let cwd = cwd.unwrap_or(Path::new(""));
    if cwd == workspace || cwd == Path::new(".") {
        return Ok(std::path::PathBuf::new());
    }
    let relative = if cwd.is_absolute() {
        cwd.strip_prefix(workspace).map_err(|_| {
            McpError::Configuration("stdio cwd must be in the authorized Client Workspace".into())
        })?
    } else {
        cwd
    };
    if relative
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(McpError::Configuration(
            "stdio cwd is not a normalized Client path".into(),
        ));
    }
    Ok(relative.to_path_buf())
}

impl Session {
    pub(super) async fn open(
        scope: &SessionExecutionScope,
        workspace: &Path,
        launch: Launch,
        cancellation: &CancellationSignal,
        violation: Arc<ProtocolViolationRecorder>,
    ) -> Result<(Arc<Self>, PeerTransport, ClientTransportAdmission), McpError> {
        let binding = scope
            .bind_stdio_mcp(workspace)
            .map_err(|error| McpError::Discovery(error.to_string()))?;
        let owner = Arc::new(Self {
            binding,
            closed: AtomicBool::new(false),
            violation,
        });
        let admitted = owner.await_admission(launch, cancellation).await;
        match admitted {
            Ok(admission) => {
                let transport = PeerTransport::new(owner.clone());
                Ok((owner, transport, admission))
            }
            Err(error) => match owner.close().await {
                Ok(()) => Err(error),
                Err(settlement) => Err(McpError::PhysicalSettlement(format!(
                    "{error}; {settlement}"
                ))),
            },
        }
    }

    async fn await_admission(
        &self,
        launch: Launch,
        signal: &CancellationSignal,
    ) -> Result<ClientTransportAdmission, McpError> {
        let cancellation = cancellation(signal.clone());
        let deadline = Instant::now() + STARTUP;
        let mut request = Request::Open {
            key: self.binding.key.clone(),
            launch,
        };
        loop {
            let reply = self
                .binding
                .exchange(
                    request,
                    deadline.min(Instant::now() + EXCHANGE),
                    &cancellation,
                )
                .await
                .map_err(|error| McpError::Discovery(error.to_string()))?;
            match reply {
                Reply::Ready { admission_base64 } => {
                    let admission = decode(&admission_base64, ADMISSION_BYTES)
                        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(io::Error::other))
                        .map_err(|_| {
                            McpError::ProtocolViolation("invalid stdio admission receipt".into())
                        })?;
                    return Ok(admission);
                }
                Reply::Opening => (),
                Reply::Failed(Failure::UnsupportedRevision) => {
                    return Err(McpError::ProtocolCompatibility(
                        "Client stdio MCP found no compatible revision".into(),
                    ));
                }
                Reply::Failed(Failure::InvalidRecord) => {
                    self.violation.record_client_rejection();
                    return Err(McpError::ProtocolViolation(
                        "Client stdio MCP rejected a malformed or oversized record".into(),
                    ));
                }
                Reply::Failed(failure) => {
                    return Err(McpError::Discovery(format!(
                        "Client stdio MCP admission failed ({failure:?})"
                    )));
                }
                _ => {
                    return Err(McpError::Discovery(
                        "Client stdio MCP closed during admission".into(),
                    ));
                }
            }
            if !pause(signal, deadline, POLL).await {
                return Err(McpError::Discovery(
                    "Client stdio MCP did not admit before cancellation or deadline".into(),
                ));
            }
            request = Request::State {
                key: self.binding.key.clone(),
            };
        }
    }

    pub(super) async fn close(&self) -> Result<(), McpError> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let signal = CancellationSignal::new();
        let cancellation = cancellation(signal.clone());
        let deadline = Instant::now() + SHUTDOWN;
        let result = loop {
            let reply = self
                .binding
                .exchange(
                    Request::Close {
                        key: self.binding.key.clone(),
                    },
                    deadline,
                    &cancellation,
                )
                .await;
            match reply {
                Ok(
                    Reply::Closed
                    | Reply::Failed(
                        Failure::Unavailable
                        | Failure::Handshake
                        | Failure::UnsupportedRevision
                        | Failure::InvalidRecord
                        | Failure::Transport,
                    ),
                ) => break Ok(()),
                Ok(Reply::Closing) if pause(&signal, deadline, POLL).await => (),
                _ => break self.binding.settle_binding().await,
            }
        };
        result.map_err(|error| McpError::PhysicalSettlement(error.to_string()))?;
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.closed.load(Ordering::Acquire) {
            self.binding.abandon();
        }
    }
}

impl PeerTransport {
    fn new(owner: Arc<Session>) -> Self {
        let stop = CancellationSignal::new();
        let reader = owner.clone();
        let signal = stop.clone();
        let (deliver, receive) = mpsc::channel(1);
        let pump = tokio::spawn(async move {
            reader.read_messages(deliver, signal).await;
        });
        Self {
            owner,
            stop,
            receive,
            pump: Some(pump),
        }
    }
}

impl Drop for PeerTransport {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl Transport<RoleClient> for PeerTransport {
    type Error = io::Error;

    fn send(
        &mut self,
        message: ClientJsonRpcMessage,
    ) -> impl Future<Output = io::Result<()>> + Send + 'static {
        let owner = self.owner.clone();
        let stop = self.stop.clone();
        async move {
            let bytes = serde_json::to_vec(&message).map_err(io::Error::other)?;
            if bytes.len() > MAX_SEND_BYTES {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            let request = Request::Send {
                key: owner.binding.key.clone(),
                data_base64: STANDARD.encode(bytes),
            };
            let reply = owner
                .binding
                .exchange(request, Instant::now() + EXCHANGE, &cancellation(stop))
                .await?;
            match reply {
                Reply::Queued => Ok(()),
                Reply::Failed(Failure::Capacity) => Err(io::ErrorKind::WouldBlock.into()),
                _ => Err(io::ErrorKind::NotConnected.into()),
            }
        }
    }

    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        self.receive.recv().await
    }

    async fn close(&mut self) -> io::Result<()> {
        self.stop.cancel();
        if let Some(pump) = self.pump.take() {
            pump.await.map_err(io::Error::other)?;
        }
        Ok(())
    }
}

impl Session {
    async fn read_messages(
        &self,
        deliver: mpsc::Sender<ServerJsonRpcMessage>,
        stop: CancellationSignal,
    ) {
        let cancellation = cancellation(stop.clone());
        let mut sequence = 1_u64;
        let mut data = Vec::new();
        let mut idle = POLL;
        loop {
            let reply = self
                .binding
                .exchange(
                    Request::Receive {
                        key: self.binding.key.clone(),
                        sequence: sequence.into(),
                        offset: (data.len() as u64).into(),
                    },
                    Instant::now() + EXCHANGE,
                    &cancellation,
                )
                .await;
            match reply {
                Ok(Reply::Page {
                    data_base64, last, ..
                }) => {
                    let Ok(bytes) = decode(&data_base64, MAX_RAW_CHUNK_BYTES) else {
                        self.violation.record_client_rejection();
                        return;
                    };
                    if bytes.is_empty() || data.len() + bytes.len() > MAX_RECORD_BYTES {
                        self.violation.record_client_rejection();
                        return;
                    }
                    data.extend(bytes);
                    idle = POLL;
                    if last {
                        let Ok(message) = serde_json::from_slice(&data) else {
                            self.violation.record_client_rejection();
                            return;
                        };
                        data.clear();
                        let sent = tokio::select! {
                            biased;
                            () = stop.cancelled() => return,
                            sent = deliver.send(message) => sent,
                        };
                        if sent.is_err() {
                            return;
                        }
                        let Some(next) = sequence.checked_add(1) else {
                            return;
                        };
                        sequence = next;
                    }
                }
                Ok(Reply::Pending) => {
                    tokio::select! {
                        biased;
                        () = stop.cancelled() => return,
                        () = tokio::time::sleep(idle) => (),
                    }
                    idle = idle.saturating_mul(2).min(MAX_IDLE_POLL);
                }
                Ok(Reply::Failed(Failure::InvalidRecord)) => {
                    self.violation.record_client_rejection();
                    return;
                }
                _ => return,
            }
        }
    }
}

async fn pause(signal: &CancellationSignal, deadline: Instant, interval: Duration) -> bool {
    tokio::select! {
        biased;
        () = signal.cancelled() => false,
        () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline.min(Instant::now() + interval))) => Instant::now() < deadline,
    }
}

fn decode(encoded: &str, limit: usize) -> io::Result<Vec<u8>> {
    if encoded.len() > limit.div_ceil(3) * 4 {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    let bytes = STANDARD.decode(encoded).map_err(io::Error::other)?;
    if bytes.len() > limit {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    Ok(bytes)
}

fn cancellation(signal: CancellationSignal) -> ExecutionCancellation {
    ExecutionCancellation::detached(signal, CancellationReason::UserRequested)
}
