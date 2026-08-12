//! MCP 2026-07-28 adapter.
//!
//! The SDK is intentionally contained in this module. Discovery turns every
//! remote tool into a canonical definition and executor; the agent loop only
//! sees the normal `ToolExecutor` boundary. A server runtime owns one shared
//! rmcp peer, transport, notification subscription, and (for stdio) the
//! rustX interactive process owner.

#[cfg(feature = "mcp-fixture")]
#[doc(hidden)]
pub mod fixture;

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use futures_util::StreamExt;
use rmcp::handler::client::progress::ProgressDispatcher;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo,
    ClientRequest, ContentBlock, Implementation, ProgressNotificationParam, ProtocolVersion,
    ServerNotification, ServerResult, SubscriptionFilter,
};
use rmcp::service::{PeerRequestOptions, RoleClient, RunningService};
use rmcp::{ClientHandler, ClientServiceExt};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::runtime::identity::{McpServerId, ToolId};
use crate::runtime::interactive_process::{InteractiveProcessSpec, SupervisedInteractiveProcess};
use crate::tools::artifacts::ArtifactStore;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{
    MAX_MODEL_TOOL_RESULT_BYTES, bound_tool_progress, bounded_text_preview,
};
use crate::tools::types::{
    ToolDefinition, ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationPolicy,
    ToolOrigin, ToolReplayPolicy, ToolResultContent,
};
use crate::tools::workspace::Workspace;

/// The MCP protocol revision implemented by M7.
pub const MCP_PROTOCOL_VERSION: &str = "2026-07-28";

/// The one shared `tools/list_changed` invalidation synchronization boundary
/// of a capability coordinator.
///
/// MCP invalidation epoch mutation (a received `tools/list_changed`
/// notification) and capability snapshot activation (preparation epoch
/// snapshot + commit epoch validation) share exactly one mutex, so they have
/// one synchronization order:
///
/// - if the notification wins first, the prepared candidate cannot commit
///   (its epoch is stale);
/// - if the commit wins first, the notification belongs to a future refresh
///   and can never retroactively invalidate the already-committed snapshot.
///
/// Lock ordering is explicit and documented: the capability commit lock
/// (the coordinator's `state` mutex) is always acquired **before** this
/// invalidation guard, and the notification path acquires **only** this
/// guard. No path ever takes the capability state lock while holding this
/// guard, so no cycle exists.
///
/// The type is `pub` only because the public `McpServerRuntime::connect`
/// entry point accepts it; it is an internal runtime coordination boundary,
/// not a stable application interface.
#[doc(hidden)]
pub struct McpInvalidationState {
    inner: std::sync::Mutex<BTreeMap<McpServerId, u64>>,
}

impl McpInvalidationState {
    /// Creates an empty invalidation state.
    #[must_use]
    #[doc(hidden)]
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// Acquires the invalidation guard: the single shared boundary of epoch
    /// mutation and epoch validation.
    ///
    /// # Panics
    ///
    /// Panics only if the invalidation lock is poisoned.
    #[must_use]
    #[doc(hidden)]
    pub fn lock(&self) -> McpInvalidationGuard<'_> {
        McpInvalidationGuard {
            guard: self.inner.lock().expect("MCP invalidation lock poisoned"),
        }
    }

    /// The current invalidation epoch of one server.
    ///
    /// # Panics
    ///
    /// Panics only if the invalidation lock is poisoned.
    #[must_use]
    #[doc(hidden)]
    pub fn epoch(&self, server_id: &McpServerId) -> u64 {
        self.lock().epoch(server_id)
    }
}

impl Default for McpInvalidationState {
    fn default() -> Self {
        Self::new()
    }
}

/// The held invalidation guard: epoch reads and mutations under it are
/// ordered against the capability commit's epoch validation and swap.
#[doc(hidden)]
pub struct McpInvalidationGuard<'a> {
    guard: std::sync::MutexGuard<'a, BTreeMap<McpServerId, u64>>,
}

