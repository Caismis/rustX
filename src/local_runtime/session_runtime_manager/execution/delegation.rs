//! Routes a staged Server child through its parent's existing Session binding.

use super::{
    EndpointAuthority, ExecutionRoute, LocalExecutionEndpoint, SessionExecutionScope, SessionId,
};
use rustx_execution_protocol::process::{
    BashCaptureState, MAX_GENERATION_TOMBSTONES, MAX_OPERATIONS_PER_SESSION, OperationId,
    OperationState, ProcessOperationKey, ProcessProgram, ProcessReply, ProcessRequest,
};
use rustx_execution_protocol::transport::{
    LOCAL_HANDSHAKE_TIMEOUT, PAGE_REQUEST_TIMEOUT, encode_record,
};
use rustx_execution_protocol::{
    Admission, Capability, ClientReply, ClientRequest, ConnectionId, ControlReply, ControlRequest,
    DecimalU64, MAX_DATA_RECORDS_PER_CONNECTION, ProcessCancelDisposition,
};
use rustx_execution_server::LocalConnection;
use rustx_execution_server::delegation::{ControlReceipt, DataReply, DataRequest, Refusal};
use rustx_execution_transport::{ChannelShutdown, RecordChannel};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// The existing physical admission transferred alongside the two IPC channels.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChildExecutionGrant {
    admission: Admission,
    connection: ConnectionId,
    capabilities: BTreeSet<Capability>,
}

impl ChildExecutionGrant {
    pub(crate) async fn open(
        self,
        session: &SessionId,
        workspace: &Path,
        startup: &tokio::net::UnixStream,
    ) -> io::Result<SessionExecutionScope> {
        if self.admission.session.as_str() != session.as_str()
            || self.admission.workspace != workspace
            || self.admission.version != rustx_execution_protocol::PROTOCOL_VERSION
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let [data, control] = descriptor::receive(startup).await?;
        let shutdown = RecordChannel::unix(data.try_clone()?).shutdown_handle()?;
        let connection = LocalConnection::delegated(
            self.admission.clone(),
            self.connection,
            self.capabilities,
            data,
            control,
        )?;
        let control = connection.control().expect("delegated control lane");
        let endpoint = LocalExecutionEndpoint {
            authority: EndpointAuthority::Delegated { control, shutdown },
            connection: Mutex::new(Some(connection)),
            data_slots: Arc::new(Semaphore::new(MAX_DATA_RECORDS_PER_CONNECTION)),
            agent_writer: Arc::new(Semaphore::new(1)),
            admission: std::sync::OnceLock::from(self.admission),
            workspace: workspace.to_path_buf(),
        };
        Ok(SessionExecutionScope {
            session: session.clone(),
            route: ExecutionRoute::Delegated(Arc::new(endpoint)),
        })
    }
}

pub(crate) struct ChildExecutionRelay {
    grant: ChildExecutionGrant,
    child_data: UnixStream,
    child_control: UnixStream,
    parent_data: UnixStream,
    parent_control: UnixStream,
    owner: Arc<LocalExecutionEndpoint>,
    authority: Arc<Authority>,
}

pub(crate) struct ChildExecutionSettlement {
    stop: rustx_process_supervisor::signal::ProcessSignal,
    data: Arc<ChannelShutdown>,
    control: Arc<ChannelShutdown>,
    receipt: tokio::sync::oneshot::Receiver<io::Result<()>>,
}

impl std::fmt::Debug for ChildExecutionSettlement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildExecutionSettlement")
            .finish_non_exhaustive()
    }
}

impl ChildExecutionSettlement {
    pub(crate) async fn after_child_reaped(self) -> io::Result<()> {
        self.stop.cancel();
        self.data.shutdown();
        self.control.shutdown();
        self.receipt.await.map_err(io::Error::other)?
    }
}

struct Authority {
    conversation: String,
    tools: BTreeSet<String>,
    operations: Mutex<BTreeMap<OperationId, ChildOperation>>,
    tickets: Mutex<BTreeSet<DecimalU64>>,
    stop: rustx_process_supervisor::signal::ProcessSignal,
}

struct ChildOperation {
    key: ProcessOperationKey,
    journal_released: bool,
    capture_disposed: bool,
}

