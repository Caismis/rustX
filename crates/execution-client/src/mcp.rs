//! Physical ownership of admitted stdio MCP connections.

use std::{
    collections::HashMap,
    future::Future,
    io::{self, Write},
    path::Component,
    sync::{Arc, Mutex},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::future::BoxFuture;
use rmcp::{
    ClientHandler, ClientLifecycleMode,
    model::{
        ClientCapabilities, ClientInfo, ClientJsonRpcMessage, ProtocolVersion, ServerJsonRpcMessage,
    },
    service::{ClientInitializeError, negotiate_client_transport},
    transport::{Transport, async_rw::AsyncRwTransport},
};
use rustx_execution_protocol::{
    MAX_RAW_CHUNK_BYTES,
    mcp::{
        Failure, Launch, MAX_PENDING_SENDS, MAX_RECORD_BYTES, MAX_SEND_BYTES, MAX_SESSIONS, Reply,
        Request, Response,
    },
    process::{OperationId, ProcessOperationKey},
};
use rustx_process_supervisor::{
    interactive_process::{InteractiveProcessSpec, SupervisedInteractiveProcess},
    signal::ProcessSignal,
};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::process::{Operations, WorkerGuard};

const MAX_IDENTITIES: usize = 4096;
const MAX_ADMISSION_BYTES: usize = 64 * 1024;
const HANDSHAKE_BUDGET: Duration = Duration::from_secs(20);
const SEND_BUDGET: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct Sessions {
    entries: HashMap<OperationId, Entry>,
}

enum Entry {
    Session(Session),
    Retired {
        fingerprint: [u8; 32],
        failure: Option<Failure>,
    },
}

struct Session {
    fingerprint: [u8; 32],
    signal: ProcessSignal,
    status: Arc<Mutex<Status>>,
    outgoing: mpsc::Sender<ClientJsonRpcMessage>,
    incoming: mpsc::Receiver<Vec<u8>>,
    current: Option<Vec<u8>>,
    sequence: u64,
    offset: usize,
}

enum Status {
    Opening,
    Ready(String),
    Settled(Option<Failure>),
}

impl Sessions {
    pub(crate) fn exchange(&mut self, request: Request, operations: &Arc<Operations>) -> Response {
        let key = request.key().clone();
        let reply = if operations.own_key(&key) {
            self.dispatch(request, &key, operations)
        } else {
            Reply::Failed(Failure::Foreign)
        };
        Response { key, reply }
    }

    fn dispatch(
        &mut self,
        request: Request,
        key: &ProcessOperationKey,
        operations: &Arc<Operations>,
    ) -> Reply {
        if let Request::Open { launch, .. } = request {
            return self.open(key.operation, launch, operations);
        }
        let Some(entry) = self.entries.get_mut(&key.operation) else {
            return Reply::Failed(Failure::Unavailable);
        };
        let Entry::Session(session) = entry else {
            return entry.state();
        };
        match request {
            Request::Open { .. } => unreachable!("handled above"),
            Request::State { .. } => session.state(),
            Request::Send { data_base64, .. } => session.send(&data_base64),
            Request::Receive {
                sequence, offset, ..
            } => session.receive(sequence.get(), offset.get()),
            Request::Close { .. } => {
                session.signal.cancel();
                let Status::Settled(failure) = *session.status.lock().expect("stdio MCP status")
                else {
                    return Reply::Closing;
                };
                *entry = Entry::Retired {
                    fingerprint: session.fingerprint,
                    failure,
                };
                entry.state()
            }
        }
    }

    fn open(&mut self, id: OperationId, launch: Launch, operations: &Arc<Operations>) -> Reply {
        let Ok(bytes) = serde_json::to_vec(&launch) else {
            return Reply::Failed(Failure::Invalid);
        };
        let fingerprint: [u8; 32] = Sha256::digest(bytes).into();
        if let Some(registered) = self.entries.get(&id) {
            return if registered.fingerprint() == &fingerprint {
                registered.state()
            } else {
                Reply::Failed(Failure::Invalid)
            };
        }
        if self.entries.len() == MAX_IDENTITIES
            || self
                .entries
                .values()
                .filter(|entry| matches!(entry, Entry::Session(_)))
                .count()
                >= MAX_SESSIONS
        {
            return Reply::Failed(Failure::Capacity);
        }
        let Some((spec, lifecycle, info)) = launch_parameters(launch, operations) else {
            return Reply::Failed(Failure::Invalid);
        };
        let (outgoing, requests) = mpsc::channel(MAX_PENDING_SENDS);
        let (replies, incoming) = mpsc::channel(1);
        let signal = operations.auxiliary_signal();
        let status = Arc::new(Mutex::new(Status::Opening));
        self.entries.insert(
            id,
            Entry::Session(Session {
                fingerprint,
                signal: signal.clone(),
                status: status.clone(),
                outgoing,
                incoming,
                current: None,
                sequence: 1,
                offset: 0,
            }),
        );
        if !operations.start_auxiliary_owned(move |worker| async move {
            run(
                spec, lifecycle, info, signal, status, requests, replies, worker,
            )
            .await;
        }) {
            self.entries.insert(
                id,
                Entry::Retired {
                    fingerprint,
                    failure: Some(Failure::Unavailable),
                },
            );
            return Reply::Failed(Failure::Unavailable);
        }
        Reply::Opening
    }
}

impl Entry {
    fn fingerprint(&self) -> &[u8; 32] {
        match self {
            Self::Session(session) => &session.fingerprint,
            Self::Retired { fingerprint, .. } => fingerprint,
        }
    }

    fn state(&self) -> Reply {
        match self {
            Self::Session(session) => session.state(),
            Self::Retired { failure, .. } => failure.map_or(Reply::Closed, Reply::Failed),
        }
    }
}

impl Session {
    fn state(&self) -> Reply {
        match &*self.status.lock().expect("stdio MCP status") {
            Status::Opening => Reply::Opening,
            Status::Ready(_) if self.signal.is_cancelled() => Reply::Closing,
            Status::Ready(admission) => Reply::Ready {
                admission_base64: admission.clone(),
            },
            Status::Settled(failure) => failure.map_or(Reply::Closed, Reply::Failed),
        }
    }

    fn send(&self, encoded: &str) -> Reply {
        let result = if encoded.len() <= MAX_SEND_BYTES.div_ceil(3) * 4 {
            STANDARD
                .decode(encoded)
                .ok()
                .filter(|bytes| bytes.len() <= MAX_SEND_BYTES)
                .and_then(|bytes| serde_json::from_slice::<ClientJsonRpcMessage>(&bytes).ok())
        } else {
            None
        };
        let Some(message) = result else {
            return Reply::Failed(Failure::Invalid);
        };
        if self.signal.is_cancelled()
            || !matches!(
                *self.status.lock().expect("stdio MCP status"),
                Status::Ready(_)
            )
        {
            return Reply::Failed(Failure::Unavailable);
        }
        // The physical Client negotiated exactly once. Handshake messages
        // from a logical peer cannot reacquire or renegotiate this connection.
        if matches!(
            &message,
            ClientJsonRpcMessage::Request(request) if matches!(
                request.request, rmcp::model::ClientRequest::InitializeRequest(_)
                    | rmcp::model::ClientRequest::DiscoverRequest(_)
            )
        ) {
            return Reply::Failed(Failure::Invalid);
        }
        match self.outgoing.try_send(message) {
            Ok(()) => Reply::Queued,
            Err(mpsc::error::TrySendError::Full(_)) => Reply::Failed(Failure::Capacity),
            Err(mpsc::error::TrySendError::Closed(_)) => Reply::Failed(Failure::Unavailable),
        }
    }

    fn receive(&mut self, sequence: u64, offset: u64) -> Reply {
        if self.sequence != sequence || self.offset as u64 != offset {
            return Reply::Failed(Failure::Invalid);
        }
        if self.current.is_none() {
            self.current = match self.incoming.try_recv() {
                Ok(message) => Some(message),
                Err(mpsc::error::TryRecvError::Empty) => return Reply::Pending,
                Err(mpsc::error::TryRecvError::Disconnected) => return self.state(),
            };
        }
        let Some(current) = &self.current else {
            return Reply::Pending;
        };
        let end = (self.offset + MAX_RAW_CHUNK_BYTES).min(current.len());
        let data_base64 = STANDARD.encode(&current[self.offset..end]);
        let last = end == current.len();
        if last {
            self.current = None;
            self.sequence = self.sequence.checked_add(1).expect("stdio MCP sequence");
            self.offset = 0;
        } else {
            self.offset = end;
        }
        Reply::Page {
            sequence: sequence.into(),
            offset: offset.into(),
            data_base64,
            last,
        }
    }
}

fn known_version(value: &str) -> Option<ProtocolVersion> {
    ProtocolVersion::KNOWN_VERSIONS
        .iter()
        .find(|version| version.as_str() == value)
        .cloned()
}

fn launch_parameters(
    launch: Launch,
    operations: &Operations,
) -> Option<(InteractiveProcessSpec, ClientLifecycleMode, ClientInfo)> {
    if launch.versions.is_empty()
        || launch.versions.len() > ProtocolVersion::KNOWN_VERSIONS.len()
        || launch.program.as_os_str().is_empty()
        || launch.args.len() > 128
        || launch.environment.len() > 128
        || launch
            .cwd
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || operations.workspace_root().verify().is_err()
    {
        return None;
    }
    operations
        .workspace_root()
        .descend(&launch.cwd)
        .ok()?
        .verify()
        .ok()?;
    let cwd = operations.workspace().join(&launch.cwd);
    let versions = launch
        .versions
        .iter()
        .map(|version| known_version(version))
        .collect::<Option<Vec<_>>>()?;
    let legacy = launch.legacy_version.as_deref().map(known_version);
    let lifecycle = match legacy {
        Some(Some(version)) => ClientLifecycleMode::Auto {
            preferred_versions: versions,
            legacy_version: Some(version),
        },
        Some(None) => return None,
        None => ClientLifecycleMode::Discover {
            preferred_versions: versions,
        },
    };
    let mut info = ClientInfo::new(
        ClientCapabilities::default(),
        rmcp::model::Implementation::new("rustx", env!("CARGO_PKG_VERSION")),
    );
    if let Some(Some(version)) = launch.legacy_version.as_deref().map(known_version) {
        info = info.with_protocol_version(version);
    }
    let program = if launch.program.is_relative()
        && (launch.program.components().count() > 1 || launch.program.starts_with("."))
    {
        if launch
            .program
            .components()
            .any(|part| !matches!(part, Component::CurDir | Component::Normal(_)))
        {
            return None;
        }
        cwd.join(launch.program)
    } else {
        launch.program
    };
    Some((
        InteractiveProcessSpec {
            cwd,
            program,
            args: launch.args,
            environment: launch.environment.into_iter().collect(),
        },
        lifecycle,
        info,
    ))
}

struct Negotiator(ClientInfo);
impl ClientHandler for Negotiator {
    fn get_info(&self) -> ClientInfo {
        self.0.clone()
    }
}

type Stdio =
    AsyncRwTransport<rmcp::RoleClient, tokio::process::ChildStdout, tokio::process::ChildStdin>;

#[allow(clippy::too_many_arguments)] // One counted physical-owner transition.
async fn run(
    spec: InteractiveProcessSpec,
    lifecycle: ClientLifecycleMode,
    info: ClientInfo,
    signal: ProcessSignal,
    status: Arc<Mutex<Status>>,
    requests: mpsc::Receiver<ClientJsonRpcMessage>,
    replies: mpsc::Sender<Vec<u8>>,
    mut worker: WorkerGuard,
) {
    worker.physical_settlement(false);
    let Ok(mut process) = SupervisedInteractiveProcess::spawn_cancellable(spec, signal.clone())
    else {
        worker.physical_settlement(true);
        *status.lock().expect("stdio MCP status") = Status::Settled(Some(Failure::Unavailable));
        return;
    };
    let failure = if let (Some(stdout), Some(stdin)) = (process.stdout.take(), process.stdin.take())
    {
        let mut pipe = AsyncRwTransport::new_client(stdout, stdin)
            .with_received_record_limit(MAX_RECORD_BYTES);
        let result = negotiate(&mut pipe, &info, lifecycle, &signal).await;
        match result {
            Ok(admission) => {
                *status.lock().expect("stdio MCP status") = Status::Ready(admission);
                transfer(&mut pipe, requests, &replies, &signal).await
            }
            Err(failure) => failure,
        }
    } else {
        Some(Failure::Transport)
    };
    process.request_shutdown();
    let proven = process.wait_for_settlement().await.is_ok();
    worker.physical_settlement(proven);
    *status.lock().expect("stdio MCP status") = Status::Settled(if proven {
        failure
    } else {
        Some(Failure::SettlementUnconfirmed)
    });
}

async fn negotiate(
    pipe: &mut Stdio,
    info: &ClientInfo,
    lifecycle: ClientLifecycleMode,
    signal: &ProcessSignal,
) -> Result<String, Option<Failure>> {
    let negotiator = Negotiator(info.clone());
    let admitted = tokio::select! {
        biased;
        () = signal.cancelled() => return Err(None),
        result = tokio::time::timeout(HANDSHAKE_BUDGET, negotiate_client_transport(&negotiator, pipe, lifecycle)) => result,
    };
    if pipe.invalid_record() {
        return Err(Some(Failure::InvalidRecord));
    }
    let admitted = match admitted {
        Ok(Ok(admitted)) => admitted,
        Ok(Err(ClientInitializeError::NoCompatibleProtocolVersion { .. })) => {
            return Err(Some(Failure::UnsupportedRevision));
        }
        _ => return Err(Some(Failure::Handshake)),
    };
    if known_version(admitted.peer_info.protocol_version.as_str()).is_none() {
        return Err(Some(Failure::UnsupportedRevision));
    }
    encode_bounded(&admitted, MAX_ADMISSION_BYTES)
        .map(|bytes| STANDARD.encode(bytes))
        .map_err(|_| Some(Failure::InvalidRecord))
}

async fn transfer(
    pipe: &mut Stdio,
    mut requests: mpsc::Receiver<ClientJsonRpcMessage>,
    replies: &mpsc::Sender<Vec<u8>>,
    signal: &ProcessSignal,
) -> Option<Failure> {
    let mut sending: Option<BoxFuture<'static, Result<(), Option<Failure>>>> = None;
    let mut pending = None;
    loop {
        tokio::select! {
            biased;
            () = signal.cancelled() => return None,
            sent = poll_send(sending.as_mut()), if sending.is_some() => {
                if let Err(failure) = sent {
                    return failure;
                }
                sending = None;
            }
            capacity = replies.reserve(), if pending.is_some() => {
                let Ok(capacity) = capacity else { return None; };
                capacity.send(pending.take().expect("buffered response"));
            }
            message = requests.recv(), if sending.is_none() => {
                let message = message?;
                let send = pipe.send(message);
                sending = Some(Box::pin(async move {
                    match tokio::time::timeout(SEND_BUDGET, send).await {
                        Ok(Ok(())) => Ok(()),
                        _ => Err(Some(Failure::Transport)),
                    }
                }));
            }
            message = pipe.receive(), if pending.is_none() => match message {
                Some(message) => {
                    pending = match encode_bounded::<ServerJsonRpcMessage>(&message, MAX_RECORD_BYTES) {
                        Ok(bytes) => Some(bytes),
                        Err(_) => return Some(Failure::InvalidRecord),
                    };
                }
                None if pipe.invalid_record() => return Some(Failure::InvalidRecord),
                None => return Some(Failure::Transport),
            },
        }
    }
}

async fn poll_send<F: Future>(sending: Option<F>) -> F::Output {
    match sending {
        Some(sending) => sending.await,
        None => std::future::pending().await,
    }
}

fn encode_bounded<T: serde::Serialize>(value: &T, limit: usize) -> serde_json::Result<Vec<u8>> {
    struct Bounded(Vec<u8>, usize);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.1 - self.0.len() {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Bounded(Vec::new(), limit);
    serde_json::to_writer(&mut buffer, value)?;
    Ok(buffer.0)
}
