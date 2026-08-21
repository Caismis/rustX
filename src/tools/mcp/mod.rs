//! The MCP adapter.
//!
//! The SDK is intentionally contained in this module. Discovery turns every
//! remote tool into a canonical definition and executor; the agent loop only
//! sees the normal `ToolExecutor` boundary. A server runtime owns one shared
//! rmcp peer, transport, notification subscription, and (for stdio) the
//! rustX interactive process owner.
//!
//! # Protocol revisions
//!
//! rustX does not pin one compile-time protocol revision. It offers the
//! complete set of revisions the resolved `rmcp` build knows
//! ([`ProtocolVersion::KNOWN_VERSIONS`]), newest first, and lets rmcp's own
//! lifecycle machinery pick the mutually supported one:
//!
//! - [`rmcp::ClientLifecycleMode::Auto`] first probes `server/discover` (the
//!   MCP 2026-07-28 inline lifecycle), walking the offered list downwards
//!   whenever the peer answers `UNSUPPORTED_PROTOCOL_VERSION`;
//! - a peer that does not know `server/discover` proves it by answering the
//!   probe with a correlated non-modern JSON-RPC error — legacy servers
//!   variously use `METHOD_NOT_FOUND`, `INVALID_REQUEST`, or
//!   session-middleware rejections — and rmcp (>= 3.1.3) then falls back to
//!   the legacy `initialize`/`notifications/initialized` handshake on the
//!   same connection, offering the newest pre-inline revision rustX speaks;
//! - a peer that silently ignores the probe hits rmcp's bounded discover
//!   timeout and takes the same legacy fallback.
//!
//! rustX then validates the negotiated revision against its own supported
//! set (the legacy handshake lets a server echo any revision it likes) and
//! selects the invalidation mechanism that revision actually defines:
//! `subscriptions/listen` from 2026-07-28 onwards, the plain
//! `notifications/tools/list_changed` client callback before it. At most one
//! invalidation mechanism is installed per connection; when the server
//! advertises `tools.listChanged`, exactly one revision-appropriate
//! mechanism is installed.

#[cfg(feature = "mcp-fixture")]
#[doc(hidden)]
pub mod fixture;

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

use crate::runtime::cancellation::CancellationSignal;
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

/// The MCP protocol revisions rustX offers, most preferred first.
///
/// The set is the resolved `rmcp` build's own
/// [`ProtocolVersion::KNOWN_VERSIONS`] — rustX narrows nothing, because the
/// only MCP surface it uses (`tools/list` with cursor pagination,
/// `tools/call`, progress notifications, `notifications/cancelled`, and
/// `tools/list_changed`) exists in every revision that SDK knows. The order
/// is newest-first: MCP revisions are ISO-8601 dates, so a descending
/// lexicographic sort is a descending chronological sort.
#[must_use]
pub fn supported_protocol_versions() -> Vec<ProtocolVersion> {
    let mut versions = ProtocolVersion::KNOWN_VERSIONS.to_vec();
    versions.sort_by(|left, right| right.as_str().cmp(left.as_str()));
    versions
}

/// Whether a revision defines the inline (`server/discover` +
/// `subscriptions/listen`) lifecycle rather than the legacy
/// `initialize` handshake and bare `tools/list_changed` notification.
fn uses_inline_lifecycle(version: &ProtocolVersion) -> bool {
    // rmcp compares revisions by their ISO-8601 string exactly this way.
    version.as_str() >= ProtocolVersion::V_2026_07_28.as_str()
}

/// The revision rustX offers to a peer that only speaks the legacy
/// `initialize` handshake: the newest known revision that predates the
/// inline lifecycle.
fn legacy_handshake_version() -> ProtocolVersion {
    supported_protocol_versions()
        .into_iter()
        .find(|version| !uses_inline_lifecycle(version))
        .unwrap_or(ProtocolVersion::LATEST)
}

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
///
/// The binding deliberately carries no identity field: an MCP server set is
/// keyed by [`McpServerId`], and the key is the one authoritative identity.
#[derive(Clone, PartialEq, Eq)]
pub struct McpServerBinding {
    /// The configured transport.
    pub transport: McpTransportConfig,
    /// One origin-independent policy for all tools from this server.
    pub policy: ToolInvocationPolicy,
}