impl McpInvalidationGuard<'_> {
    /// The current epoch of one server.
    #[must_use]
    #[doc(hidden)]
    pub fn epoch(&self, server_id: &McpServerId) -> u64 {
        self.guard.get(server_id).copied().unwrap_or(0)
    }

    /// Advances one server's invalidation epoch — the exact mutation the
    /// `tools/list_changed` notification performs.
    #[doc(hidden)]
    pub fn advance(&mut self, server_id: &McpServerId) {
        let current = self.guard.get(server_id).copied().unwrap_or(0);
        self.guard.insert(server_id.clone(), current + 1);
    }
}

/// A configured MCP server transport.
#[derive(Clone, PartialEq, Eq)]
pub enum McpTransportConfig {
    /// A stdio server launched with an explicit environment and workspace
    /// relative working directory.
    Stdio {
        /// Executable path or explicit executable name.
        program: String,
        /// Program arguments.
        args: Vec<String>,
        /// Workspace-relative working directory; `None` means workspace root.
        cwd: Option<PathBuf>,
        /// Explicit child environment.
        environment: BTreeMap<String, String>,
    },
    /// A stateless Streamable HTTP endpoint with explicit headers.
    StreamableHttp {
        /// Endpoint URL.
        endpoint: String,
        /// Explicit static request headers.
        headers: BTreeMap<String, String>,
    },
}

impl std::fmt::Debug for McpTransportConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdio {
                program,
                args,
                cwd,
                environment,
            } => formatter
                .debug_struct("Stdio")
                .field("program", program)
                .field("arg_count", &args.len())
                .field("cwd", cwd)
                .field("environment_keys", &environment.keys().collect::<Vec<_>>())
                .finish(),
            Self::StreamableHttp { endpoint, headers } => formatter
                .debug_struct("StreamableHttp")
                .field("endpoint_configured", &!endpoint.is_empty())
                .field("header_names", &headers.keys().collect::<Vec<_>>())
                .finish(),
        }
    }
}

/// One immutable MCP server binding.
#[derive(Clone, PartialEq, Eq)]
pub struct McpServerConfig {
    /// Non-empty stable server identity.
    pub server_id: McpServerId,
    /// The configured transport.
    pub transport: McpTransportConfig,
    /// One origin-independent policy for all tools from this server.
    pub policy: ToolInvocationPolicy,
}

impl std::fmt::Debug for McpServerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpServerConfig")
            .field("server_id", &self.server_id)
            .field("transport", &self.transport)
            .field("policy", &self.policy)
            .finish()
    }
}

/// A preparation/execution failure at the MCP adapter boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpError {
    /// Configuration is not valid for the fixed M7 transport contract.
    Configuration(String),
    /// Discovery or lifecycle setup failed.
    Discovery(String),
    /// The remote call or response could not be translated.
    Execution(String),
    /// The owned stdio process tree could not be proven terminal, so no
    /// physical settlement could be published.
    PhysicalSettlement(String),
}

impl std::fmt::Display for McpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(message) => {
                write!(formatter, "MCP configuration failed: {message}")
            }
            Self::Discovery(message) => write!(formatter, "MCP discovery failed: {message}"),
            Self::Execution(message) => write!(formatter, "MCP execution failed: {message}"),
            Self::PhysicalSettlement(message) => {
                write!(formatter, "MCP physical settlement is unproven: {message}")
            }
        }
    }
}

impl std::error::Error for McpError {}

/// One shared runtime for every tool exposed by a configured server.
pub struct McpServerRuntime {
    server_id: McpServerId,
    peer: rmcp::Peer<RoleClient>,
    service: Arc<tokio::sync::Mutex<Option<RunningService<RoleClient, McpClientHandler>>>>,
    handler: McpClientHandler,
    process: Option<Arc<tokio::sync::Mutex<SupervisedInteractiveProcess>>>,
    invalidation: Arc<McpInvalidationState>,
    change_notify: Arc<tokio::sync::Notify>,
}