impl ChildExecutionRelay {
    pub(crate) fn create(
        scope: &SessionExecutionScope,
        workspace: &Path,
        conversation: &crate::runtime::identity::ConversationId,
        tools: &[crate::runtime::subagent::ResolvedSubagentTool],
    ) -> io::Result<Self> {
        let owner = scope.endpoint()?;
        if workspace != owner.workspace() {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let (parent_data, child_data) = UnixStream::pair()?;
        let (parent_control, child_control) = UnixStream::pair()?;
        let grant = owner.with_connection(None, true, |connection| {
            Ok(ChildExecutionGrant {
                admission: connection.admission().clone(),
                connection: connection.connection_id().clone(),
                capabilities: connection.capabilities().clone(),
            })
        })?;
        let authority = Arc::new(Authority {
            conversation: conversation.as_str().to_owned(),
            tools: tools
                .iter()
                .filter_map(|tool| match tool {
                    crate::runtime::subagent::ResolvedSubagentTool::Builtin { .. } => {
                        Some(tool.definition().name.clone())
                    }
                    crate::runtime::subagent::ResolvedSubagentTool::Source { .. } => None,
                })
                .collect(),
            operations: Mutex::default(),
            tickets: Mutex::default(),
            stop: rustx_process_supervisor::signal::ProcessSignal::new(),
        });
        Ok(Self {
            grant,
            child_data,
            child_control,
            parent_data,
            parent_control,
            owner,
            authority,
        })
    }

    pub(crate) fn grant(&self) -> ChildExecutionGrant {
        self.grant.clone()
    }

    pub(crate) async fn transfer(&self, startup: &tokio::net::UnixStream) -> io::Result<()> {
        descriptor::send(startup, &self.child_data, &self.child_control).await
    }

    pub(crate) fn start(self) -> io::Result<ChildExecutionSettlement> {
        let Self {
            parent_data,
            parent_control,
            owner,
            authority,
            ..
        } = self;
        let parent_data = RecordChannel::unix(parent_data);
        let parent_control = RecordChannel::unix(parent_control);
        let data_stop = Arc::new(parent_data.shutdown_handle()?);
        let control_stop = Arc::new(parent_control.shutdown_handle()?);
        let control_to_data = Arc::clone(&data_stop);
        let data_to_control = Arc::clone(&control_stop);
        let stop = authority.stop.clone();
        let control_owner = Arc::clone(&owner);
        let control_authority = Arc::clone(&authority);
        let control_worker = std::thread::Builder::new()
            .name("child-execution-control-relay".into())
            .spawn(move || {
                run_control(
                    parent_control,
                    &control_owner,
                    &control_authority,
                    &control_to_data,
                );
            })?;
        let (finished, settlement) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("child-execution-data-relay".into())
            .spawn(move || {
                run_data(parent_data, &owner, &authority);
                authority.stop.cancel();
                data_to_control.shutdown();
                let result = authority.release(&owner);
                let control_result = control_worker
                    .join()
                    .map_err(|_| io::Error::other("child control relay stopped without proof"));
                let _ = finished.send(result.and(control_result));
            })?;
        Ok(ChildExecutionSettlement {
            stop,
            data: data_stop,
            control: control_stop,
            receipt: settlement,
        })
    }
}

impl Authority {
    fn permits(&self, endpoint: &LocalExecutionEndpoint, request: &ClientRequest) -> bool {
        let tool = |name| self.tools.contains(name);
        match request {
            ClientRequest::ReadLines { .. } | ClientRequest::ReadDocument { .. } => tool("read"),
            ClientRequest::ReadRange { .. } => tool("read_image"),
            ClientRequest::Search { spec, .. } => match spec {
                rustx_execution_protocol::SearchSpec::Glob { .. } => tool("glob"),
                rustx_execution_protocol::SearchSpec::Grep { .. } => tool("grep"),
            },
            ClientRequest::GetMetadata { .. } | ClientRequest::ReadDirectory { .. } => {
                tool("read") || tool("glob") || tool("grep") || tool("write") || tool("edit")
            }
            ClientRequest::PrepareWrite { .. } | ClientRequest::BeginWrite { .. } => tool("write"),
            ClientRequest::PrepareEdit { .. } => tool("edit"),
            ClientRequest::WriteChunk { ticket, .. }
            | ClientRequest::CommitWrite { ticket }
            | ClientRequest::AbortWrite { ticket } => self.owns_ticket(*ticket),
            ClientRequest::Process(ProcessRequest::RegisterAgentBash { key, spec, .. }) => {
                tool("bash")
                    && matches!(&spec.program, ProcessProgram::Shell { conversation, .. } if *conversation == self.conversation)
                    && endpoint
                        .process_key(key.operation)
                        .is_ok_and(|current| current == *key)
                    && {
                        let operations = self.operations.lock().expect("child process keys");
                        operations.contains_key(&key.operation)
                            || operations.len()
                                < MAX_GENERATION_TOMBSTONES + MAX_OPERATIONS_PER_SESSION
                    }
            }
            ClientRequest::Process(ProcessRequest::Register { .. }) => false,
            ClientRequest::Process(request) => self.owns_process(request.key()),
            ClientRequest::ReadBashOutput {
                key, conversation, ..
            } => tool("read") && *conversation == self.conversation && self.owns_process(key),
            ClientRequest::SearchBashOutput {
                key, conversation, ..
            } => tool("grep") && *conversation == self.conversation && self.owns_process(key),
            _ => false,
        }
    }