impl std::fmt::Debug for McpServerBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpServerBinding")
            .field("transport", &self.transport)
            .field("policy", &self.policy)
            .finish()
    }
}

/// The deterministic keyed MCP server set of one capability owner.
///
/// One identity maps to exactly one binding by construction, so no duplicate
/// check and no ordering pass exists anywhere downstream.
pub type McpServerBindings = BTreeMap<McpServerId, McpServerBinding>;

/// A preparation/execution failure at the MCP adapter boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpError {
    /// Configuration is not valid for the fixed M7 transport contract.
    Configuration(String),
    /// Discovery or lifecycle setup failed.
    Discovery(String),
    /// Client and server share no MCP protocol revision rustX can speak.
    ProtocolCompatibility(String),
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
            Self::ProtocolCompatibility(message) => {
                write!(formatter, "MCP protocol negotiation failed: {message}")
            }
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
    protocol_version: ProtocolVersion,
    peer: rmcp::Peer<RoleClient>,
    service: Arc<tokio::sync::Mutex<Option<RunningService<RoleClient, McpClientHandler>>>>,
    handler: McpClientHandler,
    process: Option<Arc<tokio::sync::Mutex<SupervisedInteractiveProcess>>>,
    /// Closed by the owning capability drain before physical process
    /// settlement is awaited. Stale executor handles cannot start another
    /// remote request after the runtime's process boundary closes.
    closed: Arc<AtomicBool>,
    /// Serializes the close write boundary against every in-flight MCP
    /// catalog/request operation. The capability drain waits for this write
    /// lock, so a remote effect that began before close is settled before the
    /// runtime can publish quiescence and a stale reader that begins after
    /// close observes `closed` before it reaches the peer.
    call_gate: Arc<tokio::sync::RwLock<()>>,
    /// The owned inline-lifecycle notification task. Capability drain joins
    /// it after closing the service, so a buffered notification cannot call
    /// back into capability invalidation after physical/runtime settlement.
    subscription: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    invalidation: Arc<McpInvalidationState>,
    change_notify: Arc<tokio::sync::Notify>,
    /// Test-only close synchronization/fault seam, installed at most once.
    #[cfg(test)]
    close_probe: std::sync::OnceLock<Arc<test_sync::CloseProbe>>,
}

impl std::fmt::Debug for McpServerRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpServerRuntime")
            .field("server_id", &self.server_id)
            .field("protocol_version", &self.protocol_version)
            .field("change_epoch", &self.change_epoch())
            .finish_non_exhaustive()
    }
}

/// One conversation-owned MCP connection request (Issue #12, M9c).
///
/// It carries the **ownership** cancellation signal of the connection, which
/// is deliberately separate from the lifetime of whichever future awaits the
/// result: waiter lifetime is not ownership lifetime.
pub(crate) struct OwnedConnect<'a> {
    server_id: &'a McpServerId,
    binding: &'a McpServerBinding,
    workspace: &'a Workspace,
    invalidation: Arc<McpInvalidationState>,
    cancellation: CancellationSignal,
    /// Test-only: parks the connect exactly once physical process ownership
    /// exists and before the handshake begins.
    #[cfg(test)]
    ownership_pause: Option<Arc<test_sync::ConnectOwnershipPause>>,
}