impl std::fmt::Debug for McpServerRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpServerRuntime")
            .field("server_id", &self.server_id)
            .field("change_epoch", &self.change_epoch())
            .finish_non_exhaustive()
    }
}

impl McpServerRuntime {
    /// Connects one configured server with the current Discover lifecycle.
    ///
    /// # Errors
    ///
    /// Returns an error when transport construction, discovery, capability
    /// validation, or subscription setup fails.
    #[allow(clippy::too_many_lines)]
    pub async fn connect(
        config: &McpServerConfig,
        workspace: &Workspace,
        invalidation: Arc<McpInvalidationState>,
    ) -> Result<Arc<Self>, McpError> {
        if config.server_id.as_str().is_empty() {
            return Err(McpError::Configuration(
                "server_id must be non-empty".to_owned(),
            ));
        }
        let handler = McpClientHandler::new();
        let (service, process) = match &config.transport {
            McpTransportConfig::Stdio {
                program,
                args,
                cwd,
                environment,
            } => {
                if program.trim().is_empty() {
                    return Err(McpError::Configuration(
                        "stdio program must be non-empty".to_owned(),
                    ));
                }
                let cwd = resolve_workspace_cwd(workspace, cwd.as_deref())?;
                let mut explicit_environment =
                    vec![("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned())];
                explicit_environment.extend(
                    environment
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
                let process = SupervisedInteractiveProcess::spawn(InteractiveProcessSpec {
                    program: PathBuf::from(program),
                    args: args.clone(),
                    cwd,
                    environment: explicit_environment,
                })
                .map_err(McpError::Discovery)?;
                let mut process = process;
                let stdout = process
                    .stdout
                    .take()
                    .ok_or_else(|| McpError::Discovery("stdio stdout unavailable".to_owned()))?;
                let stdin = process
                    .stdin
                    .take()
                    .ok_or_else(|| McpError::Discovery("stdio stdin unavailable".to_owned()))?;
                let transport =
                    rmcp::transport::async_rw::AsyncRwTransport::new_client(stdout, stdin);
                // A handshake failure drops `process`, which requests
                // shutdown from the runtime-owned driver — the physical
                // owner is never abandoned. The bounded stderr preview is
                // the server's own diagnosis of why the handshake failed.
                let service = match start_client_service(handler.clone(), transport).await {
                    Ok(service) => service,
                    Err(error) => {
                        let preview = process.stderr_preview();
                        return Err(if preview.is_empty() {
                            error
                        } else {
                            McpError::Discovery(format!("{error}; server stderr: {preview}"))
                        });
                    }
                };
                (service, Some(Arc::new(tokio::sync::Mutex::new(process))))
            }
            McpTransportConfig::StreamableHttp { endpoint, headers } => {
                if endpoint.trim().is_empty() {
                    return Err(McpError::Configuration(
                        "HTTP endpoint must be non-empty".to_owned(),
                    ));
                }
                let mut transport_config = rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(endpoint.clone());
                transport_config.custom_headers = headers
                    .iter()
                    .map(|(name, value)| {
                        let name = http::HeaderName::try_from(name).map_err(|_| {
                            McpError::Configuration("invalid HTTP header name".to_owned())
                        })?;
                        let value = http::HeaderValue::try_from(value).map_err(|_| {
                            McpError::Configuration("invalid HTTP header value".to_owned())
                        })?;
                        Ok((name, value))
                    })
                    .collect::<Result<_, McpError>>()?;
                transport_config.reinit_on_expired_session = false;
                transport_config.allow_stateless = true;
                let transport =
                    rmcp::transport::StreamableHttpClientTransport::from_config(transport_config);
                let service = start_client_service(handler.clone(), transport).await?;
                (service, None)
            }
        };
        let peer = service.peer().clone();
        let info = peer.peer_info().ok_or_else(|| {
            McpError::Discovery("server/discover returned no peer info".to_owned())
        })?;
        if info.protocol_version != ProtocolVersion::V_2026_07_28 {
            return Err(McpError::Discovery(format!(
                "unsupported MCP protocol revision {}",
                info.protocol_version
            )));
        }
        if info.capabilities.tools.is_none() {
            return Err(McpError::Discovery(
                "MCP server did not advertise the tools capability".to_owned(),
            ));
        }
        let runtime = Arc::new(Self {
            server_id: config.server_id.clone(),
            peer,
            service: Arc::new(tokio::sync::Mutex::new(Some(service))),
            handler,
            process,
            invalidation,
            change_notify: Arc::new(tokio::sync::Notify::new()),
        });
        if info
            .capabilities
            .tools
            .as_ref()
            .is_some_and(|tools| tools.list_changed == Some(true))
        {
            let mut subscription = runtime
                .peer
                .listen(SubscriptionFilter::builder().tools_list_changed().build())
                .await
                .map_err(|error| McpError::Discovery(bound_error(&error.to_string())))?;
            let server_id = runtime.server_id.clone();
            let invalidation = runtime.invalidation.clone();
            let change_notify = runtime.change_notify.clone();
            tokio::spawn(async move {
                while let Ok(Some(notification)) = subscription.next().await {
                    if matches!(
                        notification,
                        ServerNotification::ToolListChangedNotification(_)
                    ) {
                        // The one shared invalidation boundary: the epoch
                        // mutation serializes against preparation epoch
                        // snapshots and the commit's epoch validation + swap.
                        let mut guard = invalidation.lock();
                        guard.advance(&server_id);
                        drop(guard);
                        change_notify.notify_waiters();
                    }
                }
            });
        }
        Ok(runtime)
    }

    /// The server identity captured by each MCP executor.
    #[must_use]
    pub fn server_id(&self) -> &McpServerId {
        &self.server_id
    }

    /// The invalidation epoch at the current observation point.
    #[must_use]
    pub fn change_epoch(&self) -> u64 {
        self.invalidation.epoch(&self.server_id)
    }

    /// Waits for a newer tools/list invalidation epoch without polling.
    pub async fn wait_for_change(&self, observed_epoch: u64) {
        loop {
            let notified = self.change_notify.notified();
            if self.change_epoch() != observed_epoch {
                return;
            }
            notified.await;
        }
    }

    /// Returns a complete, paginated remote catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when the remote catalog cannot be fetched or a tool
    /// cannot be translated into rustX's canonical schema contract.
    pub async fn list_tools(&self) -> Result<Vec<CanonicalMcpTool>, McpError> {
        let tools = self
            .peer
            .list_all_tools()
            .await
            .map_err(|error| McpError::Discovery(bound_error(&error.to_string())))?;
        let mut canonical = tools
            .into_iter()
            .map(CanonicalMcpTool::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        canonical.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(canonical)
    }

    /// Gracefully retires the runtime and its owned stdio process, when any.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::PhysicalSettlement`] when the owned stdio unit
    /// could not publish physical settlement: its owned process tree's
    /// terminal state is unproven. The MCP service is retired either way,
    /// but an unproven terminal state is never reported as success.
    pub async fn close(&self) -> Result<(), McpError> {
        let settlement = match &self.process {
            Some(process) => {
                let process = process.lock().await;
                process.request_shutdown();
                process.wait_for_settlement().await
            }
            None => Ok(()),
        };
        if let Some(service) = self.service.lock().await.as_mut() {
            let _ = service.close().await;
        }
        settlement.map_err(McpError::PhysicalSettlement)
    }

    async fn call(
        &self,
        remote_name: &str,
        arguments: serde_json::Value,
        context: &ToolExecutionContext<'_>,
    ) -> ToolExecutionResult {
        let started = Instant::now();
        let serde_json::Value::Object(arguments) = arguments else {
            return failed_mcp("MCP tool arguments must be a JSON object", started);
        };
        let params = CallToolRequestParams::new(remote_name.to_owned()).with_arguments(arguments);
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
        let mut handle = match self
            .peer
            .send_cancellable_request(request, PeerRequestOptions::no_options())
            .await
        {
            Ok(handle) => handle,
            Err(error) => return failed_mcp(&bound_error(&error.to_string()), started),
        };
        let mut progress = self
            .handler
            .progress
            .subscribe(handle.progress_token.clone())
            .await;
        let response = loop {
            tokio::select! {
                biased;
                response = &mut handle.rx => break Some(response),
                () = context.cancellation.cancelled() => {
                    let cancelled = handle.cancel(Some("rustX execution cancellation".to_owned())).await;
                    drop(progress);
                    return match cancelled {
                        Ok(()) => ToolExecutionResult {
                            status: ToolExecutionStatus::Cancelled {
                                reason: crate::runtime::types::CancellationReason::UserRequested,
                            },
                            content: Vec::new(), duration_ms: duration_ms(started),
                            exit_code: None, artifacts: Vec::new(), truncation: None,
                        },
                        Err(error) => failed_mcp(&format!("MCP cancellation failed: {}", bound_error(&error.to_string())), started),
                    };
                }
                progress_item = progress.next() => {
                    if let Some(progress_item) = progress_item {
                        // The shared canonical normalization drops non-finite
                        // `completed`/`total` values; the remote value needs
                        // no adapter-local filter.
                        context.progress.report(bound_tool_progress(crate::tools::types::ToolProgress {
                            message: progress_item.message,
                            completed: Some(progress_item.progress),
                            total: progress_item.total,
                        }));
                    }
                }
            }
        };
        drop(progress);
        let response = match response {
            Some(Ok(Ok(ServerResult::CallToolResult(result)))) => result,
            Some(Ok(Ok(ServerResult::InputRequiredResult(_)))) => {
                return failed_mcp("MCP input_required results are unsupported in M7", started);
            }
            Some(Ok(Ok(_))) => return failed_mcp("unexpected MCP tools/call response", started),
            Some(Ok(Err(error))) => return failed_mcp(&bound_error(&error.to_string()), started),
            Some(Err(_)) | None => {
                return failed_mcp("MCP transport closed during tools/call", started);
            }
        };
        translate_result(response, context.artifacts, started)
    }
}

async fn start_client_service<T, E, A>(
    handler: McpClientHandler,
    transport: T,
) -> Result<RunningService<RoleClient, McpClientHandler>, McpError>
where
    T: rmcp::transport::IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    handler
        .serve_with_lifecycle(
            transport,
            rmcp::ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .map_err(|error| McpError::Discovery(bound_error(&error.to_string())))
}

/// A canonicalized MCP tool definition at the adapter boundary.
pub struct CanonicalMcpTool {
    /// Remote model-facing name.
    pub name: String,
    /// Remote description, normalized to a string.
    pub description: String,
    /// Canonical JSON schema.
    pub input_schema: serde_json::Value,
}

impl TryFrom<rmcp::model::Tool> for CanonicalMcpTool {
    type Error = McpError;

    fn try_from(tool: rmcp::model::Tool) -> Result<Self, Self::Error> {
        let name = tool.name.into_owned();
        if name.is_empty() {
            return Err(McpError::Discovery("MCP tool name is empty".to_owned()));
        }
        let input_schema = serde_json::to_value(tool.input_schema)
            .map_err(|error| McpError::Discovery(bound_error(&error.to_string())))?;
        Ok(Self {
            name,
            description: tool
                .description
                .map_or_else(String::new, std::borrow::Cow::into_owned),
            input_schema,
        })
    }
}

/// One canonical executor bound to the exact server runtime captured at
/// capability preparation.
pub struct McpToolExecutor {
    runtime: Arc<McpServerRuntime>,
    remote_name: String,
}

impl McpToolExecutor {
    /// Creates an executor bound to one discovered remote name.
    #[must_use]
    pub fn new(runtime: Arc<McpServerRuntime>, remote_name: String) -> Self {
        Self {
            runtime,
            remote_name,
        }
    }
}

impl ToolExecutor for McpToolExecutor {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> futures_util::future::BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move {
            self.runtime
                .call(&self.remote_name, invocation.arguments, &context)
                .await
        })
    }
}

/// Converts one server catalog into canonical registry entries.
#[must_use]
pub fn definitions(
    server_id: &McpServerId,
    policy: ToolInvocationPolicy,
    runtime: &Arc<McpServerRuntime>,
    tools: Vec<CanonicalMcpTool>,
) -> Vec<(ToolDefinition, Arc<dyn ToolExecutor>)> {
    tools
        .into_iter()
        .map(|tool| {
            let id = ToolId::new(mcp_tool_id(server_id, &tool.name));
            let definition = ToolDefinition {
                id,
                name: tool.name.clone(),
                description: tool.description,
                input_schema: tool.input_schema,
                execution_policy: policy.execution,
                concurrency_policy: policy.concurrency,
                replay_policy: ToolReplayPolicy::Never,
                origin: ToolOrigin::Mcp {
                    server_id: server_id.clone(),
                },
            };
            (
                definition,
                Arc::new(McpToolExecutor::new(runtime.clone(), tool.name)) as Arc<dyn ToolExecutor>,
            )
        })
        .collect()
}

/// Generates a stable, length-prefixed MCP logical tool identity.
#[must_use]
pub fn mcp_tool_id(server_id: &McpServerId, remote_name: &str) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"rustx:mcp-tool-id:v1\0");
    append_length_prefixed(&mut bytes, server_id.as_str().as_bytes());
    append_length_prefixed(&mut bytes, remote_name.as_bytes());
    let digest = sha2::Sha256::digest(bytes);
    format!("mcp:sha256:{}", hex_digest(&digest))
}