    fn owns_process(&self, key: &ProcessOperationKey) -> bool {
        self.operations
            .lock()
            .expect("child process keys")
            .get(&key.operation)
            .is_some_and(|operation| operation.key == *key)
    }

    fn owns_ticket(&self, ticket: DecimalU64) -> bool {
        self.tickets
            .lock()
            .expect("child mutation tickets")
            .contains(&ticket)
    }

    fn retain_reply(&self, request: &ClientRequest, reply: &ClientReply) {
        match (request, reply) {
            (
                ClientRequest::Process(ProcessRequest::RegisterAgentBash { key, .. }),
                ClientReply::Process(ProcessReply::Registered { key: echoed }),
            ) if key == echoed => {
                self.operations
                    .lock()
                    .expect("child process keys")
                    .entry(key.operation)
                    .or_insert_with(|| ChildOperation {
                        key: key.clone(),
                        journal_released: false,
                        capture_disposed: false,
                    });
            }
            (
                ClientRequest::Process(
                    request @ (ProcessRequest::ReleaseOutput { key, .. }
                    | ProcessRequest::DiscardBashOutput { key }),
                ),
                ClientReply::Process(ProcessReply::OutputReleased { key: echoed }),
            ) if key == echoed => {
                let mut operations = self.operations.lock().expect("child process keys");
                if let Some(operation) = operations.get_mut(&key.operation) {
                    operation.journal_released = true;
                    operation.capture_disposed |=
                        matches!(request, ProcessRequest::DiscardBashOutput { .. });
                }
            }
            (
                ClientRequest::Process(ProcessRequest::BashCapture { key }),
                ClientReply::Process(ProcessReply::BashCapture {
                    key: echoed,
                    capture: Some(capture),
                }),
            ) if key == echoed
                && matches!(
                    capture.output,
                    BashCaptureState::None | BashCaptureState::Unavailable
                ) =>
            {
                if let Some(operation) = self
                    .operations
                    .lock()
                    .expect("child process keys")
                    .get_mut(&key.operation)
                {
                    operation.capture_disposed = true;
                }
            }
            (
                ClientRequest::PrepareWrite { .. } | ClientRequest::BeginWrite { .. },
                ClientReply::WritePrepared { ticket, .. },
            )
            | (ClientRequest::PrepareEdit { .. }, ClientReply::EditPrepared { ticket, .. }) => {
                self.tickets
                    .lock()
                    .expect("child mutation tickets")
                    .insert(*ticket);
            }
            (ClientRequest::CommitWrite { ticket } | ClientRequest::AbortWrite { ticket }, _) => {
                self.tickets
                    .lock()
                    .expect("child mutation tickets")
                    .remove(ticket);
            }
            _ => {}
        }
    }