impl<'a> OwnedConnect<'a> {
    pub(crate) fn new(
        server_id: &'a McpServerId,
        binding: &'a McpServerBinding,
        workspace: &'a Workspace,
        invalidation: Arc<McpInvalidationState>,
        cancellation: CancellationSignal,
    ) -> Self {
        Self {
            server_id,
            binding,
            workspace,
            invalidation,
            cancellation,
            #[cfg(test)]
            ownership_pause: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_ownership_pause(
        mut self,
        pause: Option<Arc<test_sync::ConnectOwnershipPause>>,
    ) -> Self {
        self.ownership_pause = pause;
        self
    }
}

/// Deterministic MCP ownership synchronization seams used by the M9c
/// regressions. Test-only: no production path constructs these.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod test_sync {
    use std::sync::Mutex;

    /// A deterministic close seam for one retained MCP runtime: it records
    /// that `close` was entered, optionally parks there, and optionally makes
    /// the close report an unproven physical settlement.
    #[derive(Debug, Default)]
    pub(crate) struct CloseProbe {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        state: Mutex<CloseState>,
    }

    #[derive(Debug, Default)]
    struct CloseState {
        entered: bool,
        parks: bool,
        released: bool,
        failure: Option<String>,
    }

    impl CloseProbe {
        /// A probe whose close reports an unproven physical settlement.
        pub(crate) fn failing(diagnostic: &str) -> Self {
            Self {
                state: Mutex::new(CloseState {
                    failure: Some(diagnostic.to_owned()),
                    ..CloseState::default()
                }),
                ..Self::default()
            }
        }

        /// A probe whose close parks until [`CloseProbe::release`].
        pub(crate) fn parking() -> Self {
            Self {
                state: Mutex::new(CloseState {
                    parks: true,
                    ..CloseState::default()
                }),
                ..Self::default()
            }
        }

        /// Whether `close` has been entered on the probed runtime.
        pub(crate) fn was_entered(&self) -> bool {
            self.state.lock().expect("close probe lock").entered
        }

        pub(crate) fn injected_failure(&self) -> Option<String> {
            self.state.lock().expect("close probe lock").failure.clone()
        }

        pub(crate) async fn enter(&self) {
            {
                let mut state = self.state.lock().expect("close probe lock");
                state.entered = true;
                if !state.parks || state.released {
                    drop(state);
                    self.entered.notify_waiters();
                    return;
                }
            }
            self.entered.notify_waiters();
            loop {
                let released = self.release.notified();
                tokio::pin!(released);
                released.as_mut().enable();
                if self.state.lock().expect("close probe lock").released {
                    return;
                }
                released.await;
            }
        }

        /// Waits until `close` has been entered on the probed runtime.
        pub(crate) async fn wait_entered(&self) {
            loop {
                let entered = self.entered.notified();
                tokio::pin!(entered);
                entered.as_mut().enable();
                if self.state.lock().expect("close probe lock").entered {
                    return;
                }
                entered.await;
            }
        }

        /// Releases a parked close.
        pub(crate) fn release(&self) {
            self.state.lock().expect("close probe lock").released = true;
            self.release.notify_waiters();
        }
    }

    /// Parks one MCP stdio connect at the exact instant physical process
    /// ownership exists and the handshake has not started.
    #[derive(Debug, Default)]
    pub(crate) struct ConnectOwnershipPause {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        state: Mutex<PauseState>,
    }

    #[derive(Debug, Default)]
    struct PauseState {
        entered: bool,
        released: bool,
    }

    impl ConnectOwnershipPause {
        /// Called by the connect owner: announces physical ownership and
        /// waits for the test to release it.
        pub(crate) async fn park(&self) {
            {
                let mut state = self.state.lock().expect("connect pause lock");
                state.entered = true;
                if state.released {
                    return;
                }
            }
            self.entered.notify_waiters();
            loop {
                let released = self.release.notified();
                tokio::pin!(released);
                released.as_mut().enable();
                if self.state.lock().expect("connect pause lock").released {
                    return;
                }
                released.await;
            }
        }

        /// Waits until physical process ownership provably exists.
        pub(crate) async fn wait_entered(&self) {
            loop {
                let entered = self.entered.notified();
                tokio::pin!(entered);
                entered.as_mut().enable();
                if self.state.lock().expect("connect pause lock").entered {
                    return;
                }
                entered.await;
            }
        }

        /// Releases the parked connect owner.
        pub(crate) fn release(&self) {
            self.state.lock().expect("connect pause lock").released = true;
            self.release.notify_waiters();
        }
    }
}

impl McpServerRuntime {
    /// Connects one configured server, negotiating a mutually supported MCP
    /// protocol revision.
    ///
    /// `server_id` is the authoritative identity of the binding; the binding
    /// itself carries none.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::ProtocolCompatibility`] when no revision is shared
    /// with the peer, and another variant when transport construction,
    /// discovery, capability validation, or subscription setup fails.
    ///
    /// # Panics
    ///
    /// Panics only if the connection's invalidation sink is installed twice,
    /// which the single-install control flow below makes impossible.
    pub async fn connect(
        server_id: &McpServerId,
        binding: &McpServerBinding,
        workspace: &Workspace,
        invalidation: Arc<McpInvalidationState>,
    ) -> Result<Arc<Self>, McpError> {
        Self::connect_owned(OwnedConnect::new(
            server_id,
            binding,
            workspace,
            invalidation,
            CancellationSignal::new(),
        ))
        .await
    }

