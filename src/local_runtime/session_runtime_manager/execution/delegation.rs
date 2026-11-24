//! Routes a staged Server child through its parent's existing Session binding.

use super::{
    EndpointAuthority, ExecutionRoute, LocalExecutionEndpoint, SessionExecutionScope, SessionId,
};
use crate::runtime::subagent::{ResolvedSubagentSpec, ResolvedSubagentTool};
use base64::Engine as _;
use rustx_execution_protocol::mcp::{
    Launch as McpLaunch, Reply as McpReply, Request as McpRequest,
};
use rustx_execution_protocol::process::{
    BashCaptureState, MAX_PROCESS_IDENTITIES_PER_GENERATION, OperationId, OperationState,
    ProcessOperationKey, ProcessProgram, ProcessReply, ProcessRequest,
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
use std::cell::Cell;
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
        let stdio_mcp = self.capabilities.contains(&Capability::StdioMcp);
        let connection = LocalConnection::delegated(
            self.admission.clone(),
            self.connection,
            self.capabilities,
            data,
            control,
        )?;
        let control = connection.control().expect("delegated control lane");
        let work = connection
            .delegated_work()
            .expect("delegated dispatch witness");
        let endpoint = LocalExecutionEndpoint {
            authority: EndpointAuthority::Delegated {
                control,
                shutdown,
                work,
            },
            connection: Mutex::new(Some(connection)),
            data_slots: Arc::new(Semaphore::new(MAX_DATA_RECORDS_PER_CONNECTION)),
            connection_released: tokio::sync::Notify::new(),
            accept_stop: tokio_util::sync::CancellationToken::new(),
            settlement: Mutex::new(super::Settlement::Open),
            settlement_changed: std::sync::Condvar::new(),
            #[cfg(test)]
            settlement_joined: tokio::sync::Notify::new(),
            #[cfg(test)]
            test_hooks: Mutex::default(),
            agent_writer: Arc::new(Semaphore::new(1)),
            admission: std::sync::OnceLock::from(self.admission),
            managed_output: std::sync::OnceLock::from(false),
            stdio_mcp: std::sync::OnceLock::from(stdio_mcp),
            recovery_fence: None,
            workspace: workspace.to_path_buf(),
        };
        Ok(SessionExecutionScope {
            session: session.clone(),
            route: ExecutionRoute::Fixed(Arc::new(endpoint)),
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
    tickets: Mutex<BTreeMap<DecimalU64, u64>>,
    mcp_sources: Vec<ChildMcpSource>,
    mcp_operations: Mutex<BTreeMap<OperationId, ChildMcpOperation>>,
    stop: rustx_process_supervisor::signal::ProcessSignal,
}

struct ChildMcpSource {
    id: String,
    launch: McpLaunch,
    tools: BTreeSet<String>,
    resources: bool,
}

struct ChildMcpOperation {
    key: ProcessOperationKey,
    physical: ProcessOperationKey,
    source: usize,
    retained: bool,
}

struct ChildOperation {
    key: ProcessOperationKey,
    physically_settled: bool,
    journal_released: bool,
    capture_disposed: bool,
}

impl ChildExecutionRelay {
    pub(crate) fn create(
        scope: &SessionExecutionScope,
        workspace: &Path,
        conversation: &crate::runtime::identity::ConversationId,
        tools: &[ResolvedSubagentTool],
        resolved: Option<&ResolvedSubagentSpec>,
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
                    ResolvedSubagentTool::Builtin { .. } => Some(tool.definition().name.clone()),
                    ResolvedSubagentTool::Source { .. } => None,
                })
                .collect(),
            operations: Mutex::default(),
            tickets: Mutex::default(),
            mcp_sources: resolved
                .map_or_else(Vec::new, |resolved| Self::mcp_sources(resolved, workspace)),
            mcp_operations: Mutex::default(),
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

    fn mcp_sources(resolved: &ResolvedSubagentSpec, workspace: &Path) -> Vec<ChildMcpSource> {
        resolved
            .materialization
            .sources
            .iter()
            .filter_map(|(id, binding)| {
                let server = crate::tools::mcp::source_server_id(id);
                let launch = crate::tools::mcp::stdio_launch_spec(binding, &server, workspace)
                    .ok()
                    .flatten()?;
                let tools = resolved
                    .tools
                    .iter()
                    .filter_map(|tool| match tool {
                        ResolvedSubagentTool::Source {
                            source_id, name, ..
                        } if source_id == id => Some(name.clone()),
                        _ => None,
                    })
                    .collect();
                Some(ChildMcpSource {
                    id: id.to_string(),
                    launch,
                    tools,
                    resources: resolved.resource_sources.contains(id),
                })
            })
            .collect()
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
        let runtime = tokio::runtime::Handle::current();
        std::thread::Builder::new()
            .name("child-execution-data-relay".into())
            .spawn(move || {
                let _runtime = runtime.enter();
                let unconfirmed = run_data(parent_data, &owner, &authority);
                authority.stop.cancel();
                data_to_control.shutdown();
                let result = authority.release(&owner, unconfirmed);
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
                            || operations.len() < MAX_PROCESS_IDENTITIES_PER_GENERATION
                    }
            }
            ClientRequest::Process(ProcessRequest::Register { .. }) => false,
            ClientRequest::Process(request) => self.owns_process(request.key()),
            ClientRequest::Mcp(request) => self.permits_mcp(endpoint, request),
            ClientRequest::ReadBashOutput {
                key, conversation, ..
            } => tool("read") && *conversation == self.conversation && self.owns_process(key),
            ClientRequest::SearchBashOutput {
                key, conversation, ..
            } => tool("grep") && *conversation == self.conversation && self.owns_process(key),
            _ => false,
        }
    }

    fn permits_mcp(&self, endpoint: &LocalExecutionEndpoint, request: &McpRequest) -> bool {
        let key = request.key();
        let operations = self.mcp_operations.lock().expect("child MCP identities");
        let existing = operations.get(&key.operation);
        if let McpRequest::Open {
            source_id, launch, ..
        } = request
        {
            return endpoint
                .process_key(key.operation)
                .is_ok_and(|current| current == *key)
                && self.mcp_source(source_id, launch).is_some_and(|source| {
                    existing.is_none_or(|op| op.key == *key && op.source == source)
                });
        }
        let Some(operation) = existing.filter(|operation| operation.key == *key) else {
            return false;
        };
        if let McpRequest::Send { data_base64, .. } = request {
            return self.permits_mcp_message(operation.source, data_base64);
        }
        true
    }

    fn mcp_source(&self, id: &str, launch: &McpLaunch) -> Option<usize> {
        self.mcp_sources
            .iter()
            .position(|source| source.id == id && source.launch == *launch)
    }

    fn mcp_physical_request(&self, request: &McpRequest) -> io::Result<(McpRequest, bool)> {
        let key = request.key();
        let mut operations = self.mcp_operations.lock().expect("child MCP identities");
        let existing = operations.get(&key.operation);
        let (physical, inserted) = if let McpRequest::Open {
            source_id, launch, ..
        } = request
        {
            let source = self
                .mcp_source(source_id, launch)
                .ok_or(io::ErrorKind::PermissionDenied)?;
            if let Some(operation) = existing {
                if operation.key != *key || operation.source != source {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                (operation.physical.clone(), false)
            } else {
                if operations.len() >= MAX_PROCESS_IDENTITIES_PER_GENERATION {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let mut physical = key.clone();
                physical.operation = OperationId::new();
                operations.insert(
                    key.operation,
                    ChildMcpOperation {
                        key: key.clone(),
                        physical: physical.clone(),
                        source,
                        retained: false,
                    },
                );
                (physical, true)
            }
        } else {
            (
                existing
                    .filter(|operation| operation.key == *key)
                    .ok_or(io::ErrorKind::PermissionDenied)?
                    .physical
                    .clone(),
                false,
            )
        };
        Ok((request.clone().with_key(physical), inserted))
    }

    fn permits_mcp_message(&self, source: usize, encoded: &str) -> bool {
        use rmcp::model::{ClientJsonRpcMessage, ClientRequest};
        if encoded.len() > rustx_execution_protocol::mcp::MAX_SEND_BYTES.div_ceil(3) * 4 {
            return false;
        }
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
            return false;
        };
        if bytes.len() > rustx_execution_protocol::mcp::MAX_SEND_BYTES {
            return false;
        }
        let Ok(message) = serde_json::from_slice::<ClientJsonRpcMessage>(&bytes) else {
            return false;
        };
        let ClientJsonRpcMessage::Request(message) = message else {
            return true;
        };
        match message.request {
            ClientRequest::CallToolRequest(call) => self.mcp_sources[source]
                .tools
                .contains(call.params.name.as_ref()),
            ClientRequest::ListResourcesRequest(_)
            | ClientRequest::ListResourceTemplatesRequest(_)
            | ClientRequest::ReadResourceRequest(_) => self.mcp_sources[source].resources,
            ClientRequest::ListToolsRequest(_)
            | ClientRequest::SubscriptionsListenRequest(_)
            | ClientRequest::PingRequest(_) => true,
            ClientRequest::GetTaskRequest(_)
            | ClientRequest::UpdateTaskRequest(_)
            | ClientRequest::CancelTaskRequest(_) => !self.mcp_sources[source].tools.is_empty(),
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
            .contains_key(&ticket)
    }

    fn commit_size(&self, request: &ClientRequest) -> Option<u64> {
        let ClientRequest::CommitWrite { ticket } = request else {
            return None;
        };
        self.tickets
            .lock()
            .expect("child mutation tickets")
            .get(ticket)
            .copied()
    }

    fn retain_reply(&self, request: &ClientRequest, reply: &ClientReply) -> io::Result<()> {
        match (request, reply) {
            (ClientRequest::Process(request), ClientReply::Process(reply)) => {
                self.retain_process_reply(request, reply);
            }
            (ClientRequest::Mcp(request), ClientReply::Mcp(response)) => {
                self.retain_mcp_reply(request, &response.reply);
            }
            (
                ClientRequest::PrepareWrite { data_base64, .. },
                ClientReply::WritePrepared { ticket, .. },
            ) => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data_base64)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                return self.retain_ticket(*ticket, bytes.len() as u64);
            }
            (
                ClientRequest::BeginWrite { total: size, .. },
                ClientReply::WritePrepared { ticket, .. },
            )
            | (
                ClientRequest::PrepareEdit { .. },
                ClientReply::EditPrepared {
                    ticket,
                    published_size: size,
                    ..
                },
            ) => {
                return self.retain_ticket(*ticket, size.get());
            }
            (
                ClientRequest::CommitWrite { ticket },
                ClientReply::Written { ticket: echoed, .. }
                | ClientReply::WriteCancelled { ticket: echoed },
            )
            | (
                ClientRequest::AbortWrite { ticket },
                ClientReply::WriteAborted { ticket: echoed },
            ) if ticket == echoed => {
                self.tickets
                    .lock()
                    .expect("child mutation tickets")
                    .remove(ticket);
            }
            _ => {}
        }
        Ok(())
    }

    fn retain_mcp_reply(&self, request: &McpRequest, reply: &McpReply) {
        let retained = match (request, reply) {
            (McpRequest::Open { .. }, McpReply::Opening | McpReply::Ready { .. }) => true,
            (McpRequest::Close { .. }, reply) if reply.proves_close() => false,
            _ => return,
        };
        self.mcp_operations
            .lock()
            .expect("child MCP identities")
            .get_mut(&request.key().operation)
            .expect("dispatch registered the exact identity")
            .retained = retained;
    }

    fn retain_ticket(&self, ticket: DecimalU64, size: u64) -> io::Result<()> {
        let mut tickets = self.tickets.lock().expect("child mutation tickets");
        let std::collections::btree_map::Entry::Vacant(entry) = tickets.entry(ticket) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate delegated mutation ticket",
            ));
        };
        entry.insert(size);
        Ok(())
    }

    fn retain_process_reply(&self, request: &ProcessRequest, reply: &ProcessReply) {
        match (request, reply) {
            (
                ProcessRequest::RegisterAgentBash { key, .. },
                reply @ (ProcessReply::Registered { key: echoed }
                | ProcessReply::State {
                    key: echoed,
                    state: OperationState::Settled(_),
                }),
            ) if key == echoed => {
                let mut operations = self.operations.lock().expect("child process keys");
                let operation = operations
                    .entry(key.operation)
                    .or_insert_with(|| ChildOperation {
                        key: key.clone(),
                        physically_settled: false,
                        journal_released: false,
                        capture_disposed: false,
                    });
                if let ProcessReply::State {
                    state: OperationState::Settled(snapshot),
                    ..
                } = reply
                {
                    operation.physically_settled |= snapshot.physical_settlement_proven;
                }
            }
            (
                ProcessRequest::BashCapture { key },
                ProcessReply::BashCapture {
                    key: echoed,
                    capture: Some(capture),
                },
            ) if key == echoed && capture.output == BashCaptureState::None => {
                // None proves no capture was allocated. Unavailable
                // describes a read outcome and may still retain a file.
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
                request,
                ProcessReply::State {
                    key,
                    state: OperationState::Settled(snapshot),
                },
            ) if request.key() == key && snapshot.physical_settlement_proven => {
                if let Some(operation) = self
                    .operations
                    .lock()
                    .expect("child process keys")
                    .get_mut(&key.operation)
                {
                    operation.physically_settled = true;
                }
            }
            (
                request @ (ProcessRequest::ReleaseOutput { key, .. }
                | ProcessRequest::DiscardBashOutput { key }),
                ProcessReply::OutputReleased { key: echoed },
            ) if key == echoed => {
                let mut operations = self.operations.lock().expect("child process keys");
                if let Some(operation) = operations.get_mut(&key.operation) {
                    operation.journal_released |=
                        matches!(request, ProcessRequest::ReleaseOutput { .. });
                    operation.capture_disposed |=
                        matches!(request, ProcessRequest::DiscardBashOutput { .. });
                }
            }
            _ => {}
        }
    }

    fn release(&self, owner: &LocalExecutionEndpoint, unconfirmed: bool) -> io::Result<()> {
        let deadline =
            Instant::now() + rustx_execution_protocol::transport::CONTROL_COMMAND_TIMEOUT;
        if unconfirmed {
            // A lost preparation may have no ticket to address. A lost commit
            // may already have consumed its ticket. Only binding settlement
            // covers those resources and any other work on the connection.
            return owner.finish_child_cleanup(deadline);
        }
        let tickets = std::mem::take(&mut *self.tickets.lock().expect("child mutation tickets"));
        let operations = std::mem::take(&mut *self.operations.lock().expect("child process keys"));
        let mcp = std::mem::take(&mut *self.mcp_operations.lock().expect("child MCP identities"));
        for ticket in tickets.into_keys() {
            let reply = owner.with_bounded_data_connection(deadline, |connection| {
                connection.forward_delegated(
                    &ClientRequest::AbortWrite { ticket },
                    deadline,
                    None,
                    || Ok(()),
                )
            });
            if !matches!(reply, Ok(ClientReply::WriteAborted { ticket: echoed }) if ticket == echoed)
            {
                return owner.finish_child_cleanup(deadline);
            }
        }
        for operation in operations.into_values() {
            if Self::release_operation(owner, &operation, deadline).is_err() {
                return owner.finish_child_cleanup(deadline);
            }
        }
        for operation in mcp.into_values().filter(|operation| operation.retained) {
            if Self::release_mcp(owner, &operation.physical, deadline).is_err() {
                return owner.finish_child_cleanup(deadline);
            }
        }
        Ok(())
    }

    fn release_mcp(
        owner: &LocalExecutionEndpoint,
        key: &ProcessOperationKey,
        deadline: Instant,
    ) -> io::Result<()> {
        loop {
            let reply = owner.with_bounded_data_connection(deadline, |connection| {
                connection.mcp_by(McpRequest::Close { key: key.clone() }, deadline, || Ok(()))
            })?;
            match reply {
                reply if reply.proves_close() => return Ok(()),
                McpReply::Closing if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    return Err(io::Error::other(
                        "child stdio session did not prove settlement",
                    ));
                }
            }
        }
    }

    fn release_operation(
        owner: &LocalExecutionEndpoint,
        operation: &ChildOperation,
        deadline: Instant,
    ) -> io::Result<()> {
        Self::settle_process(owner, operation, deadline)?;
        if !operation.capture_disposed {
            let key = &operation.key;
            let discarded = owner.with_bounded_data_connection(deadline, |connection| {
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

    fn settle_process(
        owner: &LocalExecutionEndpoint,
        operation: &ChildOperation,
        deadline: Instant,
    ) -> io::Result<()> {
        if operation.physically_settled && operation.journal_released {
            return Ok(());
        }
        let key = &operation.key;
        let exchange = |request| {
            owner.with_bounded_data_connection(deadline, |connection| {
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
        if !operation.journal_released {
            let released = exchange(ProcessRequest::ReleaseOutput {
                key: key.clone(),
                final_sequence: snapshot.final_sequence,
            })?;
            if !matches!(released, ProcessReply::OutputReleased { key: echoed } if echoed == *key) {
                return Err(io::ErrorKind::InvalidData.into());
            }
        }
        if !snapshot.physical_settlement_proven {
            return Err(io::Error::other(
                "the child process has no physical settlement proof",
            ));
        }
        Ok(())
    }
}

fn run_data(
    mut channel: RecordChannel,
    owner: &LocalExecutionEndpoint,
    authority: &Authority,
) -> bool {
    let mut unconfirmed = false;
    while let Ok(Some(DataRequest {
        request,
        remaining_ms,
    })) = channel.receive_optional(None)
    {
        let budget = Duration::from_millis(u64::from(remaining_ms)).min(PAGE_REQUEST_TIMEOUT);
        let deadline = Instant::now() + budget;
        let (reply, request_unconfirmed) = if authority.permits(owner, &request) {
            forward_data_request(owner, authority, &request, deadline)
        } else {
            (DataReply::Rejected(Refusal::Denied), false)
        };
        unconfirmed |= request_unconfirmed;
        if encode_record(&reply)
            .and_then(|encoded| {
                channel.send(
                    &encoded,
                    Some(deadline + rustx_execution_server::delegation::DATA_REPLY_BUDGET),
                )
            })
            .is_err()
        {
            break;
        }
    }
    unconfirmed
}

fn forward_data_request(
    owner: &LocalExecutionEndpoint,
    authority: &Authority,
    request: &ClientRequest,
    deadline: Instant,
) -> (DataReply, bool) {
    // Known operations and tickets already have an owner. The following
    // exchanges can create an unidentified resource or consume a ticket
    // without a confirmable settlement.
    let needs_receipt = matches!(
        request,
        ClientRequest::Process(ProcessRequest::RegisterAgentBash { .. })
            | ClientRequest::Mcp(McpRequest::Open { .. })
            | ClientRequest::PrepareWrite { .. }
            | ClientRequest::BeginWrite { .. }
            | ClientRequest::PrepareEdit { .. }
            | ClientRequest::CommitWrite { .. }
    );
    let crossed = Cell::new(false);
    let result = owner
        .with_bounded_data_connection(deadline, |connection| {
            Ok((|| {
                let mut inserted_mcp = None;
                let physical;
                let dispatched = if let ClientRequest::Mcp(command) = request {
                    let (mapped, inserted) = authority.mcp_physical_request(command)?;
                    if inserted {
                        inserted_mcp = Some(command.key().operation);
                    }
                    physical = ClientRequest::Mcp(mapped);
                    &physical
                } else {
                    request
                };
                let forward = connection.forward_delegated(
                    dispatched,
                    deadline,
                    authority.commit_size(request),
                    || {
                        if Instant::now() >= deadline {
                            return Err(io::ErrorKind::TimedOut.into());
                        }
                        authority
                            .stop
                            .with_active(|| crossed.set(true))
                            .ok_or(io::ErrorKind::Interrupted.into())
                    },
                );
                let mut reply = match forward {
                    Ok(reply) => reply,
                    Err(error) => {
                        if !crossed.get()
                            && let Some(operation) = inserted_mcp
                        {
                            authority
                                .mcp_operations
                                .lock()
                                .expect("child MCP identities")
                                .remove(&operation);
                        }
                        return Err(error);
                    }
                };
                if let (ClientRequest::Mcp(logical), ClientReply::Mcp(response)) =
                    (request, &mut reply)
                {
                    response.key = logical.key().clone();
                }
                if let Err(error) = authority.retain_reply(request, &reply) {
                    connection.disconnect();
                    return Err(error);
                }
                Ok(reply)
            })())
        })
        .and_then(std::convert::identity);
    match result {
        Ok(reply) => {
            let unconfirmed = needs_receipt
                && crossed.get()
                && matches!(
                    reply,
                    ClientReply::CommitUnknown { .. }
                        | ClientReply::Process(ProcessReply::Unknown { .. })
                );
            (DataReply::Client(reply), unconfirmed)
        }
        Err(error) => {
            // The send gate runs after encoding and admission. In particular,
            // a refused deadline/permit cannot leave debt.
            let unconfirmed = needs_receipt && crossed.get();
            let reply = if crossed.get() {
                DataReply::Unknown(error.into())
            } else {
                DataReply::Rejected(error.into())
            };
            (reply, unconfirmed)
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
    #[allow(clippy::too_many_lines)] // Verify both source grants and retain the same physical identities.
    fn child_stdio_uses_parent_identity_and_keeps_unconfirmed_cleanup() {
        let key = ProcessOperationKey {
            incarnation: ServerIncarnation::generate(),
            session: rustx_execution_protocol::SessionId::new("session"),
            instance: ClientInstanceId::new("client"),
            generation: Generation::new(std::num::NonZeroU64::MIN),
            operation: OperationId::new(),
        };
        let launch = McpLaunch {
            program: "/bin/cat".into(),
            args: Vec::new(),
            cwd: std::path::PathBuf::new(),
            environment: BTreeMap::new(),
            versions: vec!["2025-11-25".into()],
            legacy_version: None,
            python_source: None,
        };
        let authority = Authority {
            conversation: "child".into(),
            tools: BTreeSet::new(),
            operations: Mutex::default(),
            tickets: Mutex::default(),
            mcp_sources: vec![
                ChildMcpSource {
                    id: "a".into(),
                    launch: launch.clone(),
                    tools: BTreeSet::from(["inspect".into()]),
                    resources: false,
                },
                ChildMcpSource {
                    id: "b".into(),
                    launch: launch.clone(),
                    tools: BTreeSet::from(["mutate".into()]),
                    resources: true,
                },
            ],
            mcp_operations: Mutex::default(),
            stop: rustx_process_supervisor::signal::ProcessSignal::new(),
        };
        let open = McpRequest::Open {
            key: key.clone(),
            source_id: "a".into(),
            launch: launch.clone(),
        };
        let (physical, inserted) = authority.mcp_physical_request(&open).unwrap();
        assert!(inserted);
        assert_ne!(physical.key().operation, key.operation);
        assert_eq!(
            authority.mcp_physical_request(&open).unwrap(),
            (physical.clone(), false)
        );
        let mut changed = launch.clone();
        changed.args.push("different".into());
        let conflicting = McpRequest::Open {
            key: key.clone(),
            source_id: "a".into(),
            launch: changed,
        };
        assert_eq!(
            authority
                .mcp_physical_request(&conflicting)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );

        let close = McpRequest::Close { key: key.clone() };
        assert_eq!(
            authority.mcp_physical_request(&close).unwrap().0.key(),
            physical.key()
        );
        authority.retain_mcp_reply(&open, &McpReply::Opening);
        authority.retain_mcp_reply(
            &close,
            &McpReply::Failed(rustx_execution_protocol::mcp::Failure::SettlementUnconfirmed),
        );
        assert!(authority.mcp_operations.lock().unwrap()[&key.operation].retained);
        authority.retain_mcp_reply(&close, &McpReply::Closed);
        assert!(!authority.mcp_operations.lock().unwrap()[&key.operation].retained);
        assert_eq!(
            authority.mcp_physical_request(&open).unwrap(),
            (physical.clone(), false)
        );

        let message = |name: &str| {
            base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_vec(&serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": name}})).unwrap()
        )
        };
        let open_other = McpRequest::Open {
            key: key.clone(),
            source_id: "b".into(),
            launch,
        };
        assert_eq!(
            authority
                .mcp_physical_request(&open_other)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        let mut other_key = key.clone();
        other_key.operation = OperationId::new();
        let (other, inserted_other) = authority
            .mcp_physical_request(&open_other.with_key(other_key.clone()))
            .unwrap();
        assert!(inserted_other);
        assert_ne!(physical.key(), other.key());
        let resources = base64::engine::general_purpose::STANDARD
            .encode(br#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#);
        let operations = authority.mcp_operations.lock().unwrap();
        let a = operations[&key.operation].source;
        let b = operations[&other_key.operation].source;
        assert!(authority.permits_mcp_message(a, &message("inspect")));
        assert!(!authority.permits_mcp_message(a, &message("mutate")));
        assert!(!authority.permits_mcp_message(a, &resources));
        assert!(!authority.permits_mcp_message(b, &message("inspect")));
        assert!(authority.permits_mcp_message(b, &message("mutate")));
        assert!(authority.permits_mcp_message(b, &resources));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)] // Keep the cancelled Open and the subsequent real Open on one relay.
    async fn child_relay_reaps_its_running_stdio_session_on_the_original_client() {
        use crate::tools::mcp::{
            McpError, McpInvalidationState, McpServerBinding, McpServerRuntime, McpTransportConfig,
            OwnedConnect,
        };
        use tokio::io::AsyncReadExt;

        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let execution = crate::session_execution_support::ExecutionFixture::new(&root);
        let observer_socket = root.join("started.sock");
        let observer = tokio::net::UnixListener::bind(&observer_socket).unwrap();
        let server_code = r"
import json, socket, sys
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as proof:
    proof.connect(sys.argv[1])
    proof.sendall(b'started')
    for line in sys.stdin:
        message = json.loads(line)
        if message.get('method') == 'server/discover':
            result = {
                'resultType': 'complete', 'supportedVersions': ['2026-07-28'],
                'capabilities': {}, 'ttlMs': 0, 'cacheScope': 'private',
                '_meta': {'io.modelcontextprotocol/serverInfo': {'name': 'delegated', 'version': '1'}}
            }
        elif message.get('method') == 'ping':
            proof.sendall(b'ping')
            result = {}
        else:
            continue
        print(json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'result': result}), flush=True)
";
        let source = crate::runtime::identity::McpServerId::new("delegated-stdio");
        let authored = McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            transport: McpTransportConfig::Stdio {
                program: "/usr/bin/env".into(),
                args: vec![
                    "python3".into(),
                    "-c".into(),
                    server_code.into(),
                    observer_socket.to_string_lossy().into_owned(),
                ],
                cwd: None,
                environment: BTreeMap::from([(
                    "PATH".into(),
                    "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin".into(),
                )]),
            },
            policy: crate::tools::types::ToolInvocationPolicy::default(),
            permissions: None,
            tool_permissions: BTreeMap::new(),
        };
        let launch = crate::tools::mcp::stdio_launch_spec(&authored, &source, &root)
            .unwrap()
            .unwrap();
        let conversation = crate::runtime::identity::ConversationId::new(
            "conv_0410af39-84e2-7896-a1f1-06c4c338553d",
        );
        let mut relay =
            ChildExecutionRelay::create(&execution.scope, &root, &conversation, &[], None).unwrap();
        Arc::get_mut(&mut relay.authority)
            .unwrap()
            .mcp_sources
            .push(ChildMcpSource {
                id: source.to_string(),
                launch: launch.clone(),
                tools: BTreeSet::from(["inspect".into()]),
                resources: false,
            });
        let (parent, child) = tokio::net::UnixStream::pair().unwrap();
        relay.transfer(&parent).await.unwrap();
        let child = relay
            .grant()
            .open(&execution.scope.session, &root, &child)
            .await
            .unwrap();
        let cleanup = relay.start().unwrap();
        let child_workspace =
            crate::tools::workspace::Workspace::for_session(&root, child.clone()).unwrap();
        let writer = child
            .endpoint()
            .unwrap()
            .agent_writer
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let opening_signal = crate::runtime::CancellationSignal::new();
        let mut opening = Box::pin(McpServerRuntime::connect_owned(OwnedConnect::new(
            &source,
            &authored,
            &child_workspace,
            Arc::new(McpInvalidationState::new()),
            opening_signal.clone(),
        )));
        assert!(futures_util::poll!(&mut opening).is_pending());
        opening_signal.cancel();
        drop(writer);
        assert!(matches!(opening.await, Err(McpError::Discovery(_))));
        let mut accepted_socket = Box::pin(observer.accept());
        assert!(
            futures_util::poll!(&mut accepted_socket).is_pending(),
            "queued Open started no process"
        );
        drop(accepted_socket);

        let binding = child.bind_stdio_mcp(&root).unwrap();
        let cancellation = crate::runtime::cancellation::ExecutionCancellation::detached(
            crate::runtime::CancellationSignal::new(),
            crate::runtime::types::CancellationReason::UserRequested,
        );
        let reply = binding
            .exchange(
                McpRequest::Open {
                    key: binding.key.clone(),
                    source_id: source.to_string(),
                    launch,
                },
                Instant::now() + LOCAL_HANDSHAKE_TIMEOUT,
                &cancellation,
            )
            .await
            .unwrap();
        assert!(matches!(reply, McpReply::Opening));
        let (mut started, _) = tokio::time::timeout(LOCAL_HANDSHAKE_TIMEOUT, observer.accept())
            .await
            .unwrap()
            .unwrap();
        let mut marker = [0; 7];
        started.read_exact(&mut marker).await.unwrap();
        assert_eq!(&marker, b"started");

        let deadline = Instant::now() + LOCAL_HANDSHAKE_TIMEOUT;
        loop {
            let state = binding
                .exchange(
                    McpRequest::State {
                        key: binding.key.clone(),
                    },
                    deadline,
                    &cancellation,
                )
                .await
                .unwrap();
            match state {
                McpReply::Opening => tokio::task::yield_now().await,
                McpReply::Ready { .. } => break,
                other => panic!("the real stdio process did not become Ready: {other:?}"),
            }
        }
        let ping = base64::engine::general_purpose::STANDARD
            .encode(br#"{"jsonrpc":"2.0","id":41,"method":"ping"}"#);
        assert_eq!(
            binding
                .exchange(
                    McpRequest::Send {
                        key: binding.key.clone(),
                        data_base64: ping,
                    },
                    deadline,
                    &cancellation
                )
                .await
                .unwrap(),
            McpReply::Queued
        );
        let mut echo = [0; 4];
        started.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"ping");

        // First block local child admission. Then block the physical data lane
        // in the parent so Receive crosses only the delegated return channel.
        let writer = child
            .endpoint()
            .unwrap()
            .agent_writer
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        assert!(matches!(
            binding.receive(1, 0, Instant::now(), &cancellation).await,
            super::super::ReceiveExchange::Deferred
        ));
        drop(writer);
        let parent = execution.scope.endpoint().unwrap();
        let (held, holding) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let parking = tokio::task::spawn_blocking(move || {
            parent
                .hold_data_exchange_for_test(|| {
                    held.send(()).unwrap();
                    resume.recv().unwrap();
                })
                .unwrap();
        });
        holding.await.unwrap();
        let reply = binding
            .receive(
                1,
                0,
                Instant::now() + std::time::Duration::from_millis(150),
                &cancellation,
            )
            .await;
        assert!(matches!(reply, super::super::ReceiveExchange::Deferred));
        release.send(()).unwrap();
        parking.await.unwrap();
        let deadline = Instant::now() + LOCAL_HANDSHAKE_TIMEOUT;
        loop {
            match binding.receive(1, 0, deadline, &cancellation).await {
                super::super::ReceiveExchange::Reply(McpReply::Page {
                    sequence,
                    offset,
                    data_base64,
                    last,
                }) => {
                    assert_eq!((sequence.get(), offset.get(), last), (1, 0, true));
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(data_base64)
                        .unwrap();
                    let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(
                        response,
                        serde_json::json!({"jsonrpc":"2.0","id":41,"result":{}})
                    );
                    break;
                }
                super::super::ReceiveExchange::Reply(McpReply::Pending) => {
                    tokio::task::yield_now().await;
                }
                _ => panic!("the original stdio cursor was lost"),
            }
        }
        assert!(matches!(
            binding.receive(2, 0, deadline, &cancellation).await,
            super::super::ReceiveExchange::Reply(McpReply::Pending),
        ));
        cleanup.after_child_reaped().await.unwrap();
        let mut ended = [0; 1];
        assert_eq!(
            tokio::time::timeout(LOCAL_HANDSHAKE_TIMEOUT, started.read(&mut ended))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        std::fs::write(root.join("receipt"), b"parent still bound").unwrap();
        let read = execution
            .scope
            .endpoint()
            .unwrap()
            .read_range(&root.join("receipt"), 0, 128, None)
            .unwrap();
        assert_eq!(read.bytes, b"parent still bound");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)] // Drive a real proxied Client, ready MCP server, and parent-held Open on one relay.
    async fn parent_held_initial_open_keeps_child_relay_and_ready_mcp_intact() {
        use crate::credentials::CredentialSnapshot;
        use crate::local_runtime::composition::LocalRuntimeDependencies;
        use crate::local_runtime::configuration::{
            SessionConfigInput, UserConfigManager, UserConfigSources,
        };
        use crate::local_runtime::session::SessionPersistentState;
        use crate::local_runtime::session_controller::SessionController;
        use crate::local_runtime::session_runtime_manager::{
            RuntimeResidencyPolicy, SessionRuntimeManager,
        };
        use crate::tools::mcp::{
            McpError, McpInvalidationState, McpServerBinding, McpServerRuntime, McpTransportConfig,
            OwnedConnect,
        };
        use rustx_execution_protocol::{
            Bootstrap, BootstrapEndpoint, ControlHelloReply, DataHelloReply, PeerHello,
            transport::{read_record, write_record},
        };
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::AsyncReadExt;

        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let runtime_root = temporary.path().join("runtime");
        let home = temporary.path().join("home");
        std::fs::create_dir_all(&runtime_root).unwrap();
        std::fs::create_dir_all(home.join("rustx")).unwrap();
        let controller = SessionController::open(&runtime_root).unwrap();
        let session = controller
            .create_session(SessionPersistentState::from_input(
                &SessionConfigInput::new(root.clone()),
            ))
            .await
            .unwrap()
            .session
            .id;
        let user_config = UserConfigManager::new(UserConfigSources {
            home_directory: home.clone(),
            config_path: home.join("rustx").join("rustx.toml"),
            runtime_root,
        })
        .unwrap();
        let manager = SessionRuntimeManager::new(
            controller,
            user_config,
            CredentialSnapshot::default(),
            LocalRuntimeDependencies::default(),
            RuntimeResidencyPolicy {
                max_resident_runtimes: Some(8),
                idle_grace_ms: 300_000,
            },
        )
        .unwrap();
        let parent_endpoint = manager.reserve_local_execution(&session).await.unwrap();

        let scratch = tempfile::tempdir().unwrap();
        std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let proxy_path = scratch.path().canonicalize().unwrap().join("proxy.sock");
        let proxy = std::os::unix::net::UnixListener::bind(&proxy_path).unwrap();
        let mut bootstrap: Bootstrap =
            read_record(&mut std::fs::File::open(parent_endpoint.bootstrap_path()).unwrap())
                .unwrap();
        let BootstrapEndpoint::Unix {
            socket_path: server_socket,
        } = bootstrap.endpoint
        else {
            panic!("local execution endpoint");
        };
        bootstrap.endpoint = BootstrapEndpoint::Unix {
            socket_path: proxy_path,
        };
        let bundle = scratch.path().join("binding");
        let mut bundle_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&bundle)
            .unwrap();
        write_record(&mut bundle_file, &bootstrap).unwrap();
        drop(bundle_file);

        let client_bin = std::env::current_exe()
            .unwrap()
            .with_file_name("rustx-execution-client");
        let mut client_proc = tokio::process::Command::new(client_bin)
            .arg("--binding-file")
            .arg(bundle.canonicalize().unwrap())
            .arg("--workspace")
            .arg(&root)
            .env_clear()
            .env("TMPDIR", scratch.path())
            .kill_on_drop(true)
            .spawn()
            .unwrap();

        let super::super::ExecutionListener::Unix(server_listener) = parent_endpoint.listener()
        else {
            panic!("Unix listener");
        };
        let server_listener = server_listener.try_clone().unwrap();
        let open_count = Arc::new(AtomicUsize::new(0));
        let open_count_wire = open_count.clone();
        let endpoint_ref = parent_endpoint.clone();
        let (live_tx, live_rx) = tokio::sync::oneshot::channel();
        let proxy_join = std::thread::spawn(move || {
            let (mut data_peer, _) = proxy.accept().unwrap();
            let mut data_server = UnixStream::connect(&server_socket).unwrap();
            let (server_data_accepted, _) = server_listener.accept().unwrap();
            let endpoint_data = endpoint_ref.clone();
            let admit_data =
                std::thread::spawn(move || endpoint_data.admit_local_stream(server_data_accepted));
            let hello: PeerHello = read_record(&mut data_peer).unwrap();
            write_record(&mut data_server, &hello).unwrap();
            let reply: DataHelloReply = read_record(&mut data_server).unwrap();
            write_record(&mut data_peer, &reply).unwrap();
            assert!(matches!(
                admit_data.join().unwrap().unwrap(),
                super::super::EndpointAdmission::Pending
            ));

            let (mut ctrl_peer, _) = proxy.accept().unwrap();
            let mut ctrl_server = UnixStream::connect(&server_socket).unwrap();
            let (server_ctrl_accepted, _) = server_listener.accept().unwrap();
            let endpoint_ctrl = endpoint_ref.clone();
            let admit_ctrl =
                std::thread::spawn(move || endpoint_ctrl.admit_local_stream(server_ctrl_accepted));
            let hello: PeerHello = read_record(&mut ctrl_peer).unwrap();
            write_record(&mut ctrl_server, &hello).unwrap();
            let reply: ControlHelloReply = read_record(&mut ctrl_server).unwrap();
            write_record(&mut ctrl_peer, &reply).unwrap();
            assert!(matches!(
                admit_ctrl.join().unwrap().unwrap(),
                super::super::EndpointAdmission::Live
            ));
            let _ = live_tx.send(());

            let mut data_peer_rx = data_peer.try_clone().unwrap();
            let mut data_server_tx = data_server.try_clone().unwrap();
            let request_fwd = std::thread::spawn(move || {
                while let Ok(req) = read_record::<ClientRequest>(&mut data_server) {
                    if matches!(req, ClientRequest::Mcp(McpRequest::Open { .. })) {
                        open_count_wire.fetch_add(1, Ordering::SeqCst);
                    }
                    if write_record(&mut data_peer, &req).is_err() {
                        break;
                    }
                }
            });
            let reply_fwd = std::thread::spawn(move || {
                while let Ok(rep) = read_record::<ClientReply>(&mut data_peer_rx) {
                    if write_record(&mut data_server_tx, &rep).is_err() {
                        break;
                    }
                }
            });
            let mut ctrl_peer_rx = ctrl_peer.try_clone().unwrap();
            let mut ctrl_server_tx = ctrl_server.try_clone().unwrap();
            let ctrl_s2c = std::thread::spawn(move || {
                let _ = std::io::copy(&mut ctrl_server, &mut ctrl_peer);
            });
            let ctrl_c2s = std::thread::spawn(move || {
                let _ = std::io::copy(&mut ctrl_peer_rx, &mut ctrl_server_tx);
            });
            let _ = request_fwd.join();
            let _ = reply_fwd.join();
            let _ = ctrl_s2c.join();
            let _ = ctrl_c2s.join();
        });

        let observer_socket = root.join("started.sock");
        let observer = tokio::net::UnixListener::bind(&observer_socket).unwrap();
        let server_code = r"
import json, socket, sys
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as proof:
    proof.connect(sys.argv[1])
    proof.sendall(b'started')
    for line in sys.stdin:
        message = json.loads(line)
        if message.get('method') == 'server/discover':
            result = {
                'resultType': 'complete', 'supportedVersions': ['2026-07-28'],
                'capabilities': {'tools': {}}, 'ttlMs': 0, 'cacheScope': 'private',
                '_meta': {'io.modelcontextprotocol/serverInfo': {'name': 'delegated', 'version': '1'}}
            }
        elif message.get('method') == 'tools/list':
            result = {
                'tools': [{'name': 'inspect', 'description': 'inspect', 'inputSchema': {'type': 'object'}}]
            }
        else:
            continue
        print(json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'result': result}), flush=True)
";
        let ready_source = crate::runtime::identity::McpServerId::new("ready-stdio");
        let queued_source = crate::runtime::identity::McpServerId::new("queued-stdio");
        let authored = McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            transport: McpTransportConfig::Stdio {
                program: "/usr/bin/env".into(),
                args: vec![
                    "python3".into(),
                    "-c".into(),
                    server_code.into(),
                    observer_socket.to_string_lossy().into_owned(),
                ],
                cwd: None,
                environment: BTreeMap::from([(
                    "PATH".into(),
                    "/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin".into(),
                )]),
            },
            policy: crate::tools::types::ToolInvocationPolicy::default(),
            permissions: None,
            tool_permissions: BTreeMap::new(),
        };
        let ready_launch = crate::tools::mcp::stdio_launch_spec(&authored, &ready_source, &root)
            .unwrap()
            .unwrap();
        let queued_launch = crate::tools::mcp::stdio_launch_spec(&authored, &queued_source, &root)
            .unwrap()
            .unwrap();
        let conversation = crate::runtime::identity::ConversationId::new(
            "conv_0410af39-84e2-7896-a1f1-06c4c338553d",
        );
        let parent_scope = manager.execution_scope(&session);
        tokio::time::timeout(LOCAL_HANDSHAKE_TIMEOUT, live_rx)
            .await
            .unwrap()
            .unwrap();
        let mut relay =
            ChildExecutionRelay::create(&parent_scope, &root, &conversation, &[], None).unwrap();
        let authority = Arc::get_mut(&mut relay.authority).unwrap();
        authority.tools.insert("read_image".into());
        authority.mcp_sources.extend([
            ChildMcpSource {
                id: ready_source.to_string(),
                launch: ready_launch,
                tools: BTreeSet::from(["inspect".into()]),
                resources: false,
            },
            ChildMcpSource {
                id: queued_source.to_string(),
                launch: queued_launch,
                tools: BTreeSet::from(["inspect".into()]),
                resources: false,
            },
        ]);
        let (parent, child) = tokio::net::UnixStream::pair().unwrap();
        relay.transfer(&parent).await.unwrap();
        let child = relay.grant().open(&session, &root, &child).await.unwrap();
        let cleanup = relay.start().unwrap();
        let child_workspace =
            crate::tools::workspace::Workspace::for_session(&root, child.clone()).unwrap();

        let ready_runtime = tokio::time::timeout(
            LOCAL_HANDSHAKE_TIMEOUT,
            McpServerRuntime::connect_owned(OwnedConnect::new(
                &ready_source,
                &authored,
                &child_workspace,
                Arc::new(McpInvalidationState::new()),
                crate::runtime::CancellationSignal::new(),
            )),
        )
        .await
        .unwrap()
        .unwrap();
        let (mut first_started, _) =
            tokio::time::timeout(LOCAL_HANDSHAKE_TIMEOUT, observer.accept())
                .await
                .unwrap()
                .unwrap();
        let mut marker = [0; 7];
        first_started.read_exact(&mut marker).await.unwrap();
        assert_eq!(&marker, b"started");
        assert_eq!(open_count.load(Ordering::SeqCst), 1);
        assert_eq!(ready_runtime.list_tools().await.unwrap().len(), 1);

        // Hold only the parent's data connection while leaving the child writer free.
        let (held, holding) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let parent_for_hold = parent_endpoint.clone();
        let parking = tokio::task::spawn_blocking(move || {
            parent_for_hold
                .hold_data_exchange_for_test(|| {
                    held.send(()).unwrap();
                    resume.recv().unwrap();
                })
                .unwrap();
        });
        holding.await.unwrap();

        let refused = tokio::time::timeout(
            Duration::from_secs(15),
            McpServerRuntime::connect_owned(OwnedConnect::new(
                &queued_source,
                &authored,
                &child_workspace,
                Arc::new(McpInvalidationState::new()),
                crate::runtime::CancellationSignal::new(),
            )),
        )
        .await
        .unwrap();
        assert!(
            matches!(refused, Err(McpError::Discovery(_))),
            "parent-held pre-dispatch refusal must not escalate to physical settlement failure: {refused:?}"
        );
        assert_eq!(
            open_count.load(Ordering::SeqCst),
            1,
            "the Execution Client must receive zero Open requests for the parent-held attempt"
        );
        let mut unexpected_process = Box::pin(observer.accept());
        assert!(
            futures_util::poll!(&mut unexpected_process).is_pending(),
            "zero MCP processes may start when parent held the data lane"
        );
        drop(unexpected_process);

        release.send(()).unwrap();
        parking.await.unwrap();

        // After releasing the barrier, the same child relay still reads,
        // keeps the existing Ready MCP operation intact, and opens legally.
        std::fs::write(root.join("child-receipt"), b"child still alive").unwrap();
        let read = loop {
            match child
                .endpoint()
                .unwrap()
                .read_range(&root.join("child-receipt"), 0, 128, None)
            {
                Ok(read) => break read,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    tokio::task::yield_now().await;
                }
                Err(error) => panic!("child read_range failed after barrier release: {error}"),
            }
        };
        assert_eq!(read.bytes, b"child still alive");
        assert_eq!(ready_runtime.list_tools().await.unwrap().len(), 1);
        assert_eq!(open_count.load(Ordering::SeqCst), 1);

        let recovered_runtime = tokio::time::timeout(
            LOCAL_HANDSHAKE_TIMEOUT,
            McpServerRuntime::connect_owned(OwnedConnect::new(
                &queued_source,
                &authored,
                &child_workspace,
                Arc::new(McpInvalidationState::new()),
                crate::runtime::CancellationSignal::new(),
            )),
        )
        .await
        .unwrap()
        .unwrap();
        let (mut second_started, _) =
            tokio::time::timeout(LOCAL_HANDSHAKE_TIMEOUT, observer.accept())
                .await
                .unwrap()
                .unwrap();
        second_started.read_exact(&mut marker).await.unwrap();
        assert_eq!(&marker, b"started");
        assert_eq!(open_count.load(Ordering::SeqCst), 2);
        assert_eq!(recovered_runtime.list_tools().await.unwrap().len(), 1);

        ready_runtime.close().await.unwrap();
        recovered_runtime.close().await.unwrap();
        cleanup.after_child_reaped().await.unwrap();
        tokio::task::spawn_blocking(move || {
            parent_endpoint.fence_connection_by(Instant::now() + LOCAL_HANDSHAKE_TIMEOUT)
        })
        .await
        .unwrap()
        .unwrap();
        let _ = client_proc.wait().await;
        let _ = proxy_join.join();
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
                    physically_settled: false,
                    journal_released: false,
                    capture_disposed: false,
                },
            )])),
            tickets: Mutex::default(),
            mcp_sources: Vec::new(),
            mcp_operations: Mutex::default(),
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
        authority
            .retain_reply(
                &command(ProcessRequest::BashCapture { key: key.clone() }),
                &receipt(ProcessReply::BashCapture {
                    key: key.clone(),
                    capture: None,
                }),
            )
            .unwrap();
        assert_eq!(state(), (false, false));
        authority
            .retain_reply(
                &command(ProcessRequest::BashCapture { key: key.clone() }),
                &receipt(ProcessReply::BashCapture {
                    key: key.clone(),
                    capture: Some(BashCaptureResult {
                        stdout: String::new(),
                        stderr: String::new(),
                        combined: String::new(),
                        truncated: true,
                        total: None,
                        output: BashCaptureState::Unavailable,
                        started: true,
                        signal: None,
                    }),
                }),
            )
            .unwrap();
        assert_eq!(state(), (false, false));
        authority
            .retain_reply(
                &command(ProcessRequest::ReleaseOutput {
                    key: key.clone(),
                    final_sequence: 0.into(),
                }),
                &output_released,
            )
            .unwrap();
        assert_eq!(state(), (true, false));
        authority
            .retain_reply(
                &command(ProcessRequest::DiscardBashOutput { key: key.clone() }),
                &receipt(ProcessReply::Conflict { key: key.clone() }),
            )
            .unwrap();
        assert_eq!(state(), (true, false));
        authority
            .retain_reply(
                &command(ProcessRequest::DiscardBashOutput { key: key.clone() }),
                &output_released,
            )
            .unwrap();
        assert_eq!(state(), (true, true));
        assert!(authority.owns_process(&key));
    }
}