fn append_length_prefixed(target: &mut Vec<u8>, bytes: &[u8]) {
    target.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    target.extend_from_slice(bytes);
}

fn hex_digest(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(result, "{byte:02x}");
    }
    result
}

#[derive(Clone)]
struct McpClientHandler {
    info: ClientInfo,
    progress: ProgressDispatcher,
}

impl McpClientHandler {
    fn new() -> Self {
        Self {
            info: ClientInfo::new(
                ClientCapabilities::default(),
                Implementation::new("rustx", env!("CARGO_PKG_VERSION")),
            )
            .with_protocol_version(ProtocolVersion::V_2026_07_28),
            progress: ProgressDispatcher::new(),
        }
    }
}

impl ClientHandler for McpClientHandler {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.progress.handle_notification(params).await;
    }

    fn get_info(&self) -> ClientInfo {
        self.info.clone()
    }
}

fn resolve_workspace_cwd(workspace: &Workspace, cwd: Option<&Path>) -> Result<PathBuf, McpError> {
    let Some(cwd) = cwd else {
        return Ok(workspace.root().to_path_buf());
    };
    if cwd.is_absolute()
        || cwd.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(McpError::Configuration(
            "stdio cwd must be a workspace-relative path without parent components".to_owned(),
        ));
    }
    let resolved = workspace.root().join(cwd);
    if !resolved.is_dir() {
        return Err(McpError::Configuration(
            "stdio cwd is not a directory".to_owned(),
        ));
    }
    Ok(resolved)
}