    /// Connects one configured server under an explicit **ownership**
    /// cancellation signal (Issue #12, M9c).
    ///
    /// The signal cancels the connection *owner*, not merely the caller's
    /// future: once a stdio process has been spawned, cancellation drives
    /// that process to its physical settlement proof before this returns.
    /// Dropping the returned future is therefore never how a conversation
    /// stops an MCP connection — the owning capability coordinator cancels
    /// it and awaits the owner instead.
    ///
    /// # Errors
    ///
    /// Same as [`McpServerRuntime::connect`], plus a cancellation diagnostic
    /// when the owner was cancelled before the handshake completed.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn connect_owned(request: OwnedConnect<'_>) -> Result<Arc<Self>, McpError> {
        let OwnedConnect {
            server_id,
            binding,
            workspace,
            invalidation,
            cancellation,
            #[cfg(test)]
            ownership_pause,
        } = request;
        // Defensive: `McpServerBindings` keys come from validated session
        // configuration, but `connect` is reachable without that parser.
        if server_id.as_str().is_empty() {
            return Err(McpError::Configuration(
                "server_id must be non-empty".to_owned(),
            ));
        }
        if cancellation.is_cancelled() {
            return Err(McpError::Discovery(
                "MCP connection cancelled before any process ownership began".to_owned(),
            ));
        }
        let closed = Arc::new(AtomicBool::new(false));
        let call_gate = Arc::new(tokio::sync::RwLock::new(()));
        let handler = McpClientHandler::new();
        let (service, process) = match &binding.transport {
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
                let process = Arc::new(tokio::sync::Mutex::new(process));
                // Physical process ownership now exists and the handshake has
                // not started. This is the exact window in which a dropped
                // caller future used to leave a detached physical owner with
                // no retained runtime behind it.
                #[cfg(test)]
                if let Some(pause) = &ownership_pause {
                    pause.park().await;
                }
                let stdout = {
                    let mut process_guard = process.lock().await;
                    process_guard.stdout.take()
                };
                let Some(stdout) = stdout else {
                    return Err(settle_connect_failure(
                        Some(&process),
                        McpError::Discovery("stdio stdout unavailable".to_owned()),
                    )
                    .await);
                };
                let stdin = {
                    let mut process_guard = process.lock().await;
                    process_guard.stdin.take()
                };
                let Some(stdin) = stdin else {
                    return Err(settle_connect_failure(
                        Some(&process),
                        McpError::Discovery("stdio stdin unavailable".to_owned()),
                    )
                    .await);
                };
                let transport =
                    rmcp::transport::async_rw::AsyncRwTransport::new_client(stdout, stdin);
                // A handshake failure explicitly awaits the same physical
                // settlement proof as normal runtime drain. Dropping the
                // process handle would only request shutdown and would leave
                // this preparation operation unable to prove terminality.
                // The handshake races the ownership cancellation. Cancelling
                // drops the handshake future but never the process: the owner
                // still awaits the same physical settlement proof drain uses.
                let started = tokio::select! {
                    biased;
                    () = cancellation.cancelled() => Err(McpError::Discovery(
                        "MCP connection cancelled during the handshake".to_owned(),
                    )),
                    started = start_client_service(handler.clone(), transport) => started,
                };
                let service = match started {
                    Ok(service) => service,
                    Err(error) => {
                        return Err(settle_connect_failure(Some(&process), error).await);
                    }
                };
                (service, Some(process))
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
                let service = tokio::select! {
                    biased;
                    () = cancellation.cancelled() => Err(McpError::Discovery(
                        "MCP connection cancelled during the handshake".to_owned(),
                    )),
                    started = start_client_service(handler.clone(), transport) => started,
                }?;
                (service, None)
            }
        };
        let peer = service.peer().clone();
        let info = match peer
            .peer_info()
            .ok_or_else(|| McpError::Discovery("MCP handshake returned no peer info".to_owned()))
        {
            Ok(info) => info,
            Err(error) => return Err(settle_connect_failure(process.as_ref(), error).await),
        };
        // rmcp's discover lifecycle already rejects a peer with no shared
        // revision, but the legacy `initialize` handshake lets a server echo
        // whichever revision it likes. This is the one authority on what
        // rustX actually agreed to speak.
        let supported = supported_protocol_versions();
        if !supported.contains(&info.protocol_version) {
            return Err(
                settle_connect_failure(
                    process.as_ref(),
                    McpError::ProtocolCompatibility(format!(
                        "server negotiated MCP revision {}, which rustX does not speak (rustX speaks {})",
                        info.protocol_version,
                        joined_versions(&supported)
                    )),
                )
                .await,
            );
        }
        if info.capabilities.tools.is_none() {
            return Err(settle_connect_failure(
                process.as_ref(),
                McpError::Discovery("MCP server did not advertise the tools capability".to_owned()),
            )
            .await);
        }
        let runtime = Arc::new(Self {
            server_id: server_id.clone(),
            protocol_version: info.protocol_version.clone(),
            peer,
            service: Arc::new(tokio::sync::Mutex::new(Some(service))),
            handler,
            process,
            closed,
            call_gate,
            subscription: tokio::sync::Mutex::new(None),
            invalidation,
            change_notify: Arc::new(tokio::sync::Notify::new()),
            #[cfg(test)]
            close_probe: std::sync::OnceLock::new(),
        });
        if info
            .capabilities
            .tools
            .as_ref()
            .is_some_and(|tools| tools.list_changed == Some(true))
        {
            // Reached only when the server advertises `tools.listChanged`,
            // so at most one invalidation mechanism exists per connection and
            // exactly one revision-appropriate mechanism is installed here.
            // Both feed the same epoch and the same notify, so neither
            // duplicates the other's subscription, discovery, or published
            // tools.
            if uses_inline_lifecycle(&info.protocol_version) {
                if let Err(error) = runtime.subscribe_tool_list_changed().await {
                    return Err(match runtime.close().await {
                        Ok(()) => error,
                        Err(settlement) => McpError::PhysicalSettlement(format!(
                            "MCP subscription setup failed: {error}; {settlement}"
                        )),
                    });
                }
            } else {
                runtime
                    .handler
                    .install_tool_list_changed_sink(ToolListChangedSink {
                        server_id: runtime.server_id.clone(),
                        invalidation: runtime.invalidation.clone(),
                        change_notify: runtime.change_notify.clone(),
                        closed: runtime.closed.clone(),
                        call_gate: runtime.call_gate.clone(),
                    });
            }
        }
        Ok(runtime)
    }

    /// Opens the MCP 2026-07-28 `subscriptions/listen` stream that carries
    /// `tools/list_changed` for inline-lifecycle peers.
    async fn subscribe_tool_list_changed(self: &Arc<Self>) -> Result<(), McpError> {
        let mut subscription = self
            .peer
            .listen(SubscriptionFilter::builder().tools_list_changed().build())
            .await
            .map_err(|error| McpError::Discovery(bound_error(&error.to_string())))?;
        let server_id = self.server_id.clone();
        let invalidation = self.invalidation.clone();
        let change_notify = self.change_notify.clone();
        let closed = self.closed.clone();
        let call_gate = self.call_gate.clone();
        let task = tokio::spawn(async move {
            while let Ok(Some(notification)) = subscription.next().await {
                let _call_gate = call_gate.read().await;
                if closed.load(Ordering::Acquire) {
                    break;
                }
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
        *self.subscription.lock().await = Some(task);
        Ok(())
    }

    /// The server identity captured by each MCP executor.
    #[must_use]
    pub fn server_id(&self) -> &McpServerId {
        &self.server_id
    }

    /// The MCP protocol revision this connection actually negotiated.
    #[must_use]
    pub const fn protocol_version(&self) -> &ProtocolVersion {
        &self.protocol_version
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
        let _call_gate = self.call_gate.read().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(McpError::Execution(
                "the MCP server runtime is closed".to_owned(),
            ));
        }
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
        #[cfg(test)]
        let probe = self.close_probe.get().cloned();
        #[cfg(test)]
        if let Some(probe) = &probe {
            probe.enter().await;
        }
        self.closed.store(true, Ordering::Release);
        // Request native shutdown before waiting for an in-flight call. A
        // remote tools/call may need the process/transport to close before
        // its response future can settle; waiting for the read gate first
        // would turn that normal ownership cycle into a deadlock.
        if let Some(process) = &self.process {
            let process = process.lock().await;
            process.request_shutdown();
        }
        let service_settlement = if let Some(mut service) = self.service.lock().await.take() {
            service
                .close()
                .await
                .map(|_| ())
                .map_err(|error| format!("MCP service shutdown failed: {error}"))
        } else {
            Ok(())
        };
        // The service close above cancels request futures. Crossing the write
        // barrier now proves that every call or notification that began
        // before close has returned. Callbacks that begin after the barrier
        // observe `closed` and return without mutating invalidation state.
        {
            let _call_gate = self.call_gate.write().await;
        }
        let process_settlement = match &self.process {
            Some(process) => process.lock().await.wait_for_settlement().await,
            None => Ok(()),
        };
        let subscription = self.subscription.lock().await.take();
        let subscription_settlement = if let Some(subscription) = subscription {
            subscription
                .await
                .map_err(|error| format!("MCP notification task failed: {error}"))
        } else {
            Ok(())
        };
        #[cfg(test)]
        if let Some(injected) = probe.and_then(|probe| probe.injected_failure()) {
            return Err(McpError::PhysicalSettlement(injected));
        }
        match (
            process_settlement,
            service_settlement,
            subscription_settlement,
        ) {
            (Err(process), _, _) => Err(McpError::PhysicalSettlement(process)),
            (_, Err(service), _) => Err(McpError::PhysicalSettlement(service)),
            (_, _, Err(subscription)) => Err(McpError::PhysicalSettlement(subscription)),
            (Ok(()), Ok(()), Ok(())) => Ok(()),
        }
    }

    /// Installs the test-only close synchronization/fault seam.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn install_close_probe(&self, probe: Arc<test_sync::CloseProbe>) {
        assert!(
            self.close_probe.set(probe).is_ok(),
            "one close probe per MCP runtime"
        );
    }

    async fn call(
        &self,
        remote_name: &str,
        arguments: serde_json::Value,
        context: &ToolExecutionContext<'_>,
    ) -> ToolExecutionResult {
        let _call_gate = self.call_gate.read().await;
        let started = Instant::now();
        if self.closed.load(Ordering::Acquire) {
            return failed_mcp("MCP server runtime is closed", started);
        }
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
                                reason: context.cancellation.reason(),
                            },
                            content: Vec::new(), duration_ms: duration_ms(started),
                            exit_code: None, artifacts: Vec::new(), truncation: None,
                            managed_output: None,
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
            // `Auto` runs the real negotiation: it probes the inline
            // `server/discover` lifecycle, walks the offered revisions down
            // whenever the peer answers `UNSUPPORTED_PROTOCOL_VERSION`, and
            // falls back to the legacy `initialize` handshake when the peer
            // proves it is pre-2026 — a correlated non-modern JSON-RPC
            // error (`METHOD_NOT_FOUND`, `INVALID_REQUEST`, session
            // middleware rejections, …) or a bounded probe timeout.
            rmcp::ClientLifecycleMode::Auto {
                preferred_versions: supported_protocol_versions(),
                legacy_version: Some(legacy_handshake_version()),
            },
        )
        .await
        .map_err(|error| match error {
            rmcp::service::ClientInitializeError::NoCompatibleProtocolVersion {
                client_supported,
                server_supported,
            } => McpError::ProtocolCompatibility(bound_error(&format!(
                "rustX speaks {}; the server speaks {}",
                joined_versions(&client_supported),
                if server_supported.is_empty() {
                    "no revision it disclosed".to_owned()
                } else {
                    joined_versions(&server_supported)
                }
            ))),
            error => McpError::Discovery(bound_error(&error.to_string())),
        })
}

