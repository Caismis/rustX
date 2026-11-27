//! Physical ownership of admitted stdio MCP connections.

use std::{
    collections::HashMap,
    future::Future,
    io::{self, Write},
    path::{Component, Path},
    sync::{Arc, Mutex},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::future::BoxFuture;
use rmcp::{
    ClientHandler, ClientLifecycleMode, RoleClient,
    model::{
        ClientCapabilities, ClientInfo, ClientJsonRpcMessage, ClientRequest, GetMeta,
        ProtocolVersion, RequestMetaObject, ServerJsonRpcMessage,
    },
    service::{ClientInitializeError, negotiate_client_transport},
    transport::{Transport, async_rw::AsyncRwTransport},
};
use rustx_execution_protocol::{
    MAX_RAW_CHUNK_BYTES,
    mcp::{
        Failure, Launch, MAX_PENDING_SENDS, MAX_RECORD_BYTES, MAX_SEND_BYTES, MAX_SESSIONS,
        MAX_STARTUP_STDERR_BYTES, Reply, Request, Response, StartupStderr,
    },
    process::{OperationId, ProcessOperationKey},
};
use rustx_process_supervisor::{
    interactive_process::{InteractiveProcessSpec, SupervisedInteractiveProcess},
    signal::ProcessSignal,
};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::{
    managed_python,
    process::{Operations, WorkerGuard},
};

const MAX_IDENTITIES: usize = 4096;
const MAX_ADMISSION_BYTES: usize = 64 * 1024;
const HANDSHAKE_BUDGET: Duration = Duration::from_secs(20);
const SEND_BUDGET: Duration = Duration::from_secs(5);
const STARTUP_STDERR_BUDGET: Duration = Duration::from_millis(250);

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
    Settled {
        failure: Option<Failure>,
        startup_stderr: Option<StartupStderr>,
    },
}

impl Sessions {
    /// A detached transport must not leave Client-owned MCP process groups
    /// running while the binding waits for a possible reconnection.
    pub(crate) fn cancel_all(&self) {
        for entry in self.entries.values() {
            if let Entry::Session(session) = entry {
                session.signal.cancel();
            }
        }
    }