fn translate_result(
    result: CallToolResult,
    artifacts: &ArtifactStore,
    started: Instant,
) -> ToolExecutionResult {
    let mut content = Vec::new();
    let mut unsupported = None;
    let mut remaining = MAX_MODEL_TOOL_RESULT_BYTES;
    let mut original_bytes = 0usize;
    let mut truncated = false;
    for block in result.content {
        match block {
            ContentBlock::Text(text) => {
                original_bytes = original_bytes.saturating_add(text.text.len());
                let (preview, was_truncated) =
                    bounded_text_preview(text.text.as_bytes(), remaining);
                remaining = remaining.saturating_sub(preview.len());
                truncated |= was_truncated;
                content.push(ToolResultContent::Text(
                    crate::message::content::TextBlock { text: preview },
                ));
            }
            ContentBlock::Image(image) => {
                match base64::engine::general_purpose::STANDARD.decode(image.data.as_bytes()) {
                    Ok(bytes) if bytes.len() <= remaining => {
                        match write_artifact(artifacts, &bytes) {
                            Ok(id) => {
                                remaining = remaining.saturating_sub(bytes.len());
                                original_bytes = original_bytes.saturating_add(bytes.len());
                                content.push(ToolResultContent::Image(
                                    crate::message::content::ImageReference {
                                        artifact_id: id,
                                        alt: None,
                                    },
                                ));
                            }
                            Err(error) => unsupported = Some(error),
                        }
                    }
                    Ok(_) => {
                        unsupported = Some(
                            "MCP image content exceeds the bounded tool-result limit".to_owned(),
                        );
                    }
                    Err(error) => unsupported = Some(format!("invalid MCP image data: {error}")),
                }
            }
            ContentBlock::Audio(_) | ContentBlock::Resource(_) | ContentBlock::ResourceLink(_) => {
                unsupported =
                    Some("MCP content variant has no canonical M7 representation".to_owned());
            }
            _ => unsupported = Some("unknown MCP content variant is unsupported in M7".to_owned()),
        }
    }
    if let Some(value) = result.structured_content {
        match serde_json::to_vec(&value) {
            Ok(bytes) if bytes.len() <= remaining => {
                original_bytes = original_bytes.saturating_add(bytes.len());
                content.push(ToolResultContent::Json { value });
            }
            Ok(_) => {
                unsupported =
                    Some("MCP structured content exceeds the bounded tool-result limit".to_owned());
            }
            Err(error) => unsupported = Some(format!("invalid MCP structured content: {error}")),
        }
    }
    let status = if let Some(error) = unsupported {
        ToolExecutionStatus::Failed { error }
    } else if result.is_error == Some(true) {
        ToolExecutionStatus::Failed {
            error: "MCP tool reported an execution error".to_owned(),
        }
    } else {
        ToolExecutionStatus::Success
    };
    ToolExecutionResult {
        status,
        content,
        duration_ms: duration_ms(started),
        exit_code: None,
        artifacts: Vec::new(),
        truncation: truncated.then_some(crate::tools::types::TruncationState {
            truncated: true,
            original_bytes: u64::try_from(original_bytes).ok(),
        }),
    }
}