/// Settles a stdio process whose MCP connection failed before the capability
/// coordinator could retain it as a live runtime. A `Drop` request is only a
/// cancellation signal; preparation must await the existing interactive
/// process settlement proof before it returns, or conversation quiescence
/// could miss this detached physical owner.
async fn settle_connect_failure(
    process: Option<&Arc<tokio::sync::Mutex<SupervisedInteractiveProcess>>>,
    error: McpError,
) -> McpError {
    let Some(process) = process else {
        return error;
    };
    let settlement = {
        let process = process.lock().await;
        process.request_shutdown();
        process.wait_for_settlement().await
    };
    let preview = process.lock().await.stderr_preview();
    match settlement {
        Ok(()) => append_server_stderr(error, &preview),
        Err(reason) => {
            let mut detail = format!("{error}; physical settlement failed: {reason}");
            if !preview.is_empty() {
                detail.push_str("; server stderr: ");
                detail.push_str(&preview);
            }
            McpError::PhysicalSettlement(detail)
        }
    }
}

fn append_server_stderr(error: McpError, preview: &str) -> McpError {
    if preview.is_empty() {
        return error;
    }
    match error {
        McpError::Discovery(message) => {
            McpError::Discovery(format!("{message}; server stderr: {preview}"))
        }
        McpError::ProtocolCompatibility(message) => {
            McpError::ProtocolCompatibility(format!("{message}; server stderr: {preview}"))
        }
        error => error,
    }
}