    pub(crate) fn exchange(
        &mut self,
        request: Request,
        operations: &Arc<Operations>,
        python: &managed_python::Catalog,
    ) -> Response {
        let key = request.key().clone();
        let reply = if operations.own_key(&key) {
            self.dispatch(request, &key, operations, python)
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
        python: &managed_python::Catalog,
    ) -> Reply {
        if let Request::Open {
            source_id, launch, ..
        } = request
        {
            return self.open(key.operation, &source_id, launch, operations, python);
        }
        let Some(entry) = self.entries.get_mut(&key.operation) else {
            return Reply::Failed(Failure::Unavailable);
        };
        let Entry::Session(session) = entry else {
            return entry.state();
        };
        match request {
            Request::Open { .. } => unreachable!("handled above"),
            Request::State { .. } => session.state(true),
            Request::Send { data_base64, .. } => session.send(&data_base64),
            Request::Receive {
                sequence, offset, ..
            } => session.receive(sequence.get(), offset.get()),
            Request::Close { .. } => {
                session.signal.cancel();
                let Status::Settled { failure, .. } =
                    *session.status.lock().expect("stdio MCP status")
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

    fn open(
        &mut self,
        id: OperationId,
        source_id: &str,
        launch: Launch,
        operations: &Arc<Operations>,
        python: &managed_python::Catalog,
    ) -> Reply {
        let Ok(bytes) = serde_json::to_vec(&(source_id, &launch)) else {
            return Reply::Failed(Failure::Invalid);
        };
        if source_id.is_empty() {
            return Reply::Failed(Failure::Invalid);
        }
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
        let Some((spec, lifecycle, info)) = launch_parameters(launch, operations, python) else {
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

impl Drop for Session {
    fn drop(&mut self) {
        self.signal.cancel();
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
            Self::Session(session) => session.state(true),
            Self::Retired { failure, .. } => failure.map_or(Reply::Closed, Reply::Failed),
        }
    }
}

impl Session {
    fn state(&self, include_startup_stderr: bool) -> Reply {
        match &*self.status.lock().expect("stdio MCP status") {
            Status::Opening => Reply::Opening,
            Status::Ready(_) if self.signal.is_cancelled() => Reply::Closing,
            Status::Ready(admission) => Reply::Ready {
                admission_base64: admission.clone(),
            },
            Status::Settled {
                failure: Some(failure),
                startup_stderr: Some(stderr),
            } if include_startup_stderr => Reply::StartupFailed {
                failure: *failure,
                stderr: stderr.clone(),
            },
            Status::Settled { failure, .. } => failure.map_or(Reply::Closed, Reply::Failed),
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
                Err(mpsc::error::TryRecvError::Disconnected) => return self.state(false),
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
    python: &managed_python::Catalog,
) -> Option<(InteractiveProcessSpec, ClientLifecycleMode, ClientInfo)> {
    if launch.versions.is_empty()
        || launch.versions.len() > ProtocolVersion::KNOWN_VERSIONS.len()
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
    if let Some(source) = &launch.python_source {
        if !launch.program.as_os_str().is_empty()
            || !launch.args.is_empty()
            || !launch.environment.is_empty()
            || !launch.cwd.as_os_str().is_empty()
            || launch.legacy_version.is_some()
        {
            return None;
        }
        return Some((python.prepared_launch(source, operations)?, lifecycle, info));
    }
    // Classify the authored interpreter spelling on both sides, even when
    // the Client pins that executable through a Workspace symlink.
    let args = client_arguments(
        operations.workspace_root(),
        operations.workspace(),
        &launch.program,
        launch.args,
    )?;
    let program = client_program(
        operations.workspace_root(),
        operations.workspace(),
        &launch.cwd,
        launch.program,
    )?;
    Some((
        InteractiveProcessSpec {
            cwd,
            program,
            args,
            environment: launch.environment.into_iter().collect(),
        },
        lifecycle,
        info,
    ))
}

// Path-valued Workspace programs use the same contained resolution as
// authored resources. Explicit system executables and PATH names are
// provided by the admitted Client's runtime.
fn client_program(
    root: &rustx_confined_fs::PackageDirectory,
    workspace: &std::path::Path,
    cwd: &std::path::Path,
    program: std::path::PathBuf,
) -> Option<std::path::PathBuf> {
    let bytes = program.as_os_str().as_encoded_bytes();
    if bytes.is_empty()
        || bytes.contains(&0)
        || program
            .to_str()
            .is_some_and(rustx_execution_protocol::mcp::foreign_stdio_path)
        || program
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return None;
    }
    let relative = if program.is_absolute() {
        let Ok(path) = program.strip_prefix(workspace) else {
            return std::fs::metadata(&program)
                .ok()?
                .is_file()
                .then_some(program);
        };
        path.to_path_buf()
    } else if bytes.contains(&b'/') || program.starts_with(".") {
        cwd.join(program)
    } else {
        return Some(program);
    };
    let normalized = relative
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect::<std::path::PathBuf>();
    let resolved = root.resolve_resource(workspace, &normalized).ok()?.entry?;
    resolved
        .metadata
        .is_file
        .then(|| workspace.join(resolved.relative_path))
}

/// Required input operands must exist; other path operands may name a new leaf
/// beneath a writable, existing Client directory.
fn client_arguments(
    root: &rustx_confined_fs::PackageDirectory,
    workspace: &std::path::Path,
    program: &std::path::Path,
    args: Vec<String>,
) -> Option<Vec<String>> {
    use rustx_execution_protocol::{EntryKind, mcp::StdioPathKind};

    for arg in rustx_execution_protocol::mcp::stdio_argument_paths(program, &args) {
        let path = Path::new(arg.value);
        let kind = existing_argument(root, workspace, path).ok()?;
        let admitted = match arg.kind {
            StdioPathKind::Input => kind.is_some(),
            StdioPathKind::Directory => kind == Some(EntryKind::Directory),
            StdioPathKind::CommonJs => commonjs_input_exists(root, workspace, path, kind)?,
            StdioPathKind::WritableLeaf => kind.is_some() || writable_parent(root, workspace, path),
        };
        if !admitted {
            return None;
        }
    }
    Some(args)
}

/// Distinguish a missing operand from one that cannot be admitted.
fn existing_argument(
    root: &rustx_confined_fs::PackageDirectory,
    workspace: &Path,
    path: &Path,
) -> io::Result<Option<rustx_execution_protocol::EntryKind>> {
    use rustx_execution_protocol::EntryKind;

    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let classify = |is_file, is_directory| {
        if is_file {
            EntryKind::RegularFile
        } else if is_directory {
            EntryKind::Directory
        } else {
            EntryKind::Other
        }
    };
    if path == workspace {
        root.verify()?;
        return Ok(Some(EntryKind::Directory));
    }
    if let Ok(relative) = path.strip_prefix(workspace) {
        if let Some(entry) = root.resolve_resource(workspace, relative)?.entry {
            return Ok(Some(classify(
                entry.metadata.is_file,
                entry.metadata.is_directory,
            )));
        }
        return match std::fs::symlink_metadata(path) {
            Ok(_) => Err(io::ErrorKind::PermissionDenied.into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        };
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => {
            let metadata = std::fs::metadata(path)?;
            Ok(Some(classify(metadata.is_file(), metadata.is_dir())))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn commonjs_input_exists(
    root: &rustx_confined_fs::PackageDirectory,
    workspace: &Path,
    path: &Path,
    exact: Option<rustx_execution_protocol::EntryKind>,
) -> Option<bool> {
    use rustx_execution_protocol::EntryKind;

    if exact == Some(EntryKind::RegularFile) {
        return Some(true);
    }
    // Match the file candidates Node tries before directory resolution. The
    // interpreter still owns package.json, index files and module execution.
    for extension in [".js", ".json", ".node"] {
        let mut candidate = path.as_os_str().to_owned();
        candidate.push(extension);
        if existing_argument(root, workspace, Path::new(&candidate)).ok()?
            == Some(EntryKind::RegularFile)
        {
            return Some(true);
        }
    }
    Some(exact == Some(EntryKind::Directory))
}

fn writable_parent(
    root: &rustx_confined_fs::PackageDirectory,
    workspace: &std::path::Path,
    path: &std::path::Path,
) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let parent = if let Ok(relative) = parent.strip_prefix(workspace) {
        if relative.as_os_str().is_empty() {
            if root.verify().is_err() {
                return false;
            }
            workspace.to_path_buf()
        } else {
            let Some(entry) = root
                .resolve_resource(workspace, relative)
                .ok()
                .and_then(|result| result.entry)
            else {
                return false;
            };
            if !entry.metadata.is_directory {
                return false;
            }
            workspace.join(entry.relative_path)
        }
    } else {
        parent.to_path_buf()
    };
    std::fs::metadata(&parent).is_ok_and(|metadata| metadata.is_dir())
        && nix::unistd::access(
            &parent,
            nix::unistd::AccessFlags::W_OK | nix::unistd::AccessFlags::X_OK,
        )
        .is_ok()
}

struct Negotiator(ClientInfo);
impl ClientHandler for Negotiator {
    fn get_info(&self) -> ClientInfo {
        self.0.clone()
    }
}

type Stdio = AsyncRwTransport<RoleClient, tokio::process::ChildStdout, tokio::process::ChildStdin>;

struct StartupTransport<T> {
    pipe: T,
    sent_initialize: bool,
}

impl<T: Transport<RoleClient>> Transport<RoleClient> for StartupTransport<T> {
    type Error = T::Error;

    fn send(
        &mut self,
        message: ClientJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.sent_initialize |= matches!(
            &message,
            ClientJsonRpcMessage::Request(request)
                if matches!(request.request, ClientRequest::InitializeRequest(_))
        );
        self.pipe.send(message)
    }

    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        self.pipe.receive().await
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.pipe.close().await
    }
}

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
        *status.lock().expect("stdio MCP status") = Status::Settled {
            failure: Some(Failure::Unavailable),
            startup_stderr: None,
        };
        return;
    };
    let (failure, startup_failed) =
        if let (Some(stdout), Some(stdin)) = (process.stdout.take(), process.stdin.take()) {
            let mut startup = StartupTransport {
                pipe: AsyncRwTransport::new_client(stdout, stdin)
                    .with_received_record_limit(MAX_RECORD_BYTES),
                sent_initialize: false,
            };
            let result = negotiate(&mut startup, &info, lifecycle, &signal).await;
            match result {
                Ok((admission, request_context)) => {
                    *status.lock().expect("stdio MCP status") = Status::Ready(admission);
                    (
                        transfer(
                            &mut startup.pipe,
                            request_context,
                            requests,
                            &replies,
                            &signal,
                        )
                        .await,
                        false,
                    )
                }
                Err(failure) => (failure, failure.is_some()),
            }
        } else {
            (Some(Failure::Transport), true)
        };
    process.request_shutdown();
    let proven = process.wait_for_settlement().await.is_ok();
    worker.physical_settlement(proven);
    // Optional diagnostic capture carries no physical worker responsibility.
    drop(worker);
    let startup_stderr = if proven && startup_failed {
        let cutoff = tokio::time::Instant::now() + STARTUP_STDERR_BUDGET;
        Some(
            process
                .complete_stderr_by(cutoff)
                .await
                .filter(|text| text.len() <= MAX_STARTUP_STDERR_BYTES)
                .map_or(StartupStderr::Incomplete, |text| StartupStderr::Complete {
                    text,
                }),
        )
    } else {
        None
    };
    *status.lock().expect("stdio MCP status") = Status::Settled {
        failure: if proven {
            failure
        } else {
            Some(Failure::SettlementUnconfirmed)
        },
        startup_stderr,
    };
}

async fn negotiate(
    startup: &mut StartupTransport<Stdio>,
    info: &ClientInfo,
    lifecycle: ClientLifecycleMode,
    signal: &ProcessSignal,
) -> Result<(String, Option<RequestMetaObject>), Option<Failure>> {
    let negotiator = Negotiator(info.clone());
    let admitted = tokio::select! {
        biased;
        () = signal.cancelled() => return Err(None),
        result = tokio::time::timeout(HANDSHAKE_BUDGET, negotiate_client_transport(&negotiator, startup, lifecycle)) => result,
    };
    if startup.pipe.invalid_record() {
        return Err(Some(Failure::InvalidRecord));
    }
    let admitted = match admitted {
        Ok(Ok(admitted)) => admitted,
        Ok(Err(ClientInitializeError::NoCompatibleProtocolVersion { .. })) => {
            return Err(Some(Failure::UnsupportedRevision));
        }
        _ => return Err(Some(Failure::Handshake)),
    };
    let version = &admitted.peer_info.protocol_version;
    if known_version(version.as_str()).is_none() {
        return Err(Some(Failure::UnsupportedRevision));
    }
    // Discovery can select an older revision and still require context on
    // every request. The SDK restores context itself for newer revisions.
    let request_context = (!startup.sent_initialize
        && version.as_str() < ProtocolVersion::V_2026_07_28.as_str())
    .then(|| {
        RequestMetaObject::with_client_context(
            version.clone(),
            info.client_info.clone(),
            info.capabilities.clone(),
        )
    });
    let admission = encode_bounded(&admitted, MAX_ADMISSION_BYTES)
        .map(|bytes| STANDARD.encode(bytes))
        .map_err(|_| Some(Failure::InvalidRecord))?;
    Ok((admission, request_context))
}

async fn transfer(
    pipe: &mut Stdio,
    request_context: Option<RequestMetaObject>,
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
                let mut message = message?;
                if let (Some(context), ClientJsonRpcMessage::Request(request)) =
                    (&request_context, &mut message)
                {
                    request.request.get_meta_mut().extend(context.clone());
                }
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

#[cfg(test)]
mod launch_tests {
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    use super::{client_arguments, client_program};

    #[test]
    fn client_accepts_named_shell_options_and_url_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().canonicalize().unwrap();
        let root = rustx_confined_fs::PackageDirectory::open(&workspace).unwrap();
        let script = workspace.join("server.sh");
        std::fs::write(&script, "exit 0").unwrap();
        let args = vec![
            "-O".into(),
            "extglob".into(),
            script.into_os_string().into_string().unwrap(),
        ];
        assert_eq!(
            client_arguments(&root, &workspace, Path::new("/bin/bash"), args.clone()),
            Some(args)
        );
        let urls = vec![
            "--input".into(),
            "https://example.test/feed".into(),
            "--input=https://example.test/two".into(),
        ];
        assert_eq!(
            client_arguments(&root, &workspace, Path::new("feed"), urls.clone()),
            Some(urls)
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One Client-side path and interpreter admission matrix.
    fn path_valued_programs_stay_in_the_client_workspace() {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let workspace = base.join("workspace");
        let commands = workspace.join("commands");
        std::fs::create_dir_all(&commands).unwrap();
        let executable = commands.join("peer");
        std::fs::write(&executable, "authorized").unwrap();
        std::fs::write(base.join("external"), "outside").unwrap();
        symlink("peer", commands.join("linked")).unwrap();
        symlink("../../external", commands.join("outside")).unwrap();
        let root = rustx_confined_fs::PackageDirectory::open(&workspace).unwrap();
        let resolve =
            |name: PathBuf| client_program(&root, &workspace, Path::new("commands"), name);

        assert_eq!(resolve("./linked".into()), Some(executable.clone()));
        assert_eq!(resolve(commands.join("linked")), Some(executable));
        assert_eq!(resolve("./outside".into()), None);
        assert_eq!(resolve(commands.join("outside")), None);
        assert_eq!(resolve("../../external".into()), None);
        assert_eq!(resolve(workspace.join("../external")), None);
        assert_eq!(resolve("/bin/sh".into()), Some("/bin/sh".into()));
        assert_eq!(resolve("sh".into()), Some("sh".into()));
        assert_eq!(resolve(base.join("missing")), None);
        let args = vec![format!("--config={}", commands.join("peer").display())];
        assert_eq!(
            client_arguments(&root, &workspace, Path::new("fixture"), args.clone()),
            Some(args)
        );
        assert!(
            client_arguments(
                &root,
                &workspace,
                Path::new("python3"),
                vec![base.join("missing.py").display().to_string()]
            )
            .is_none()
        );
        assert!(
            client_arguments(
                &root,
                &workspace,
                Path::new("fixture"),
                vec![base.join("missing.py").display().to_string()]
            )
            .is_some()
        );
        assert!(
            client_arguments(
                &root,
                &workspace,
                Path::new("fixture"),
                vec!["./relative.py".into()]
            )
            .is_none()
        );
        assert!(
            client_arguments(
                &root,
                &workspace,
                Path::new("/bin/sh"),
                vec!["-c".into(), "printf 'x/y'".into()]
            )
            .is_some()
        );
        for program in ["/bin/sh", "/usr/bin/env"] {
            for flag in ["-lc", "-ic"] {
                let args = if program.ends_with("env") {
                    vec!["bash".into(), flag.into(), "exec /some/server.py".into()]
                } else {
                    vec![flag.into(), "exec /some/server.py".into()]
                };
                assert_eq!(
                    client_arguments(&root, &workspace, Path::new(program), args.clone()),
                    Some(args)
                );
            }
        }
        for path in [commands.join("new.log"), base.join("new.log")] {
            let args = vec![format!("--log-file={}", path.display())];
            assert_eq!(
                client_arguments(&root, &workspace, Path::new("fixture"), args.clone()),
                Some(args)
            );
        }
        let missing_parent = base.join("absent/new.log");
        assert!(
            client_arguments(
                &root,
                &workspace,
                Path::new("fixture"),
                vec![format!("--log-file={}", missing_parent.display())]
            )
            .is_none()
        );
        assert!(
            client_arguments(
                &root,
                &workspace,
                Path::new("fixture"),
                vec![format!("--config={}", base.join("missing.toml").display())]
            )
            .is_none()
        );
        assert!(
            client_arguments(
                &root,
                &workspace,
                Path::new("fixture"),
                vec!["--input".into(), base.join("missing").display().to_string()]
            )
            .is_none()
        );
        assert!(
            client_arguments(
                &root,
                &workspace,
                Path::new("python3"),
                vec!["-m".into(), "package.server".into()]
            )
            .is_some()
        );
        assert!(
            client_arguments(
                &root,
                &workspace,
                Path::new("/usr/bin/env"),
                vec!["python3".into(), "-c".into(), "print('x/y')".into()]
            )
            .is_some()
        );
    }
}