fn write_artifact(
    artifacts: &ArtifactStore,
    bytes: &[u8],
) -> Result<crate::runtime::identity::ArtifactId, String> {
    use std::io::Write;
    let id = artifacts
        .create_artifact()
        .map_err(|error| error.to_string())?;
    let mut writer = artifacts
        .open_writer(&id)
        .map_err(|error| error.to_string())?;
    writer.write_all(bytes).map_err(|error| error.to_string())?;
    Ok(id)
}

fn failed_mcp(message: &str, started: Instant) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Failed {
            error: bound_error(message),
        },
        content: Vec::new(),
        duration_ms: duration_ms(started),
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
    }
}

fn duration_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn bound_error(message: &str) -> String {
    const LIMIT: usize = 1024;
    bounded_text_preview(message.as_bytes(), LIMIT).0
}

/// The serializable provenance carried by a committed MCP binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCapabilityBinding {
    /// Server identity only; credentials and runtime addresses never appear.
    pub server_id: McpServerId,
}

#[cfg(test)]
mod tests {
    use super::{McpServerId, mcp_tool_id};

    #[test]
    fn mcp_tool_ids_are_length_prefixed_and_stable() {
        let first = mcp_tool_id(&McpServerId::new("a"), "b");
        assert_eq!(first, mcp_tool_id(&McpServerId::new("a"), "b"));
        assert_ne!(first, mcp_tool_id(&McpServerId::new("ab"), ""));
        assert!(first.starts_with("mcp:sha256:"));
    }
}