    fn release(&self, owner: &LocalExecutionEndpoint) -> io::Result<()> {
        let tickets = std::mem::take(&mut *self.tickets.lock().expect("child mutation tickets"));
        let operations = std::mem::take(&mut *self.operations.lock().expect("child process keys"));
        let deadline =
            Instant::now() + rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT;
        for ticket in tickets {
            let reply = owner.with_connection(None, true, |connection| {
                connection.forward_delegated(
                    &ClientRequest::AbortWrite { ticket },
                    deadline,
                    || Ok(()),
                )
            });
            if !matches!(reply, Ok(ClientReply::WriteAborted { ticket: echoed }) if ticket == echoed)
            {
                owner.cut_pair();
                return Err(io::Error::other(
                    "child execution cleanup remains unconfirmed",
                ));
            }
        }
        for operation in operations.into_values() {
            if Self::release_operation(owner, &operation, deadline).is_err() {
                owner.cut_pair();
                return Err(io::Error::other(
                    "child execution cleanup remains unconfirmed",
                ));
            }
        }
        Ok(())
    }

    fn release_operation(
        owner: &LocalExecutionEndpoint,
        operation: &ChildOperation,
        deadline: Instant,
    ) -> io::Result<()> {
        if !operation.journal_released {
            Self::release_journal(owner, &operation.key, deadline)?;
        }
        if !operation.capture_disposed {
            let key = &operation.key;
            let discarded = owner.with_connection(None, true, |connection| {
                connection.process_until(
                    &ProcessRequest::DiscardBashOutput { key: key.clone() },
                    deadline,
                )
            })?;
            if !matches!(discarded, ProcessReply::OutputReleased { key: echoed } if echoed == *key)
            {
                return Err(io::ErrorKind::InvalidData.into());
            }
        }
        Ok(())
    }