fn joined_versions(versions: &[ProtocolVersion]) -> String {
    versions
        .iter()
        .map(ProtocolVersion::as_str)
        .collect::<Vec<_>>()
        .join(", ")
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

/// The pre-inline-lifecycle `tools/list_changed` destination.
///
/// Revisions before MCP 2026-07-28 have no `subscriptions/listen`: the server
/// simply emits `notifications/tools/list_changed`, which rmcp routes to the
/// client handler. The sink feeds that notification into the exact same
/// invalidation epoch and notify the inline subscription path uses.
struct ToolListChangedSink {
    server_id: McpServerId,
    invalidation: Arc<McpInvalidationState>,
    change_notify: Arc<tokio::sync::Notify>,
    closed: Arc<AtomicBool>,
    call_gate: Arc<tokio::sync::RwLock<()>>,
}

#[derive(Clone)]
struct McpClientHandler {
    info: ClientInfo,
    progress: ProgressDispatcher,
    /// Installed at most once, and only for a legacy-revision connection.
    tool_list_changed: Arc<std::sync::OnceLock<ToolListChangedSink>>,
}

impl McpClientHandler {
    fn new() -> Self {
        Self {
            info: ClientInfo::new(
                ClientCapabilities::default(),
                Implementation::new("rustx", env!("CARGO_PKG_VERSION")),
            )
            // Only the legacy `initialize` handshake reads this field; the
            // inline lifecycle carries the negotiated candidate in each
            // request's `_meta`. Keeping it equal to the lifecycle's
            // `legacy_version` keeps the two paths from disagreeing.
            .with_protocol_version(legacy_handshake_version()),
            progress: ProgressDispatcher::new(),
            tool_list_changed: Arc::new(std::sync::OnceLock::new()),
        }
    }

    fn install_tool_list_changed_sink(&self, sink: ToolListChangedSink) {
        assert!(
            self.tool_list_changed.set(sink).is_ok(),
            "an MCP connection installs its invalidation sink exactly once"
        );
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

    async fn on_tool_list_changed(&self, _context: rmcp::service::NotificationContext<RoleClient>) {
        let Some(sink) = self.tool_list_changed.get() else {
            return;
        };
        let _call_gate = sink.call_gate.read().await;
        if sink.closed.load(Ordering::Acquire) {
            return;
        }
        // The one shared invalidation boundary, exactly as the inline
        // subscription path uses it.
        let mut guard = sink.invalidation.lock();
        guard.advance(&sink.server_id);
        drop(guard);
        sink.change_notify.notify_waiters();
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
        managed_output: None,
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
        managed_output: None,
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