    fn release_journal(
        owner: &LocalExecutionEndpoint,
        key: &ProcessOperationKey,
        deadline: Instant,
    ) -> io::Result<()> {
        let exchange = |request| {
            owner.with_connection(None, true, |connection| {
                connection.process_until(&request, deadline)
            })
        };
        let mut state = exchange(ProcessRequest::Query { key: key.clone() })?;
        if !matches!(
            state,
            ProcessReply::State {
                state: OperationState::Settled(_),
                ..
            }
        ) {
            let control = owner.control().ok_or(io::ErrorKind::NotConnected)?;
            if control.cancel_process_by(key, deadline)? == ProcessCancelDisposition::Unknown {
                return Err(io::ErrorKind::NotConnected.into());
            }
            loop {
                state = exchange(ProcessRequest::Query { key: key.clone() })?;
                if matches!(
                    state,
                    ProcessReply::State {
                        state: OperationState::Settled(_),
                        ..
                    }
                ) {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let ProcessReply::State {
            state: OperationState::Settled(snapshot),
            ..
        } = state
        else {
            unreachable!();
        };
        let released = exchange(ProcessRequest::ReleaseOutput {
            key: key.clone(),
            final_sequence: snapshot.final_sequence,
        })?;
        if !matches!(released, ProcessReply::OutputReleased { key: echoed } if echoed == *key) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(())
    }
}

fn run_data(mut channel: RecordChannel, owner: &LocalExecutionEndpoint, authority: &Authority) {
    while let Ok(Some(DataRequest {
        request,
        remaining_ms,
    })) = channel.receive_optional(None)
    {
        let budget = Duration::from_millis(u64::from(remaining_ms)).min(PAGE_REQUEST_TIMEOUT);
        let deadline = Instant::now() + budget;
        let reply = if authority.permits(owner, &request) {
            let result = owner.with_connection(None, true, |connection| {
                connection.forward_delegated(&request, deadline, || {
                    if Instant::now() >= deadline {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    authority
                        .stop
                        .with_active(|| ())
                        .ok_or(io::ErrorKind::Interrupted.into())
                })
            });
            match result {
                Ok(reply) => {
                    authority.retain_reply(&request, &reply);
                    DataReply::Client(reply)
                }
                Err(error) => DataReply::Rejected(error.into()),
            }
        } else {
            DataReply::Rejected(Refusal::Denied)
        };
        if encode_record(&reply)
            .and_then(|encoded| channel.send(&encoded, Some(deadline)))
            .is_err()
        {
            break;
        }
    }
}

fn run_control(
    mut channel: RecordChannel,
    owner: &LocalExecutionEndpoint,
    authority: &Authority,
    data_stop: &ChannelShutdown,
) {
    while let Ok(Some(request)) = channel.receive_optional::<ControlRequest>(None) {
        let result = owner
            .control()
            .ok_or(io::ErrorKind::NotConnected.into())
            .and_then(|control| match request {
                ControlRequest::Health { nonce } => {
                    control.health().map(|()| ControlReply::Healthy { nonce })
                }
                ControlRequest::CancelMutation { ticket } if authority.owns_ticket(ticket) => {
                    control.cancel_mutation(ticket).map(|disposition| {
                        ControlReply::MutationCancelled {
                            ticket,
                            disposition,
                        }
                    })
                }
                ControlRequest::CancelProcess { key } if authority.owns_process(&key) => control
                    .cancel_process(&key)
                    .map(|disposition| ControlReply::ProcessCancelled { key, disposition }),
                _ => Err(io::ErrorKind::PermissionDenied.into()),
            });
        let reply = match result {
            Ok(reply) => ControlReceipt::Client(reply),
            Err(error) => ControlReceipt::Rejected(error.into()),
        };
        let deadline = Instant::now() + rustx_execution_protocol::transport::CONTROL_FRAME_TIMEOUT;
        if encode_record(&reply)
            .and_then(|encoded| channel.send(&encoded, Some(deadline)))
            .is_err()
        {
            break;
        }
    }
    authority.stop.cancel();
    data_stop.shutdown();
}

mod descriptor {
    use nix::sys::socket::{ControlMessage, ControlMessageOwned, MsgFlags, recvmsg, sendmsg};
    use std::io::{self, IoSlice, IoSliceMut};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::net::UnixStream;
    use tokio::io::Interest;

    const TRANSFER: u8 = 1;

    pub(super) async fn send(
        stream: &tokio::net::UnixStream,
        data: &UnixStream,
        control: &UnixStream,
    ) -> io::Result<()> {
        let fds = [data.as_raw_fd(), control.as_raw_fd()];
        tokio::time::timeout(
            super::LOCAL_HANDSHAKE_TIMEOUT,
            stream.async_io(Interest::WRITABLE, || {
                let bytes = [TRANSFER];
                let iov = [IoSlice::new(&bytes)];
                let ancillary = [ControlMessage::ScmRights(&fds)];
                let size = sendmsg::<()>(
                    stream.as_raw_fd(),
                    &iov,
                    &ancillary,
                    MsgFlags::MSG_DONTWAIT,
                    None,
                )?;
                if size != bytes.len() {
                    return Err(io::ErrorKind::WriteZero.into());
                }
                Ok(())
            }),
        )
        .await
        .map_err(|_| io::ErrorKind::TimedOut)?
    }

    pub(super) async fn receive(stream: &tokio::net::UnixStream) -> io::Result<[UnixStream; 2]> {
        tokio::time::timeout(
            super::LOCAL_HANDSHAKE_TIMEOUT,
            stream.async_io(Interest::READABLE, || {
                let mut bytes = [0u8];
                let mut iov = [IoSliceMut::new(&mut bytes)];
                let mut ancillary = nix::cmsg_space!([RawFd; 256]);
                let flags = MsgFlags::MSG_DONTWAIT;
                #[cfg(target_os = "linux")]
                let flags = flags | MsgFlags::MSG_CMSG_CLOEXEC;
                let message =
                    recvmsg::<()>(stream.as_raw_fd(), &mut iov, Some(&mut ancillary), flags)?;
                let mut fds = Vec::new();
                let valid = message.bytes == 1
                    && !message
                        .flags
                        .intersects(MsgFlags::MSG_CTRUNC | MsgFlags::MSG_TRUNC);
                for control in message.cmsgs()? {
                    if let ControlMessageOwned::ScmRights(received) = control {
                        fds.extend(received.into_iter().map(take));
                    }
                }
                if !valid || bytes != [TRANSFER] || fds.len() != 2 {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                for fd in &fds {
                    use nix::fcntl::{FcntlArg, FdFlag, fcntl};
                    fcntl(fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
                    nix::sys::socket::getpeername::<nix::sys::socket::UnixAddr>(fd.as_raw_fd())?;
                }
                let [data, control] = fds.try_into().map_err(|_| io::ErrorKind::InvalidData)?;
                Ok([data.into(), control.into()])
            }),
        )
        .await
        .map_err(|_| io::ErrorKind::TimedOut)?
    }

    #[allow(unsafe_code)]
    fn take(fd: RawFd) -> OwnedFd {
        // SAFETY: recvmsg(SCM_RIGHTS) returned a newly allocated, uniquely owned
        // descriptor. Adopt every received fd, including on a malformed transfer.
        unsafe { OwnedFd::from_raw_fd(fd) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustx_execution_protocol::process::{BashCaptureResult, ServerIncarnation};
    use rustx_execution_protocol::{ClientInstanceId, Generation};

    #[tokio::test]
    async fn reaping_retires_both_relays_even_when_their_peers_are_open() {
        let (data, data_peer) = UnixStream::pair().unwrap();
        let (control, control_peer) = UnixStream::pair().unwrap();
        let mut data = RecordChannel::unix(data);
        let mut control = RecordChannel::unix(control);
        let data_stop = Arc::new(data.shutdown_handle().unwrap());
        let control_stop = Arc::new(control.shutdown_handle().unwrap());
        let stop = rustx_process_supervisor::signal::ProcessSignal::new();
        let witness = stop.clone();
        let (settled, receipt) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || {
            assert!(
                data.receive_optional::<DataRequest>(None)
                    .unwrap()
                    .is_none()
            );
            assert!(
                control
                    .receive_optional::<ControlRequest>(None)
                    .unwrap()
                    .is_none()
            );
            assert!(witness.is_cancelled());
            settled.send(Ok(())).unwrap();
        });
        tokio::time::timeout(LOCAL_HANDSHAKE_TIMEOUT, async {
            ChildExecutionSettlement {
                stop,
                data: data_stop,
                control: control_stop,
                receipt,
            }
            .after_child_reaped()
            .await
            .unwrap();
        })
        .await
        .unwrap();
        worker.join().unwrap();
        drop((data_peer, control_peer));
    }

    #[test]
    fn child_drain_uses_the_physical_output_and_capture_receipts() {
        let key = ProcessOperationKey {
            incarnation: ServerIncarnation::generate(),
            session: rustx_execution_protocol::SessionId::new("session"),
            instance: ClientInstanceId::new("client"),
            generation: Generation::new(std::num::NonZeroU64::MIN),
            operation: OperationId::new(),
        };
        let authority = Authority {
            conversation: "child".into(),
            tools: BTreeSet::from(["bash".into()]),
            operations: Mutex::new(BTreeMap::from([(
                key.operation,
                ChildOperation {
                    key: key.clone(),
                    journal_released: false,
                    capture_disposed: false,
                },
            )])),
            tickets: Mutex::default(),
            stop: rustx_process_supervisor::signal::ProcessSignal::new(),
        };
        let command = ClientRequest::Process;
        let receipt = ClientReply::Process;
        let output_released = receipt(ProcessReply::OutputReleased { key: key.clone() });
        let state = || {
            let operations = authority.operations.lock().unwrap();
            let operation = &operations[&key.operation];
            (operation.journal_released, operation.capture_disposed)
        };
        authority.retain_reply(
            &command(ProcessRequest::BashCapture { key: key.clone() }),
            &receipt(ProcessReply::BashCapture {
                key: key.clone(),
                capture: None,
            }),
        );
        assert_eq!(state(), (false, false));
        authority.retain_reply(
            &command(ProcessRequest::BashCapture { key: key.clone() }),
            &receipt(ProcessReply::BashCapture {
                key: key.clone(),
                capture: Some(BashCaptureResult {
                    stdout: String::new(),
                    stderr: String::new(),
                    combined: String::new(),
                    truncated: false,
                    total: Some(0.into()),
                    output: BashCaptureState::None,
                    started: true,
                    signal: None,
                }),
            }),
        );
        assert_eq!(state(), (false, true));
        authority.retain_reply(
            &command(ProcessRequest::ReleaseOutput {
                key: key.clone(),
                final_sequence: 0.into(),
            }),
            &output_released,
        );
        assert_eq!(state(), (true, true));
        assert!(authority.owns_process(&key));
    }
}
