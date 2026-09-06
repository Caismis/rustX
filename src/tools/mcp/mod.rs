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
//! # Stdio protocol corruption
//!
//! Stdout of a stdio server is protocol-owned; stderr is the diagnostics
//! channel. rmcp's framing deliberately keeps decode failures to itself
//! (plain noise is ignored; structurally invalid MCP/JSON-RPC data earns
//! only a peer-facing `Invalid Request` reply), so the generic observation
//! seam in [`framing`] records a confirmed structurally invalid peer
//! message as a rustX fact, ends the byte stream after the offending line,
//! and poisons the connection generation: the in-flight operation fails
//! with [`McpError::ProtocolViolation`] naming the server, and the
//! generation never serves another call as healthy.
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

mod connection;
mod framing;
pub mod identity;
mod streamable_http;

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use base64::Engine;
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
use crate::runtime::types::LifecycleAdmission;
use crate::tools::artifacts::ArtifactStore;
use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{
    FOREGROUND_TOOL_RESULT_PREVIEW_BYTES, MAX_MODEL_TOOL_RESULT_BYTES, bound_tool_progress,
    bounded_text_preview,
};
use crate::tools::output::{
    CapturedOutput, TextPreviewCapture, ToolOutputCapture, ToolOutputWriter,
    continuation_for_capture, truncation_for_capture,
};
use crate::tools::types::{
    ManagedOutputContinuation, ToolDefinition, ToolExecutionResult, ToolExecutionStatus,
    ToolInvocation, ToolInvocationPolicy, ToolOrigin, ToolReplayPolicy, ToolResultContent,
};
use crate::tools::workspace::Workspace;

pub(crate) use connection::McpConnection;

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
///
/// The type is serializable because a subagent child materializes the exact
/// transport its parent generation froze (Issue #145): the frozen binding
/// crosses the private subagent control channel rather than being
/// rediscovered from `rustx.jsonc` in the child.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    /// The peer emitted a structurally invalid MCP/JSON-RPC message on the
    /// wire. This is not version negotiation and it is not peer-only
    /// traffic: the violation is a rustX runtime fact, the current
    /// operation fails, and the connection generation is protocol-poisoned
    /// — it is never silently treated as healthy again.
    ProtocolViolation(String),
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
            Self::ProtocolViolation(message) => {
                write!(formatter, "MCP protocol violation: {message}")
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
    /// The generic stdio protocol-corruption observation seam: a confirmed
    /// structurally invalid peer message recorded by the transport tee.
    /// Once set, this generation is poisoned — no operation on it may
    /// settle as healthy.
    protocol_violation: Arc<framing::ProtocolViolationRecorder>,
    /// The rustX-owned local ownership of this generation's in-flight HTTP
    /// requests (Issue #205), for Streamable HTTP only.
    ///
    /// Over stdio an outbound write owns no local resource that outlives it,
    /// so there is nothing per-request to terminate and this is `None`. Over
    /// Streamable HTTP each dispatched request is a live HTTP request whose
    /// POST future and response body rustX must be able to terminate and
    /// prove released before it may report unconfirmed settlement — see
    /// [`streamable_http::McpHttpRequestOwnership`].
    request_ownership: Option<Arc<streamable_http::McpHttpRequestOwnership>>,
    /// The first observed proof that this transport generation can no longer
    /// carry MCP traffic (Issue #205): a transport-class rmcp failure on a
    /// request, or a response channel that ended without a reply. Recording
    /// it is monotonic and idempotent — repeated loss signals never produce
    /// a second retirement — and the owning [`connection::McpConnection`]
    /// reads it to retire this generation before any later request is
    /// dispatched.
    transport_failure: Mutex<Option<String>>,
    /// Test-only close synchronization/fault seam, installed at most once.
    #[cfg(test)]
    close_probe: std::sync::OnceLock<Arc<test_sync::CloseProbe>>,
}

/// The coordinator-owned retirement registry for MCP physical generations.
///
/// A generation is registered only after its publication/candidate owner has
/// retired. The registry contains no active pool: it is a bounded list of
/// close tasks that have not yet been reaped, plus authoritative terminal
/// failure evidence retained until the owning runtime drains it. A generation
/// whose physical settlement is unproven is never reaped.
pub(crate) struct McpRuntimeRetirementRegistry {
    inner: Arc<McpRuntimeRetirementRegistryInner>,
}

pub(crate) type McpRetirementFailureCallback = Arc<dyn Fn(String) + Send + Sync>;

struct McpRuntimeRetirementRegistryInner {
    entries: Mutex<Vec<Arc<McpRuntimeGenerationInner>>>,
    failures: Mutex<Vec<(McpServerId, String)>>,
    failure_callback: Mutex<Option<McpRetirementFailureCallback>>,
}

impl McpRuntimeRetirementRegistry {
    /// Creates the one retirement registry of a capability coordinator.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(McpRuntimeRetirementRegistryInner {
                entries: Mutex::new(Vec::new()),
                failures: Mutex::new(Vec::new()),
                failure_callback: Mutex::new(None),
            }),
        })
    }

    /// Installs the runtime-owned failure seam. The callback is invoked
    /// outside every registry mutex and therefore may begin runtime drain.
    pub(crate) fn install_failure_callback(&self, callback: &McpRetirementFailureCallback) {
        let existing = {
            let mut callback_slot = self
                .inner
                .failure_callback
                .lock()
                .expect("MCP retirement callback lock poisoned");
            *callback_slot = Some(callback.clone());
            self.inner
                .failures
                .lock()
                .expect("MCP retirement failure lock poisoned")
                .clone()
        };
        for (server_id, error) in existing {
            callback(format!("MCP server {server_id}: {error}"));
        }
    }

    fn register(&self, generation: Arc<McpRuntimeGenerationInner>) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .expect("MCP retirement lock poisoned");
        if !entries.iter().any(|entry| Arc::ptr_eq(entry, &generation)) {
            entries.push(generation);
        }
    }

    fn record_failure(&self, server_id: &McpServerId, error: &str) {
        let callback = {
            let callback_slot = self
                .inner
                .failure_callback
                .lock()
                .expect("MCP retirement callback lock poisoned");
            let mut failures = self
                .inner
                .failures
                .lock()
                .expect("MCP retirement failure lock poisoned");
            let failure = (server_id.clone(), error.to_owned());
            if failures.contains(&failure) {
                None
            } else {
                failures.push(failure);
                callback_slot.clone()
            }
        };
        if let Some(callback) = callback {
            callback(format!("MCP server {server_id}: {error}"));
        }
    }

    fn failure_diagnostics(&self) -> Vec<String> {
        let mut failures = self
            .inner
            .failures
            .lock()
            .expect("MCP retirement failure lock poisoned")
            .clone();
        failures.sort();
        failures
            .into_iter()
            .map(|(server_id, error)| format!("MCP server {server_id}: {error}"))
            .collect()
    }

    fn snapshot(&self) -> Vec<Arc<McpRuntimeGenerationInner>> {
        self.inner
            .entries
            .lock()
            .expect("MCP retirement lock poisoned")
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.inner
            .entries
            .lock()
            .expect("MCP retirement lock poisoned")
            .len()
    }

    fn reap(&self) {
        let mut entries = self
            .inner
            .entries
            .lock()
            .expect("MCP retirement lock poisoned");
        entries.retain(|entry| !entry.physical_settlement_proven());
    }

    /// Waits for retired generations that are already free of execution
    /// leases. This is used after successful reload to keep the retired list
    /// bounded without waiting for legitimate background owners.
    pub(crate) async fn settle_ready(&self) -> Result<(), Vec<String>> {
        for generation in self.snapshot() {
            if generation.can_close() {
                generation.wait_close_attempt().await;
            }
        }
        self.reap();
        let failures = self.failure_diagnostics();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }

    /// Waits for every retired generation to settle. Runtime drain calls this
    /// only after all attempt/background owners have settled.
    pub(crate) async fn settle_all(&self) -> Vec<String> {
        for generation in self.snapshot() {
            generation.wait_close_attempt().await;
        }
        self.reap();
        self.failure_diagnostics()
    }
}

/// The publication/candidate owner of one physical MCP runtime generation.
///
/// This owner is moved from a prepared capability candidate into the
/// coordinator's published generation state. Dropping it retires the
/// generation; the physical runtime closes only after every explicit
/// execution lease has settled.
pub(crate) struct McpRuntimeGeneration {
    inner: Arc<McpRuntimeGenerationInner>,
}

struct McpRuntimeGenerationInner {
    server_id: McpServerId,
    /// The stable connection owner of this server (Issue #205).
    ///
    /// A published capability generation owns one connection, and the
    /// connection owns whichever transport generation is currently
    /// authoritative. Executors bind to the connection, so a replaced
    /// transport is served immediately by every already-admitted tool
    /// without touching published capability knowledge.
    connection: Arc<connection::McpConnection>,
    retirement: Weak<McpRuntimeRetirementRegistryInner>,
    handle: tokio::runtime::Handle,
    lifecycle_admission: Mutex<Option<LifecycleAdmission>>,
    state: Mutex<McpRuntimeGenerationState>,
    close_finished: tokio::sync::Notify,
}

#[allow(clippy::struct_excessive_bools)]
struct McpRuntimeGenerationState {
    retired: bool,
    execution_leases: usize,
    close_started: bool,
    /// The complete close/retirement terminal outcome was published,
    /// whether successfully or with a terminal settlement error. This is not
    /// proof of physical settlement; that fact is represented separately
    /// below.
    close_attempt_finished: bool,
    /// True only when the owned physical MCP boundary was proven terminal.
    physical_settlement_proven: bool,
    /// Terminal evidence retained when physical settlement is unproven.
    terminal_failure: Option<String>,
}

/// A non-owning binding used by an MCP executor. It does not keep a physical
/// generation alive by itself; each execution acquires an explicit lease.
#[derive(Clone)]
pub(crate) struct McpRuntimeBinding {
    inner: Arc<McpRuntimeGenerationInner>,
}

/// One explicit physical-runtime execution lease.
pub(crate) struct McpRuntimeLease {
    inner: Arc<McpRuntimeGenerationInner>,
}

/// The leases captured by one detached background execution. Keeping this
/// value in the runner makes dispatch ownership explicit even while the
/// originating attempt and capability snapshot have settled.
#[derive(Default)]
pub(crate) struct McpRuntimeLeaseSet {
    #[allow(dead_code)]
    leases: Vec<McpRuntimeLease>,
}

/// The immutable physical-lease authority paired with one published
/// capability generation. Holding this authority does not itself retain an
/// execution lease; an admitted attempt or detached execution acquires one
/// from this exact generation rather than consulting mutable coordinator
/// state later.
#[derive(Clone, Default)]
pub(crate) struct McpRuntimeLeaseAuthority {
    bindings: Arc<[McpRuntimeBinding]>,
}

impl McpRuntimeLeaseAuthority {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn from_generations(generations: &[McpRuntimeGeneration]) -> Self {
        Self {
            bindings: generations
                .iter()
                .map(McpRuntimeGeneration::binding)
                .collect::<Vec<_>>()
                .into(),
        }
    }

    pub(crate) fn acquire(&self) -> Option<McpRuntimeLeaseSet> {
        let mut leases = Vec::with_capacity(self.bindings.len());
        for binding in self.bindings.iter() {
            leases.push(binding.acquire_lease()?);
        }
        Some(McpRuntimeLeaseSet { leases })
    }
}

impl std::fmt::Debug for McpRuntimeGeneration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpRuntimeGeneration")
            .field("server_id", &self.inner.server_id)
            .field("retired", &self.is_retired())
            .field("execution_leases", &self.execution_lease_count())
            .finish()
    }
}

impl McpRuntimeGeneration {
    /// The publication owner of one MCP connection.
    ///
    /// The connection — not a single transport — is what a published
    /// capability generation owns. Retiring this owner closes the connection
    /// and every transport generation it ever established.
    pub(crate) fn from_connection(
        server_id: McpServerId,
        connection: Arc<connection::McpConnection>,
        lifecycle_admission: Option<LifecycleAdmission>,
        handle: tokio::runtime::Handle,
        retirement: &Arc<McpRuntimeRetirementRegistry>,
    ) -> Self {
        Self {
            inner: Arc::new(McpRuntimeGenerationInner {
                server_id,
                connection,
                retirement: Arc::downgrade(&retirement.inner),
                handle,
                lifecycle_admission: Mutex::new(lifecycle_admission),
                state: Mutex::new(McpRuntimeGenerationState {
                    retired: false,
                    execution_leases: 0,
                    close_started: false,
                    close_attempt_finished: false,
                    physical_settlement_proven: false,
                    terminal_failure: None,
                }),
                close_finished: tokio::sync::Notify::new(),
            }),
        }
    }

    /// The server identity this published generation serves.
    pub(crate) fn server_id(&self) -> &McpServerId {
        &self.inner.server_id
    }

    /// The stable connection owner of this published generation.
    pub(crate) fn connection(&self) -> &Arc<connection::McpConnection> {
        &self.inner.connection
    }

    pub(crate) fn binding(&self) -> McpRuntimeBinding {
        McpRuntimeBinding {
            inner: self.inner.clone(),
        }
    }

    pub(crate) fn retire(&self) {
        let first_retirement = {
            let mut state = self
                .inner
                .state
                .lock()
                .expect("MCP generation lock poisoned");
            if state.retired {
                false
            } else {
                state.retired = true;
                true
            }
        };
        if first_retirement && let Some(retirement) = self.inner.retirement.upgrade() {
            McpRuntimeRetirementRegistry { inner: retirement }.register(self.inner.clone());
        }
        self.inner.maybe_schedule_close();
    }

    pub(crate) async fn retire_and_close(self) -> Option<String> {
        self.retire();
        self.inner.wait_close_attempt().await;
        self.inner.terminal_failure()
    }

    fn is_retired(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("MCP generation lock poisoned")
            .retired
    }

    pub(crate) fn execution_lease_count(&self) -> usize {
        self.inner.execution_lease_count()
    }
}

impl Drop for McpRuntimeGeneration {
    fn drop(&mut self) {
        self.retire();
    }
}

impl McpRuntimeBinding {
    pub(crate) fn acquire_lease(&self) -> Option<McpRuntimeLease> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("MCP generation lock poisoned");
        if state.close_started || state.close_attempt_finished {
            return None;
        }
        state.execution_leases += 1;
        Some(McpRuntimeLease {
            inner: self.inner.clone(),
        })
    }

    pub(crate) fn connection(&self) -> &Arc<connection::McpConnection> {
        &self.inner.connection
    }
}

impl McpRuntimeLease {
    fn try_clone(&self) -> Option<Self> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("MCP generation lock poisoned");
        if state.close_started || state.close_attempt_finished {
            return None;
        }
        state.execution_leases += 1;
        Some(Self {
            inner: self.inner.clone(),
        })
    }
}

impl Drop for McpRuntimeLease {
    fn drop(&mut self) {
        {
            let mut state = self
                .inner
                .state
                .lock()
                .expect("MCP generation lock poisoned");
            state.execution_leases = state
                .execution_leases
                .checked_sub(1)
                .expect("MCP execution lease count underflow");
        }
        self.inner.maybe_schedule_close();
    }
}

impl McpRuntimeLeaseSet {
    pub(crate) fn try_clone(&self) -> Option<Self> {
        let mut leases = Vec::with_capacity(self.leases.len());
        for lease in &self.leases {
            leases.push(lease.try_clone()?);
        }
        Some(Self { leases })
    }

    /// Whether one of these leases resolves to the given published
    /// transport (deterministic ownership tests only).
    #[cfg(test)]
    pub(crate) fn contains_runtime(&self, runtime: &Arc<McpServerRuntime>) -> bool {
        self.leases.iter().any(|lease| {
            lease
                .inner
                .connection
                .published_runtime()
                .is_some_and(|published| Arc::ptr_eq(&published, runtime))
        })
    }
}

impl McpRuntimeGenerationInner {
    fn execution_lease_count(&self) -> usize {
        self.state
            .lock()
            .expect("MCP generation lock poisoned")
            .execution_leases
    }

    fn can_close(&self) -> bool {
        let state = self.state.lock().expect("MCP generation lock poisoned");
        state.retired && state.execution_leases == 0
    }

    fn close_attempt_finished(&self) -> bool {
        self.state
            .lock()
            .expect("MCP generation lock poisoned")
            .close_attempt_finished
    }

    fn physical_settlement_proven(&self) -> bool {
        self.state
            .lock()
            .expect("MCP generation lock poisoned")
            .physical_settlement_proven
    }

    fn terminal_failure(&self) -> Option<String> {
        self.state
            .lock()
            .expect("MCP generation lock poisoned")
            .terminal_failure
            .clone()
    }

    fn maybe_schedule_close(self: &Arc<Self>) {
        let admission = {
            let mut state = self.state.lock().expect("MCP generation lock poisoned");
            if !state.retired || state.execution_leases != 0 || state.close_started {
                return;
            }
            state.close_started = true;
            self.lifecycle_admission
                .lock()
                .expect("MCP lifecycle admission lock poisoned")
                .take()
        };
        let inner = self.clone();
        self.handle.spawn(async move {
            // Closing the connection drives every transport generation it
            // established — the current one and any retired-but-unclosed
            // predecessor — to the same physical settlement proof.
            let failures = inner.connection.close().await;
            let failure = if failures.is_empty() {
                None
            } else {
                Some(failures.join("; "))
            };
            {
                let mut state = inner.state.lock().expect("MCP generation lock poisoned");
                state.physical_settlement_proven = failure.is_none();
                state.terminal_failure.clone_from(&failure);
            }
            if let Some(retirement) = inner.retirement.upgrade() {
                let registry = McpRuntimeRetirementRegistry { inner: retirement };
                if let Some(failure) = failure {
                    // This hook is intentionally after McpServerRuntime::close
                    // returned and before the registry/callback publication.
                    // The completion signal below must remain later than this
                    // entire terminal-outcome publication sequence.
                    #[cfg(test)]
                    if let Some(runtime) = inner.connection.published_runtime() {
                        runtime.wait_before_retirement_failure_publication().await;
                    }
                    registry.record_failure(&inner.server_id, &failure);
                }
                registry.reap();
            }
            // The lifecycle admission is part of the physical owner's
            // terminal handoff. Release it only after generation state,
            // registry evidence, and the runtime fencing callback are all
            // published, then make completion observable as the final step.
            drop(admission);
            {
                let mut state = inner.state.lock().expect("MCP generation lock poisoned");
                // `close_attempt_finished` is deliberately the last
                // publication. `wait_close_attempt` therefore cannot observe
                // completion before the complete terminal outcome is visible.
                state.close_attempt_finished = true;
            }
            inner.close_finished.notify_waiters();
        });
    }

    /// Waits for the complete terminal outcome publication. The close task
    /// sets `close_attempt_finished` only after generation state, retirement
    /// failure evidence, callback fencing, and lifecycle-admission release
    /// have all completed; this is stronger than merely waiting for the
    /// underlying `close()` future to return.
    async fn wait_close_attempt(&self) {
        loop {
            if self.close_attempt_finished() {
                return;
            }
            let notified = self.close_finished.notified();
            tokio::pin!(notified);
            if self.close_attempt_finished() {
                return;
            }
            notified.await;
        }
    }
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
        failure_publication_entered: tokio::sync::Notify,
        failure_publication_release: tokio::sync::Notify,
        state: Mutex<CloseState>,
    }

    #[derive(Debug, Default)]
    #[allow(clippy::struct_excessive_bools)]
    struct CloseState {
        entered: bool,
        parks: bool,
        released: bool,
        failure: Option<String>,
        pause_before_failure_publication: bool,
        failure_publication_entered: bool,
        failure_publication_released: bool,
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

        /// A failing close that parks after `McpServerRuntime::close()` has
        /// returned and before the retirement registry publishes the failure.
        /// This is the deterministic race seam for the generation completion
        /// happens-before contract.
        pub(crate) fn failing_before_failure_publication(diagnostic: &str) -> Self {
            Self {
                state: Mutex::new(CloseState {
                    failure: Some(diagnostic.to_owned()),
                    pause_before_failure_publication: true,
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

        /// Parks the generation close task before retirement failure
        /// publication when the specialized failing probe is armed.
        pub(crate) async fn wait_before_failure_publication(&self) {
            {
                let mut state = self.state.lock().expect("close probe lock");
                if !state.pause_before_failure_publication || state.failure_publication_released {
                    return;
                }
                state.failure_publication_entered = true;
            }
            self.failure_publication_entered.notify_waiters();
            loop {
                let released = self.failure_publication_release.notified();
                tokio::pin!(released);
                released.as_mut().enable();
                if self
                    .state
                    .lock()
                    .expect("close probe lock")
                    .failure_publication_released
                {
                    return;
                }
                released.await;
            }
        }

        /// Waits until the close task reaches the pre-publication park.
        pub(crate) async fn wait_before_failure_publication_entered(&self) {
            loop {
                let entered = self.failure_publication_entered.notified();
                tokio::pin!(entered);
                entered.as_mut().enable();
                if self
                    .state
                    .lock()
                    .expect("close probe lock")
                    .failure_publication_entered
                {
                    return;
                }
                entered.await;
            }
        }

        /// Releases the pre-retirement-publication close park.
        pub(crate) fn release_before_failure_publication(&self) {
            self.state
                .lock()
                .expect("close probe lock")
                .failure_publication_released = true;
            self.failure_publication_release.notify_waiters();
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
        // The generic protocol-corruption observation seam. Only the stdio
        // transport installs the tee (it is the byte stream the seam
        // observes); the recorder exists for every transport so the
        // violation check is uniform. Streamable HTTP surfaces corruption
        // through its own worker transport and is unchanged by this seam.
        let protocol_violation = framing::ProtocolViolationRecorder::new();
        // Set by the Streamable HTTP arm only: stdio owns no per-request
        // local HTTP state.
        let mut request_ownership: Option<Arc<streamable_http::McpHttpRequestOwnership>> = None;
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
                let transport = rmcp::transport::async_rw::AsyncRwTransport::new_client(
                    // The observation tee: rmcp remains the one
                    // framing/protocol authority over exactly these
                    // bytes; the tee only records a confirmed
                    // structurally invalid message and then ends the
                    // stream, so the violation becomes a rustX fact.
                    framing::ViolationObservingReader::new(stdout, protocol_violation.clone()),
                    stdin,
                );
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
                    Ok(service) => {
                        // Fail closed: a structurally invalid peer message
                        // during the handshake fails the connection even if
                        // rmcp's own peer-facing recovery let the handshake
                        // complete — the violation is a rustX fact, never
                        // peer-only traffic.
                        if let Some(violation) = protocol_violation.violation() {
                            return Err(settle_connect_failure(
                                Some(&process),
                                protocol_violation_error(server_id, &violation),
                            )
                            .await);
                        }
                        service
                    }
                    Err(error) => {
                        let error = match protocol_violation.violation() {
                            Some(violation) => protocol_violation_error(server_id, &violation),
                            None => error,
                        };
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
                // rustX supplies the HTTP client rather than taking rmcp's
                // default one, because the in-flight HTTP request of a
                // dispatched `tools/call` is local ownership a cancelled
                // call has to terminate and prove released. The registry
                // returned here is that authority; it belongs to exactly
                // this connection generation.
                let (client, ownership) = streamable_http::McpHttpClient::new()?;
                request_ownership = Some(ownership);
                let transport = rmcp::transport::StreamableHttpClientTransport::with_client(
                    client,
                    transport_config,
                );
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
            protocol_violation,
            request_ownership,
            transport_failure: Mutex::new(None),
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
                    let error = match runtime.protocol_violation.violation() {
                        Some(violation) => protocol_violation_error(server_id, &violation),
                        None => error,
                    };
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

    /// The proven reason this transport generation can no longer serve MCP
    /// traffic, when one exists (Issue #205).
    ///
    /// This is the connection owner's health arbitration input, and it is
    /// deliberately made of proofs only:
    ///
    /// - the runtime was closed (drain, generation retirement, or the
    ///   fail-closed protocol poison);
    /// - a confirmed structurally invalid MCP/JSON-RPC peer message was
    ///   observed by the framing seam;
    /// - an earlier operation observed a transport-class failure on this
    ///   generation.
    ///
    /// A tool call that merely *failed* (a remote error result, an
    /// unsupported response shape) proves nothing about the transport and
    /// never appears here.
    pub(crate) fn unusable_reason(&self) -> Option<String> {
        if let Some(violation) = self.protocol_violation.violation() {
            return Some(protocol_violation_call_diagnostic(
                &self.server_id,
                &violation,
            ));
        }
        if let Some(failure) = self
            .transport_failure
            .lock()
            .expect("MCP transport failure lock poisoned")
            .clone()
        {
            return Some(failure);
        }
        if self.closed.load(Ordering::Acquire) {
            return Some(format!(
                "the MCP server runtime of '{}' is closed",
                self.server_id
            ));
        }
        None
    }

    /// Records the first proof that this transport generation is unusable.
    ///
    /// Monotonic and idempotent: the first proof wins, and every later loss
    /// signal for the same generation is absorbed. Repeated transport-loss
    /// observations therefore cannot produce a second retirement, a second
    /// replacement, or a second canonical settlement.
    fn note_transport_loss(&self, reason: &str) {
        let mut failure = self
            .transport_failure
            .lock()
            .expect("MCP transport failure lock poisoned");
        if failure.is_none() {
            *failure = Some(bound_error(reason));
        }
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
        // A generation that already violated the protocol never serves a
        // healthy catalog, and the precise violation diagnostic outranks
        // the generic closed state.
        if let Some(violation) = self.protocol_violation.violation() {
            self.poison_after_protocol_violation().await;
            return Err(protocol_violation_error(&self.server_id, &violation));
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(McpError::Execution(
                "the MCP server runtime is closed".to_owned(),
            ));
        }
        let tools = match self.peer.list_all_tools().await {
            Ok(tools) => {
                // Never freeze a catalog from a connection that has already
                // violated the protocol.
                if let Some(violation) = self.protocol_violation.violation() {
                    self.poison_after_protocol_violation().await;
                    return Err(protocol_violation_error(&self.server_id, &violation));
                }
                tools
            }
            Err(error) => {
                if let Some(violation) = self.protocol_violation.violation() {
                    self.poison_after_protocol_violation().await;
                    return Err(protocol_violation_error(&self.server_id, &violation));
                }
                // A transport-class catalog failure proves this generation
                // unusable; a correlated remote `tools/list` error does not
                // and leaves the transport healthy.
                if let rmcp::service::ServiceError::TransportClosed
                | rmcp::service::ServiceError::TransportSend(_) = &error
                {
                    self.note_transport_loss(&error.to_string());
                }
                return Err(McpError::Discovery(bound_error(&error.to_string())));
            }
        };
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
        // Drain owns the per-request local HTTP control primitives this
        // generation created (Issue #205): every request rustX still owns
        // locally is terminated here, before the service shutdown below
        // joins the transport worker that holds their futures. No such
        // primitive can outlive this close.
        if let Some(ownership) = &self.request_ownership {
            ownership.terminate_all();
        }
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

    /// Parks the generation close task after this runtime has returned from
    /// `close`, but before the generation's retirement registry and runtime
    /// failure callback are published. The generation completion boundary is
    /// intentionally later than this hook.
    #[cfg(test)]
    async fn wait_before_retirement_failure_publication(&self) {
        if let Some(probe) = self.close_probe.get() {
            probe.wait_before_failure_publication().await;
        }
    }

    /// Fails closed after a confirmed peer protocol violation: this
    /// generation stops accepting new remote work (`closed`) and its owned
    /// stdio process is asked to retire. Physical settlement proof still
    /// goes through the ordinary `close()`/generation-retirement
    /// ownership — this only fences the protocol boundary, promptly.
    async fn poison_after_protocol_violation(&self) {
        self.closed.store(true, Ordering::Release);
        if let Some(process) = &self.process {
            process.lock().await.request_shutdown();
        }
    }

    /// The failed tool result for a confirmed protocol violation observed
    /// **before dispatch**, when the observation seam recorded one. Also
    /// poisons the generation (see [`Self::poison_after_protocol_violation`]).
    async fn protocol_violation_failure(
        &self,
        context: &ToolExecutionContext<'_>,
        started: Instant,
    ) -> Option<ToolExecutionResult> {
        let violation = self.poisoned_protocol_violation().await?;
        Some(failed_mcp(&violation, context, started))
    }

    /// Poisons the generation after a confirmed peer protocol violation and
    /// returns the bounded call diagnostic, when the observation seam
    /// recorded one. Call sites choose the terminal status: a violation
    /// observed before dispatch is a known `Failed`; one observed after
    /// dispatch leaves the external outcome unknown.
    async fn poisoned_protocol_violation(&self) -> Option<String> {
        let violation = self.protocol_violation.violation()?;
        self.poison_after_protocol_violation().await;
        Some(protocol_violation_call_diagnostic(
            &self.server_id,
            &violation,
        ))
    }

    /// Executes one remote `tools/call` on this transport generation.
    ///
    /// # The external-effect frontier (Issue #205)
    ///
    /// The frontier is the **exact instant
    /// [`rmcp::Peer::send_cancellable_request`] returns `Ok`**, and this is
    /// the strongest fact rustX can establish over rmcp's request path:
    ///
    /// ```text
    /// send_cancellable_request(..) -> Err
    ///     the request was never accepted by the service event loop, so it
    ///     was never serialized and never written to the transport
    ///     => before the frontier: no remote side effect was possible
    ///
    /// send_cancellable_request(..) -> Ok
    ///     the request is enqueued on the peer's outbound channel; the
    ///     service loop may already have serialized and written it
    ///     => at/after the frontier: a remote side effect may have occurred
    /// ```
    ///
    /// rmcp's `Err` is produced by exactly one condition — the outbound
    /// `mpsc` send failing because the service event loop is gone — so it
    /// proves the message was never enqueued. `Ok` proves only enqueueing:
    /// the write happens in the service loop's own spawned send task, and
    /// nothing observable to this call distinguishes "not yet written" from
    /// "written and being executed". The frontier is therefore drawn at
    /// enqueue, deliberately conservatively, rather than at any convenient
    /// polling behaviour of the response future.
    ///
    /// Past the frontier, only a **correlated remote response** — a
    /// `CallToolResult`, or a JSON-RPC error answering this request id —
    /// proves remote terminality. Everything else (transport loss, a
    /// poisoned generation, a cancellation notification rmcp acknowledged,
    /// an abandoned response channel) leaves the external outcome unknown
    /// under the Issue #202 contract.
    ///
    /// # Cancellation
    ///
    /// Cancellation intent delivered before the frontier settles the call as
    /// a proven [`ToolExecutionStatus::Cancelled`]: no request exists, so no
    /// remote effect is possible. After the frontier, the executor sends
    /// `notifications/cancelled` — the strongest cancellation the MCP
    /// protocol defines — and then **keeps the response channel** so a
    /// remote result that beat the cancellation inside rmcp still wins and
    /// is reported as proven remote terminality. Only when no correlated
    /// remote response exists is the outcome unknown; dropping the response
    /// future is never used as evidence of anything.
    async fn call(
        &self,
        remote_name: &str,
        arguments: serde_json::Value,
        context: &ToolExecutionContext<'_>,
        generation: u64,
    ) -> ToolExecutionResult {
        let _call_gate = self.call_gate.read().await;
        let started = Instant::now();
        // ---------- before the external-effect frontier ----------
        //
        // Every rejection below happens while no request exists, so each one
        // is a proven ordinary outcome: rustX can prove no remote side
        // effect was possible, and claiming an unknown external outcome here
        // would be dishonest in the opposite direction.
        //
        // A generation that already violated the protocol never serves
        // another call as healthy, whether the violation arrived during an
        // earlier call or while the connection was idle.
        if let Some(failure) = self.protocol_violation_failure(context, started).await {
            return failure;
        }
        if self.closed.load(Ordering::Acquire) {
            return failed_mcp("MCP server runtime is closed", context, started);
        }
        let serde_json::Value::Object(arguments) = arguments else {
            return failed_mcp("MCP tool arguments must be a JSON object", context, started);
        };
        // The pre-frontier cancellation checkpoint. Without it an execution
        // whose cancellation/deadline intent already won arbitration would
        // still dispatch a fresh remote request and then have to report the
        // external ambiguity it just created. Observing intent here settles
        // the call as a proven cancellation instead.
        if context.cancellation.is_cancelled() {
            return mcp_empty_terminal(
                ToolExecutionStatus::Cancelled {
                    reason: context.cancellation.reason(),
                    phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
                },
                context,
                started,
            );
        }
        let params = CallToolRequestParams::new(remote_name.to_owned()).with_arguments(arguments);
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
        let mut handle = match self
            .peer
            .send_cancellable_request(request, PeerRequestOptions::no_options())
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                // rmcp rejects a request here only when its service event
                // loop is gone, so the request was never enqueued, never
                // serialized, and never written: the frontier was not
                // crossed and this is an ordinary failure. The generation
                // itself is proven unusable.
                self.note_transport_loss(&format!(
                    "the MCP service of '{}' is gone: {error}",
                    self.server_id
                ));
                return failed_mcp(
                    &format!(
                        "the MCP request was refused before dispatch and never reached the \
                         transport ({}): {}",
                        self.generation_tag(generation),
                        bound_error(&error.to_string())
                    ),
                    context,
                    started,
                );
            }
        };
        // ---------- the external-effect frontier is crossed ----------
        // Subscribing after dispatch is unavoidable — rmcp mints the token
        // inside the request — so the router, not the subscription, is what
        // makes an immediately answered progress notification observable.
        let mut progress = self
            .handler
            .progress
            .subscribe(handle.progress_token.clone());
        let response = loop {
            tokio::select! {
                biased;
                // A correlated remote response always outranks cancellation
                // intent: it is proven remote terminality, and this bias is
                // the deterministic winner of the response-versus-
                // cancellation race.
                response = &mut handle.rx => break response,
                () = context.cancellation.cancelled() => {
                    // Liveness evidence that already arrived is never
                    // discarded by arbitration: it genuinely happened before
                    // the cancellation intent won.
                    for notification in progress.drain() {
                        report_remote_progress(context, notification);
                    }
                    drop(progress);
                    return self
                        .settle_post_frontier_cancellation(handle, context, started, generation)
                        .await;
                }
                progress_item = progress.recv() => {
                    // Genuine remote liveness evidence, forwarded through the
                    // one generic progress seam. It refreshes the Agent
                    // Loop's idle watchdog and can never extend the generic
                    // hard deadline, which this adapter neither owns nor
                    // observes.
                    if let Some(progress_item) = progress_item {
                        report_remote_progress(context, progress_item);
                    }
                }
            }
        };
        // A correlated response winning the biased arbitration must not
        // silently discard liveness evidence the peer already delivered on
        // the same ordered transport: those notifications genuinely arrived
        // before the response, and they are reported before the terminal
        // result so the durable fact order stays terminal-last.
        for notification in progress.drain() {
            report_remote_progress(context, notification);
        }
        drop(progress);
        // A response that won the biased arbitration on its own is a pure
        // response-plane outcome: this call never terminated its own
        // transport-level request, so any transport-class failure here is
        // genuine evidence about the connection generation.
        self.classify_post_frontier_response(
            McpResponseOutcome::observed(response),
            context,
            started,
            generation,
            false,
        )
        .await
    }

    /// The bounded transport-generation tag carried by MCP diagnostics.
    ///
    /// It is what correlates one call's outcome with one concrete negotiated
    /// transport authority: an ambiguous call names the generation it died
    /// on, and a later successful call names the replacement.
    fn generation_tag(&self, generation: u64) -> String {
        format!(
            "MCP server '{}' connection generation {generation}",
            self.server_id
        )
    }

    /// Settles one cancellation intent that arrived after the effect
    /// frontier (Issue #205).
    ///
    /// # Two planes, and only one of them holds settlement authority
    ///
    /// ```text
    /// remote control plane          local ownership plane
    ///   notifications/cancelled       this request's in-flight HTTP
    ///   best-effort, unacknowledged   request (Streamable HTTP only)
    ///   MAY never complete            terminated and *proven released*
    ///                                 by rustX-owned state alone
    /// ```
    ///
    /// `notifications/cancelled` is the strongest cancellation the MCP
    /// protocol defines, and the protocol defines no acknowledgement of it,
    /// so sending it is never proof that the remote operation stopped. It is
    /// also not allowed to hold local settlement hostage: over a transport
    /// whose client cannot make progress on an outbound send while a
    /// previous request is outstanding, awaiting that send would leave this
    /// branch pending indefinitely and hand the call's only bound to the
    /// generic Issue #204 settlement-control guard. That is architecturally
    /// wrong — the MCP executor's own settlement plane must terminate.
    ///
    /// So the send is *raced*, never awaited as a prerequisite, and the
    /// bound is rustX's own local request ownership: terminating this
    /// request's HTTP request and awaiting the release proof of its POST
    /// future and response body depends on no remote response, no protocol
    /// acknowledgement, and no timer. Over stdio there is no such local
    /// half, so the termination is already settled and the race collapses
    /// to the cancellation send and the response channel.
    ///
    /// # Arbitration
    ///
    /// A **correlated remote response always wins**: it is proven remote
    /// terminality, and the response channel is deliberately retained across
    /// the whole path so a result that beat the cancellation inside rmcp
    /// still becomes the call's outcome. rmcp resolves this request's local
    /// responder in the same event-loop step in which it reports the
    /// notification's send outcome, so after an observed send the response
    /// channel is checked without waiting; and rustX's own local
    /// termination resolves that responder too, through the ordinary
    /// transport-send failure path. Only when no correlated remote response
    /// can be established is the external outcome unknown. Dropping a future
    /// is never used as evidence of anything.
    ///
    /// Nothing is reported until local ownership is settled: that await is
    /// the last thing this function does before it builds a result, so
    /// `Unconfirmed` keeps its Issue #204 meaning — every rustX-owned local
    /// activity of this invocation is over, and only the remote effect
    /// remains uncertain.
    async fn settle_post_frontier_cancellation(
        &self,
        mut handle: rmcp::service::RequestHandle<RoleClient>,
        context: &ToolExecutionContext<'_>,
        started: Instant,
        generation: u64,
    ) -> ToolExecutionResult {
        // Armed first and synchronously: from here on this request cannot
        // reach the network even if its POST had not started yet.
        let termination = self.terminate_local_request(&handle.id);
        let outcome = self
            .arbitrate_post_frontier_cancellation(&mut handle, &termination)
            .await;
        // The local ownership proof. Bounded by rustX-owned state alone, and
        // a precondition of *reporting*, never a competitor to it.
        termination.settled().await;
        match outcome {
            // A correlated remote response beat the cancellation: remote
            // terminality is proven and the remote's own outcome is
            // authoritative. The generic lifecycle then applies its
            // documented rule to this proven settlement.
            PostFrontierCancellation::Correlated(response) => {
                self.classify_post_frontier_response(
                    response,
                    context,
                    started,
                    generation,
                    termination.terminated_local_request(),
                )
                .await
            }
            // No correlated remote response exists. The request may have
            // executed remotely and its final external outcome cannot be
            // established.
            PostFrontierCancellation::Uncorrelated {
                requested,
                observed,
            } => {
                // A transport-class failure observed here is still genuine
                // evidence that the generation is unusable — unless this
                // call terminated its own transport-level request, which
                // fully explains it.
                if !termination.terminated_local_request() {
                    match &observed {
                        Some(McpResponseOutcome::Answered(answered)) => {
                            if let Err(error) = answered.as_ref()
                                && is_transport_loss(error)
                            {
                                self.note_transport_loss(&error.to_string());
                            }
                        }
                        Some(McpResponseOutcome::ChannelEnded) => {
                            self.note_transport_loss(
                                "the MCP response channel ended without a correlated response",
                            );
                        }
                        _ => {}
                    }
                }
                mcp_empty_terminal(
                    post_dispatch_cancellation_status(requested, &self.generation_tag(generation)),
                    context,
                    started,
                )
            }
        }
    }

    /// Terminates rustX's own local ownership of one dispatched request.
    ///
    /// Stdio has no per-request local ownership to terminate, so it settles
    /// immediately; Streamable HTTP terminates the request's in-flight HTTP
    /// request and returns the proof that its POST future and response body
    /// have been dropped.
    fn terminate_local_request(
        &self,
        id: &rmcp::model::RequestId,
    ) -> streamable_http::LocalRequestTermination {
        self.request_ownership.as_ref().map_or(
            streamable_http::LocalRequestTermination::Settled,
            |ownership| ownership.terminate(id),
        )
    }

    /// Races the best-effort protocol cancellation against the two facts
    /// that can settle this call locally.
    async fn arbitrate_post_frontier_cancellation(
        &self,
        handle: &mut rmcp::service::RequestHandle<RoleClient>,
        termination: &streamable_http::LocalRequestTermination,
    ) -> PostFrontierCancellation {
        let notification =
            rmcp::model::CancelledNotification::new(rmcp::model::CancelledNotificationParam::new(
                Some(handle.id.clone()),
                Some("rustX execution cancellation".to_owned()),
            ));
        let cancel = self.peer.send_notification(notification.into());
        tokio::pin!(cancel);
        // The non-correlated response-channel fact, when the channel
        // resolved with one. rmcp's synthesized local cancellation outcome
        // and a transport failure both land here: neither proves remote
        // terminality, and neither ends the arbitration.
        let mut observed: Option<McpResponseOutcome> = None;
        let requested = loop {
            tokio::select! {
                biased;
                // Proven remote terminality outranks every local fact.
                response = &mut handle.rx, if observed.is_none() => {
                    let response = McpResponseOutcome::observed(response);
                    if response.is_correlated_remote_response() {
                        return PostFrontierCancellation::Correlated(response);
                    }
                    observed = Some(response);
                }
                outcome = &mut cancel => break match outcome {
                    Ok(()) => RemoteCancellation::Sent,
                    Err(error) => RemoteCancellation::Failed(bound_error(&error.to_string())),
                },
                // The local ownership plane reached its terminal decision
                // first, so the best-effort remote control send is abandoned
                // rather than allowed to hold settlement.
                //
                // The guard is what keeps this honest. This arm exists only
                // when rustX actually had a live local request to terminate
                // — the Streamable HTTP case the bound is for. A transport
                // that owns no per-request local state has no local
                // settlement *event* here, only the trivially-already-settled
                // fact, and racing that would abandon every cancellation send
                // before it could leave.
                () = termination.settled(), if termination.terminated_local_request() => {
                    break RemoteCancellation::Abandoned;
                }
            }
        };
        // The correlated response still wins if one exists: after an
        // observed cancellation send rmcp has already resolved this
        // request's responder, and rustX's own local termination resolves it
        // through the transport-send failure path. This is a check, never a
        // wait.
        if observed.is_none() {
            match handle.rx.try_recv() {
                Ok(response) => {
                    let response = McpResponseOutcome::Answered(Box::new(response));
                    if response.is_correlated_remote_response() {
                        return PostFrontierCancellation::Correlated(response);
                    }
                    observed = Some(response);
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    observed = Some(McpResponseOutcome::ChannelEnded);
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        PostFrontierCancellation::Uncorrelated {
            requested,
            observed,
        }
    }

    /// Classifies one post-frontier `tools/call` outcome (Issue #205).
    ///
    /// Only a correlated remote response proves remote terminality; every
    /// other outcome leaves the external result unknown.
    ///
    /// `terminated_local_request` records whether *this call* terminated its
    /// own transport-level request as part of settling. When it did, a
    /// transport-send failure for this request is explained by that
    /// termination and is therefore not independent evidence that the
    /// connection generation itself is dead: the Streamable HTTP session
    /// survives an aborted request, and retiring the generation over
    /// rustX's own cancellation would force a needless reconnect on the
    /// next call.
    async fn classify_post_frontier_response(
        &self,
        response: McpResponseOutcome,
        context: &ToolExecutionContext<'_>,
        started: Instant,
        generation: u64,
        terminated_local_request: bool,
    ) -> ToolExecutionResult {
        let response = match response {
            McpResponseOutcome::Answered(answered) => *answered,
            McpResponseOutcome::ChannelEnded => {
                if !terminated_local_request {
                    self.note_transport_loss(
                        "the MCP response channel ended without a correlated response",
                    );
                }
                // The observation tee ends the stream on a confirmed
                // violation, so a violation surfaced mid-call lands here as
                // a transport close. It is a protocol failure, not an
                // anonymous disconnect — and the generation is poisoned.
                if let Some(diagnostic) = self.poisoned_protocol_violation().await {
                    return mcp_empty_terminal(
                        ToolExecutionStatus::OutcomeUnknown {
                            detail: bound_error(&diagnostic),
                        },
                        context,
                        started,
                    );
                }
                // The transport closed after dispatch without a response:
                // the remote operation may have partially or fully completed.
                return mcp_empty_terminal(
                    ToolExecutionStatus::OutcomeUnknown {
                        detail: bound_error(&format!(
                            "MCP transport closed during tools/call without a response ({})",
                            self.generation_tag(generation)
                        )),
                    },
                    context,
                    started,
                );
            }
        };
        match response {
            Ok(ServerResult::CallToolResult(result)) => translate_result(result, context, started),
            Ok(ServerResult::InputRequiredResult(_)) => failed_mcp(
                "MCP input_required results are unsupported in M7",
                context,
                started,
            ),
            Ok(_) => failed_mcp("unexpected MCP tools/call response", context, started),
            // A JSON-RPC error answering this request id is a correlated
            // remote response: the remote produced a terminal outcome and it
            // is a known failure, not an ambiguity.
            Err(rmcp::service::ServiceError::McpError(error)) => {
                failed_mcp(&bound_error(&error.to_string()), context, started)
            }
            Err(error) => {
                if is_transport_loss(&error) && !terminated_local_request {
                    self.note_transport_loss(&error.to_string());
                }
                // The observation tee ends the stream on a confirmed
                // violation, so a violation surfaced mid-call can land here.
                // It is a protocol failure, not an anonymous disconnect —
                // and the generation is poisoned.
                if let Some(diagnostic) = self.poisoned_protocol_violation().await {
                    return mcp_empty_terminal(
                        ToolExecutionStatus::OutcomeUnknown {
                            detail: bound_error(&diagnostic),
                        },
                        context,
                        started,
                    );
                }
                mcp_empty_terminal(
                    ToolExecutionStatus::OutcomeUnknown {
                        detail: bound_error(&format!(
                            "the dispatched MCP tools/call produced no correlated remote \
                             response ({}): {error}",
                            self.generation_tag(generation)
                        )),
                    },
                    context,
                    started,
                )
            }
        }
    }
}

/// One post-frontier observation of a dispatched request's response channel.
///
/// The distinction is the whole post-frontier contract: `Answered` is a
/// correlated remote response — the only proof of remote terminality rustX
/// accepts — while `ChannelEnded` is the absence of one.
enum McpResponseOutcome {
    /// The peer answered this request id, with a result or a JSON-RPC error,
    /// or rmcp reported a service-level failure for it.
    ///
    /// Boxed because `ServerResult` is a large union of every MCP result
    /// shape, and this value travels through one `select!` state machine.
    Answered(Box<Result<ServerResult, rmcp::service::ServiceError>>),
    /// The response channel ended without a correlated response.
    ChannelEnded,
}

impl McpResponseOutcome {
    /// Whether this observation is a **correlated remote response**: a
    /// result, or a JSON-RPC error, that the peer produced for this request
    /// id. It is the only proof of remote terminality rustX accepts.
    ///
    /// rmcp's synthesized `ServiceError::Cancelled` is deliberately *not*
    /// one: it is a local outcome rmcp writes into the request's responder
    /// when it reports a cancellation notification's send outcome, and it
    /// says nothing about what the remote did.
    const fn is_correlated_remote_response(&self) -> bool {
        matches!(
            self,
            Self::Answered(answered)
                if matches!(
                    **answered,
                    Ok(_) | Err(rmcp::service::ServiceError::McpError(_))
                )
        )
    }

    /// The observation of one resolved response channel.
    fn observed(
        response: Result<
            Result<ServerResult, rmcp::service::ServiceError>,
            tokio::sync::oneshot::error::RecvError,
        >,
    ) -> Self {
        match response {
            Ok(answered) => Self::Answered(Box::new(answered)),
            Err(_) => Self::ChannelEnded,
        }
    }
}

/// How one post-frontier cancellation arbitration ended.
enum PostFrontierCancellation {
    /// A correlated remote response was observed, or the response channel
    /// proved none can arrive. Either way the response plane, not the
    /// cancellation plane, decides this call.
    Correlated(McpResponseOutcome),
    /// No correlated remote response exists. `requested` carries what became
    /// of the best-effort protocol cancellation, and `observed` the
    /// non-correlated response-channel fact when there was one.
    Uncorrelated {
        requested: RemoteCancellation,
        observed: Option<McpResponseOutcome>,
    },
}

/// What became of one call's best-effort `notifications/cancelled`.
///
/// None of these three is settlement evidence: remote cancellation is
/// unacknowledged by the protocol, so even `Sent` proves only that rustX
/// asked. They exist to make the call's diagnostic honest about *which*
/// remote control was actually attempted.
enum RemoteCancellation {
    /// The notification left rustX.
    Sent,
    /// The send itself failed.
    Failed(String),
    /// Local request ownership settled first, so the send was abandoned
    /// rather than allowed to hold local settlement authority.
    Abandoned,
}

/// Forwards one remote progress notification through the generic progress
/// seam.
///
/// The shared canonical normalization drops non-finite `completed`/`total`
/// values, so the remote value needs no adapter-local filter.
fn report_remote_progress(
    context: &ToolExecutionContext<'_>,
    notification: ProgressNotificationParam,
) {
    context
        .progress
        .report(bound_tool_progress(crate::tools::types::ToolProgress {
            message: notification.message,
            completed: Some(notification.progress),
            total: notification.total,
        }));
}

/// Whether one rmcp service failure is transport-class evidence that the
/// connection generation itself can no longer carry MCP traffic
/// (Issue #205).
///
/// A remote error *result* proves the opposite — the transport delivered a
/// correlated answer — and never appears here.
fn is_transport_loss(error: &rmcp::service::ServiceError) -> bool {
    matches!(
        error,
        rmcp::service::ServiceError::TransportClosed
            | rmcp::service::ServiceError::TransportSend(_)
    )
}

/// How many *unsubscribed* progress tokens the router may hold in its
/// pre-subscription window.
///
/// This bounds only the window between rmcp minting a request's progress
/// token and the dispatching call subscribing to it. The set of tokens that
/// appear on the wire is peer-controlled, so it must be bounded; the set of
/// tokens rustX is actually *waiting on* is not — it is one per in-flight
/// MCP call, and each call owns its own subscription. Eviction is
/// therefore strictly FIFO over unsubscribed tokens only: a token that just
/// entered the pre-subscription window is the newest entry and is the last
/// one an overflow can reach.
const MAX_PENDING_PROGRESS_TOKENS: usize = 32;
/// The delivery capacity of one subscribed token.
const PROGRESS_SUBSCRIPTION_CAPACITY: usize = 16;

/// Routes remote progress notifications to the in-flight `tools/call` that
/// owns their token (Issue #205).
///
/// # Why this is not rmcp's dispatcher
///
/// An MCP client cannot know a request's progress token before the request
/// exists: rmcp mints the token inside `send_cancellable_request`, so the
/// dispatching call can only subscribe *after* the request was enqueued. A
/// server that answers with a progress notification immediately therefore
/// races that subscription, and a dispatcher that drops unroutable
/// notifications silently loses genuine remote liveness evidence — the exact
/// evidence the generic idle watchdog depends on.
///
/// # The bounded liveness invariant
///
/// MCP progress is not only telemetry: it is the idle-liveness evidence the
/// generic Agent Loop watchdog consumes, so losing it can turn genuine
/// remote progress into a *false* idle timeout. The router therefore
/// guarantees exactly one thing, and deliberately not more:
///
/// > For every admitted in-flight MCP request, once the router has observed
/// > genuine remote progress for that request, no bound in this router
/// > erases the fact that progress occurred before the dispatching call can
/// > consume it.
///
/// Payload detail is explicitly *not* guaranteed. Two places coalesce:
///
/// - **the pre-subscription window** keeps one entry per unsubscribed
///   token, coalescing repeated notifications onto the latest payload and
///   counting the occurrences. The window is bounded by
///   [`MAX_PENDING_PROGRESS_TOKENS`] and evicts FIFO, so the newest entry —
///   which is exactly the one a call that just dispatched is about to claim
///   — is the last one an overflow can reach;
/// - **a subscribed token's delivery queue** is bounded by
///   [`PROGRESS_SUBSCRIPTION_CAPACITY`] and drops on full. Dropping there
///   can never erase an occurrence: the queue is full precisely because
///   [`PROGRESS_SUBSCRIPTION_CAPACITY`] undelivered notifications are
///   already waiting for the same subscriber, so the liveness fact is
///   already in its hands.
///
/// What the router no longer does is bound the number of *subscribed*
/// tokens. A subscription exists only while one in-flight `tools/call`
/// holds it, and every subscription is removed by its owner's `Drop`, so
/// the live set is bounded by the Agent Loop's own in-flight call set
/// rather than by a router constant. The previous global 16-token cache
/// mixed the two: a seventeenth concurrent call could find the cache full
/// of live subscriptions and have its only progress notification discarded,
/// which is precisely the false-idle-timeout case this invariant forbids.
///
/// Nothing is ever fabricated — the router only reorders and coalesces
/// delivery of notifications the peer genuinely sent.
#[derive(Default)]
struct McpProgressRouter {
    state: Mutex<ProgressRouterState>,
}

#[derive(Default)]
struct ProgressRouterState {
    /// One entry per live subscription. Bounded by the in-flight MCP call
    /// set, never by a router constant: each entry is removed by
    /// [`McpProgressSubscription`]'s `Drop`.
    subscribed: std::collections::HashMap<rmcp::model::ProgressToken, ProgressSubscriber>,
    /// The bounded pre-subscription window, in arrival order.
    pending: std::collections::VecDeque<(rmcp::model::ProgressToken, PendingProgress)>,
}

type ProgressSubscriber = tokio::sync::mpsc::Sender<ProgressNotificationParam>;

/// The coalesced pre-subscription evidence of one token.
struct PendingProgress {
    /// The most recent payload the peer sent for this token.
    latest: ProgressNotificationParam,
    /// How many notifications this entry represents. Only its non-zeroness
    /// is load-bearing — it is the liveness occurrence — but retaining the
    /// count keeps the coalescing honest in diagnostics.
    occurrences: u32,
}

impl McpProgressRouter {
    /// Delivers one remote progress notification, holding it in the bounded
    /// pre-subscription window when its token has no subscriber yet.
    fn deliver(&self, notification: ProgressNotificationParam) {
        let mut state = self
            .state
            .lock()
            .expect("MCP progress router lock poisoned");
        let token = notification.progress_token.clone();
        if let Some(sender) = state.subscribed.get(&token) {
            // Bounded, drop-on-full. This cannot erase a liveness
            // occurrence: a full queue is a queue whose subscriber already
            // holds `PROGRESS_SUBSCRIPTION_CAPACITY` undelivered proofs that
            // the remote is alive.
            let _ = sender.try_send(notification);
            return;
        }
        if let Some((_, pending)) = state
            .pending
            .iter_mut()
            .find(|(pending_token, _)| pending_token == &token)
        {
            pending.latest = notification;
            pending.occurrences = pending.occurrences.saturating_add(1);
            return;
        }
        if state.pending.len() == MAX_PENDING_PROGRESS_TOKENS {
            state.pending.pop_front();
        }
        state.pending.push_back((
            token,
            PendingProgress {
                latest: notification,
                occurrences: 1,
            },
        ));
    }

    /// Subscribes the dispatching call to its request's progress token,
    /// claiming anything that arrived before this call.
    fn subscribe(self: &Arc<Self>, token: rmcp::model::ProgressToken) -> McpProgressSubscription {
        let (sender, receiver) = tokio::sync::mpsc::channel(PROGRESS_SUBSCRIPTION_CAPACITY);
        {
            let mut state = self
                .state
                .lock()
                .expect("MCP progress router lock poisoned");
            if let Some(index) = state
                .pending
                .iter()
                .position(|(pending_token, _)| pending_token == &token)
            {
                let (_, pending) = state
                    .pending
                    .remove(index)
                    .expect("the index was just located");
                // The coalesced payload carries the liveness occurrence the
                // pre-subscription window preserved.
                let _ = sender.try_send(pending.latest);
            }
            state.subscribed.insert(token.clone(), sender);
        }
        McpProgressSubscription {
            router: Arc::clone(self),
            token,
            receiver,
        }
    }

    fn unsubscribe(&self, token: &rmcp::model::ProgressToken) {
        self.state
            .lock()
            .expect("MCP progress router lock poisoned")
            .subscribed
            .remove(token);
    }
}

/// One in-flight call's progress subscription. Dropping it unsubscribes.
struct McpProgressSubscription {
    router: Arc<McpProgressRouter>,
    token: rmcp::model::ProgressToken,
    receiver: tokio::sync::mpsc::Receiver<ProgressNotificationParam>,
}

impl McpProgressSubscription {
    async fn recv(&mut self) -> Option<ProgressNotificationParam> {
        self.receiver.recv().await
    }

    /// Takes every notification already delivered to this subscription
    /// without waiting for another.
    fn drain(&mut self) -> Vec<ProgressNotificationParam> {
        let mut drained = Vec::new();
        while let Ok(notification) = self.receiver.try_recv() {
            drained.push(notification);
        }
        drained
    }
}

impl Drop for McpProgressSubscription {
    fn drop(&mut self) {
        self.router.unsubscribe(&self.token);
    }
}

/// Deterministic capacity regressions for the bounded progress router
/// (Issue #205 review finding 3).
///
/// MCP progress is idle-liveness evidence, so a bound that can silently
/// discard the only progress report of an admitted in-flight call turns
/// genuine remote progress into a false idle timeout. These exercise the
/// capacity boundary directly: no clock, no transport, no sleep — the
/// router's own admission and delivery decisions are the whole contract.
#[cfg(test)]
mod progress_router_tests {
    use std::sync::Arc;

    use rmcp::model::{NumberOrString, ProgressNotificationParam, ProgressToken};

    use super::{MAX_PENDING_PROGRESS_TOKENS, McpProgressRouter, PROGRESS_SUBSCRIPTION_CAPACITY};

    fn token(value: u32) -> ProgressToken {
        ProgressToken(NumberOrString::Number(value.into()))
    }

    fn notification(value: u32, progress: f64) -> ProgressNotificationParam {
        ProgressNotificationParam::new(token(value), progress)
    }

    /// The exact shape the old global 16-token cache broke: more concurrent
    /// in-flight calls than that cache could track, each of which must still
    /// receive its own remote progress.
    ///
    /// Every subscription is one admitted in-flight MCP request, and each is
    /// owned by its own call, so the live set is bounded by the Agent Loop's
    /// in-flight call set rather than by a router constant. Delivering to all
    /// of them is what keeps the idle watchdog honest at concurrency above
    /// the old bound.
    #[test]
    fn every_admitted_call_receives_its_own_progress_above_the_old_token_bound() {
        let router = Arc::new(McpProgressRouter::default());
        let concurrent = 64_u32;
        let mut subscriptions: Vec<_> = (0..concurrent)
            .map(|index| router.subscribe(token(index)))
            .collect();
        for index in 0..concurrent {
            router.deliver(notification(index, f64::from(index)));
        }
        for (index, subscription) in subscriptions.iter_mut().enumerate() {
            let delivered = subscription.drain();
            assert_eq!(
                delivered.len(),
                1,
                "call {index} must observe its own remote liveness evidence"
            );
            #[allow(clippy::cast_precision_loss)]
            let expected = index as f64;
            assert!((delivered[0].progress - expected).abs() < f64::EPSILON);
        }
    }

    /// The pre-subscription window: a notification that arrives before the
    /// dispatching call subscribed is still claimed by that call, even while
    /// many other calls hold live subscriptions.
    ///
    /// Under the old bound this notification was the first thing dropped —
    /// the tracked-token cache was full of live subscriptions and an unknown
    /// token displaced nothing — so the call's only liveness evidence was
    /// erased before it could ever be consumed.
    #[test]
    fn a_pre_subscription_notification_survives_a_full_live_subscription_set() {
        let router = Arc::new(McpProgressRouter::default());
        let live: Vec<_> = (0..64_u32)
            .map(|index| router.subscribe(token(index)))
            .collect();
        // The dispatching call has not subscribed yet: rmcp mints the token
        // inside `send_cancellable_request`, so this window always exists.
        router.deliver(notification(1_000, 0.5));
        let mut late = router.subscribe(token(1_000));
        let delivered = late.drain();
        assert_eq!(
            delivered.len(),
            1,
            "the pre-subscription window is claimed by the call that owns the token"
        );
        assert!((delivered[0].progress - 0.5).abs() < f64::EPSILON);
        drop(live);
    }

    /// Payload may be coalesced; the liveness occurrence may not be lost.
    ///
    /// Many notifications for one unsubscribed token collapse onto the
    /// latest payload plus an occurrence count, so the window stays O(1) per
    /// token — and the subscribing call still observes that progress
    /// happened.
    #[test]
    fn pre_subscription_pressure_coalesces_payload_but_keeps_the_occurrence() {
        let router = Arc::new(McpProgressRouter::default());
        for pulse in 0..500 {
            router.deliver(notification(7, f64::from(pulse)));
        }
        let mut subscription = router.subscribe(token(7));
        let delivered = subscription.drain();
        assert_eq!(delivered.len(), 1, "the window keeps one entry per token");
        assert!(
            (delivered[0].progress - 499.0).abs() < f64::EPSILON,
            "coalescing keeps the most recent payload"
        );
    }

    /// A subscriber's queue overflowing never erases the liveness fact: the
    /// queue is full precisely because that many undelivered proofs are
    /// already waiting for it.
    #[test]
    fn a_full_subscription_queue_still_holds_undelivered_liveness_evidence() {
        let router = Arc::new(McpProgressRouter::default());
        let mut subscription = router.subscribe(token(3));
        for pulse in 0..(PROGRESS_SUBSCRIPTION_CAPACITY * 4) {
            #[allow(clippy::cast_precision_loss)]
            router.deliver(notification(3, pulse as f64));
        }
        let delivered = subscription.drain();
        assert_eq!(
            delivered.len(),
            PROGRESS_SUBSCRIPTION_CAPACITY,
            "drop-on-full bounds the queue"
        );
        assert!(
            !delivered.is_empty(),
            "the idle watchdog still observes that progress occurred"
        );
    }

    /// The pre-subscription window is bounded, and its eviction is FIFO, so
    /// the entry a just-dispatched call is about to claim is the last one an
    /// overflow can reach.
    #[test]
    fn the_pre_subscription_window_is_bounded_and_evicts_the_oldest_first() {
        let router = Arc::new(McpProgressRouter::default());
        for index in 0..u32::try_from(MAX_PENDING_PROGRESS_TOKENS).expect("small bound") {
            router.deliver(notification(index, 1.0));
        }
        // One more unsubscribed token than the window holds: the oldest is
        // evicted, the newest is retained.
        let overflow = u32::try_from(MAX_PENDING_PROGRESS_TOKENS).expect("small bound");
        router.deliver(notification(overflow, 2.0));
        let mut newest = router.subscribe(token(overflow));
        assert_eq!(
            newest.drain().len(),
            1,
            "the newest pre-subscription entry survives an overflow"
        );
        let mut oldest = router.subscribe(token(0));
        assert!(
            oldest.drain().is_empty(),
            "the oldest unsubscribed entry is the one evicted"
        );
    }
}

/// The discovery-time protocol-violation error: names the server identity
/// (for a managed Python package its synthesized `python:<folder>` id) and
/// carries the recorder's bounded diagnostic.
fn protocol_violation_error(server_id: &McpServerId, violation: &str) -> McpError {
    McpError::ProtocolViolation(format!(
        "MCP server '{server_id}' emitted a structurally invalid stdio message: {violation}; \
         the connection generation is protocol-poisoned and is not treated as healthy"
    ))
}

/// The execution-time protocol-violation diagnostic carried by a failed
/// tool result.
fn protocol_violation_call_diagnostic(server_id: &McpServerId, violation: &str) -> String {
    bound_error(&format!(
        "MCP protocol violation: server '{server_id}' emitted a structurally invalid stdio \
         message ({violation}); this connection generation is poisoned and cannot serve calls"
    ))
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
        McpError::ProtocolViolation(message) => {
            McpError::ProtocolViolation(format!("{message}; server stderr: {preview}"))
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
#[derive(Debug, Clone, PartialEq)]
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
    /// The **stable connection owner**, never one transport generation
    /// (Issue #205).
    ///
    /// The executor is long-lived: it is registered once per discovered
    /// remote tool and outlives any single transport. Binding it to the
    /// connection is what makes a replaced generation immediately usable by
    /// every already-admitted tool, and what keeps a dead transport from
    /// permanently poisoning a published capability generation.
    connection: Arc<connection::McpConnection>,
    /// The generation binding is present for coordinator-owned tools. Direct
    /// public adapter users retain the standalone runtime path and explicitly
    /// own/close that runtime themselves.
    binding: Option<McpRuntimeBinding>,
    remote_name: String,
}

impl McpToolExecutor {
    /// Creates an executor bound to one discovered remote name.
    ///
    /// The caller owns the runtime and closes it itself, so the executor's
    /// connection is *fixed*: it serves exactly this transport and never
    /// establishes a replacement.
    #[must_use]
    pub fn new(runtime: Arc<McpServerRuntime>, remote_name: String) -> Self {
        Self {
            connection: connection::McpConnection::fixed(runtime),
            binding: None,
            remote_name,
        }
    }

    fn new_owned(binding: McpRuntimeBinding, remote_name: String) -> Self {
        Self {
            connection: binding.connection().clone(),
            binding: Some(binding),
            remote_name,
        }
    }
}

impl ToolExecutor for McpToolExecutor {
    /// Starts one remote `tools/call`.
    ///
    /// # Why the operation *is* the settlement plane
    ///
    /// The whole physical operation — transport resolution, the bounded
    /// reconnect attempt it may perform, dispatch, progress forwarding, the
    /// cancellation path (local HTTP request termination, its release proof,
    /// and the best-effort `notifications/cancelled`), and post-frontier
    /// classification — lives inside the single operation future of
    /// [`ToolExecutionHandle::settled_by_operation`], and the settlement
    /// plane takes exclusive ownership of that future and drives it to its
    /// terminal end.
    ///
    /// [`ToolExecutionHandle::settled_by_operation`] is therefore still the
    /// honest abstraction after the Streamable HTTP ownership model: it is
    /// correct exactly when "the operation future returned" implies "all
    /// rustX-owned local execution for this invocation is over", and it does
    /// here. This executor spawns no task and owns no process outside that
    /// future, and the one local resource a dispatched call *does* own
    /// outside of Rust's stack — the in-flight Streamable HTTP request — is
    /// terminated and **proven released inside the same future**, before it
    /// returns anything. Nothing is left for a separate settlement plane to
    /// reclaim, so splitting completion from settlement would add a second
    /// ownership plane with nothing in it.
    ///
    /// The executor never chooses a canonical terminal status: it reports
    /// physical outcomes and settlement evidence, and the Agent Loop's
    /// generic lifecycle owns deadlines, cancellation intent, and the
    /// canonical `ToolResult`.
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        // The cancellation path stays inside the operation future: on
        // cancellation the executor terminates this call's own local
        // request ownership, issues the best-effort protocol cancellation,
        // keeps the response channel long enough to see whether a
        // correlated remote response exists, awaits the local release
        // proof, and returns either that proven remote outcome or its
        // honest `OutcomeUnknown`, which `settled_by_operation` surfaces as
        // unconfirmed settlement evidence.
        let cancellation = context.cancellation.clone();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                let started = Instant::now();
                let lease = self
                    .binding
                    .as_ref()
                    .and_then(McpRuntimeBinding::acquire_lease);
                if self.binding.is_some() && lease.is_none() {
                    return failed_mcp(
                        "MCP server runtime generation is physically retired",
                        &context,
                        started,
                    );
                }
                // Transport resolution happens strictly before this call's
                // own external-effect frontier: a dead generation is retired
                // and at most one bounded replacement attempt is made here.
                // A failure is therefore an ordinary pre-frontier failure of
                // this one call — no remote side effect was possible, and no
                // previously dispatched request is ever handed to the
                // replacement.
                let generation = match self.connection.acquire().await {
                    Ok(generation) => generation,
                    Err(error) => {
                        let facts = self.connection.recent_diagnostic();
                        return failed_mcp(
                            &format!(
                                "no MCP transport was available to dispatch the call, so it \
                                 never reached the server: {error}; connection history: {facts}"
                            ),
                            &context,
                            started,
                        );
                    }
                };
                generation
                    .runtime()
                    .call(
                        &self.remote_name,
                        invocation.arguments,
                        &context,
                        generation.generation(),
                    )
                    .await
            }),
            cancellation,
        )
    }

    // The executor forwards genuine remote MCP progress notifications to
    // `context.progress.report(...)`, so a configured idle-liveness window
    // is honestly informed. It fabricates no heartbeats: a server that sends
    // no progress notifications simply stays bounded by the generic hard
    // deadline.
    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::Meaningful
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
    // One fixed connection is shared by the whole catalog: the caller owns
    // this runtime and closes it, so no generation is ever replaced here.
    let connection = connection::McpConnection::fixed(runtime.clone());
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
                approval_policy: policy.approval,
                replay_policy: ToolReplayPolicy::Never,
                origin: ToolOrigin::Mcp {
                    server_id: server_id.clone(),
                },
            };
            (
                definition,
                Arc::new(McpToolExecutor {
                    connection: connection.clone(),
                    binding: None,
                    remote_name: tool.name,
                }) as Arc<dyn ToolExecutor>,
            )
        })
        .collect()
}

/// Converts a server catalog into registrations bound to a prepared MCP
/// generation. The generation owner remains with the candidate or published
/// capability state; each executor acquires a short explicit execution lease
/// when it runs.
pub(crate) fn definitions_owned(
    server_id: &McpServerId,
    policy: ToolInvocationPolicy,
    binding: &McpRuntimeBinding,
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
                approval_policy: policy.approval,
                replay_policy: ToolReplayPolicy::Never,
                origin: ToolOrigin::Mcp {
                    server_id: server_id.clone(),
                },
            };
            (
                definition,
                Arc::new(McpToolExecutor::new_owned(binding.clone(), tool.name))
                    as Arc<dyn ToolExecutor>,
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
    progress: Arc<McpProgressRouter>,
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
            progress: Arc::new(McpProgressRouter::default()),
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
        self.progress.deliver(params);
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
    context: &ToolExecutionContext<'_>,
    started: Instant,
) -> ToolExecutionResult {
    let background = context.execution_id.is_some();
    let mut capture = match open_mcp_output_capture(context) {
        Ok(capture) => capture,
        Err((locator, diagnostic)) => {
            return failed_mcp_storage(&diagnostic, locator, started);
        }
    };
    let reported_error = result.is_error == Some(true);
    let captured = capture_mcp_content(
        result.content,
        result.structured_content,
        context,
        &mut capture,
    );
    let capture_result = capture.finish(captured.storage_error.is_none());
    let output_diagnostic = captured
        .storage_error
        .as_deref()
        .or(captured.unsupported.as_deref());
    let continuation = continuation_for_capture(&capture_result, background, output_diagnostic);
    let (model_blocks, aggregate_error) =
        materialize_mcp_blocks(captured.pending_blocks, &capture_result, context.artifacts);
    let unsupported = captured.unsupported.or(aggregate_error);
    let status = if let Some(error) = captured.storage_error {
        ToolExecutionStatus::Failed {
            error: format!("MCP result output storage failed: {error}"),
        }
    } else if let Some(error) = unsupported {
        ToolExecutionStatus::Failed { error }
    } else if reported_error {
        ToolExecutionStatus::Failed {
            error: "MCP tool reported an execution error".to_owned(),
        }
    } else {
        ToolExecutionStatus::Success
    };
    let truncation = truncation_for_capture(&capture_result);
    ToolExecutionResult {
        status,
        content: model_blocks,
        duration_ms: duration_ms(started),
        exit_code: None,
        artifacts: Vec::new(),
        truncation,
        managed_output: continuation,
    }
}

/// The one MCP-owned accounting state for materialized result content.
///
/// Textual overflow is normalized by the shared Tool Plane capture first; the
/// complete spill is auxiliary and therefore does not consume this budget.
/// Every materialized MCP content component consumes this same counter,
/// including semantic image content. The complete status/content/continuation
/// model-facing aggregate is bounded later by
/// [`ToolExecutionResult::model_facing_projection`].
#[derive(Debug)]
struct McpAggregateBudget {
    remaining: usize,
}

const MCP_AGGREGATE_BUDGET_ERROR: &str =
    "MCP result content exceeds the aggregate model-facing result limit";
const MCP_IMAGE_AGGREGATE_BUDGET_ERROR: &str =
    "MCP image content exceeds the aggregate model-facing result limit";

impl McpAggregateBudget {
    fn new() -> Self {
        Self {
            remaining: MAX_MODEL_TOOL_RESULT_BYTES,
        }
    }

    fn consume(&mut self, bytes: usize) -> bool {
        if bytes > self.remaining {
            return false;
        }
        self.remaining -= bytes;
        true
    }
}

enum McpPendingBlock {
    Text(String),
    Json(serde_json::Value),
    Image(Vec<u8>),
}

/// Materializes semantic MCP blocks under MCP's content accounting. The
/// provider-independent complete result projection applies the final
/// aggregate model-facing bound after status and continuation are known.
fn materialize_mcp_blocks(
    pending_blocks: Vec<McpPendingBlock>,
    capture_result: &CapturedOutput,
    artifacts: &ArtifactStore,
) -> (Vec<ToolResultContent>, Option<String>) {
    let textual_overflow = capture_result.truncated || !capture_result.complete;
    let mut budget = McpAggregateBudget::new();
    let mut model_blocks = Vec::new();
    let mut aggregate_error = None;

    if textual_overflow {
        // The shared capture still owns the complete textual spill and its
        // aggregate preview state. The model-facing mixed projection needs a
        // separate bounded text budget, however: replacing all textual
        // blocks with one aggregate Text would move Images across their
        // source-relative semantic boundaries.
        let mut remaining_textual_preview = FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
        for pending in pending_blocks {
            match pending {
                McpPendingBlock::Text(text) => consume_mcp_text_preview(
                    &text,
                    &mut remaining_textual_preview,
                    &mut budget,
                    &mut model_blocks,
                    &mut aggregate_error,
                ),
                McpPendingBlock::Json(value) => consume_mcp_json_preview(
                    &value,
                    &mut remaining_textual_preview,
                    &mut budget,
                    &mut model_blocks,
                    &mut aggregate_error,
                ),
                McpPendingBlock::Image(bytes) => consume_mcp_image(
                    &bytes,
                    &mut budget,
                    &mut model_blocks,
                    &mut aggregate_error,
                    artifacts,
                ),
            }
        }
    } else {
        for pending in pending_blocks {
            match pending {
                McpPendingBlock::Text(text) => {
                    consume_mcp_text(text, &mut budget, &mut model_blocks, &mut aggregate_error);
                }
                McpPendingBlock::Json(value) => {
                    consume_mcp_json(value, &mut budget, &mut model_blocks, &mut aggregate_error);
                }
                McpPendingBlock::Image(bytes) => {
                    consume_mcp_image(
                        &bytes,
                        &mut budget,
                        &mut model_blocks,
                        &mut aggregate_error,
                        artifacts,
                    );
                }
            }
        }
    }

    (model_blocks, aggregate_error)
}

fn consume_mcp_text_preview(
    text: &str,
    remaining_textual_preview: &mut usize,
    budget: &mut McpAggregateBudget,
    model_blocks: &mut Vec<ToolResultContent>,
    aggregate_error: &mut Option<String>,
) {
    if *remaining_textual_preview < 2 {
        if *remaining_textual_preview == 1
            && let Some(character) = text.chars().next()
            && character.len_utf8() == 1
        {
            *remaining_textual_preview = 0;
            consume_mcp_text(character.to_string(), budget, model_blocks, aggregate_error);
        }
        return;
    }
    let mut preview = TextPreviewCapture::new(*remaining_textual_preview);
    preview.push(text);
    let (text, _) = preview.finish();
    *remaining_textual_preview = (*remaining_textual_preview).saturating_sub(text.len());
    if !text.is_empty() {
        consume_mcp_text(text, budget, model_blocks, aggregate_error);
    }
}

fn consume_mcp_json_preview(
    value: &serde_json::Value,
    remaining_textual_preview: &mut usize,
    budget: &mut McpAggregateBudget,
    model_blocks: &mut Vec<ToolResultContent>,
    aggregate_error: &mut Option<String>,
) {
    match serde_json::to_string(value) {
        Ok(text) => consume_mcp_text_preview(
            &text,
            remaining_textual_preview,
            budget,
            model_blocks,
            aggregate_error,
        ),
        Err(error) => {
            if aggregate_error.is_none() {
                *aggregate_error = Some(format!("invalid MCP structured content: {error}"));
            }
        }
    }
}

fn consume_mcp_text(
    text: String,
    budget: &mut McpAggregateBudget,
    model_blocks: &mut Vec<ToolResultContent>,
    aggregate_error: &mut Option<String>,
) {
    if budget.consume(text.len()) {
        model_blocks.push(ToolResultContent::Text(
            crate::message::content::TextBlock { text },
        ));
    } else if aggregate_error.is_none() {
        *aggregate_error = Some(MCP_AGGREGATE_BUDGET_ERROR.to_owned());
    }
}

fn consume_mcp_json(
    value: serde_json::Value,
    budget: &mut McpAggregateBudget,
    model_blocks: &mut Vec<ToolResultContent>,
    aggregate_error: &mut Option<String>,
) {
    match serde_json::to_vec(&value) {
        Ok(bytes) if budget.consume(bytes.len()) => {
            model_blocks.push(ToolResultContent::Json { value });
        }
        Ok(_) => {
            if aggregate_error.is_none() {
                *aggregate_error = Some(MCP_AGGREGATE_BUDGET_ERROR.to_owned());
            }
        }
        Err(error) => {
            if aggregate_error.is_none() {
                *aggregate_error = Some(format!("invalid MCP structured content: {error}"));
            }
        }
    }
}

fn consume_mcp_image(
    bytes: &[u8],
    budget: &mut McpAggregateBudget,
    model_blocks: &mut Vec<ToolResultContent>,
    aggregate_error: &mut Option<String>,
    artifacts: &ArtifactStore,
) {
    if !budget.consume(bytes.len()) {
        if aggregate_error.is_none() {
            *aggregate_error = Some(MCP_IMAGE_AGGREGATE_BUDGET_ERROR.to_owned());
        }
        return;
    }
    match write_artifact(artifacts, bytes) {
        Ok(id) => model_blocks.push(ToolResultContent::Image(
            crate::message::content::ImageReference {
                artifact_id: id,
                alt: None,
            },
        )),
        Err(error) => {
            if aggregate_error.is_none() {
                *aggregate_error = Some(error);
            }
        }
    }
}

fn open_mcp_output_capture(
    context: &ToolExecutionContext<'_>,
) -> Result<ToolOutputCapture, (PathBuf, String)> {
    let Some(execution_id) = context.execution_id else {
        return Ok(ToolOutputCapture::foreground());
    };
    let locator = context.tool_output.background_output_path(execution_id);
    let sink = context
        .tool_output
        .open_background_output_sink(execution_id)
        .map_err(|error| {
            (
                locator,
                format!("cannot open the background MCP result output: {error}"),
            )
        })?;
    Ok(ToolOutputCapture::background(sink, None))
}

struct McpContentCapture {
    pending_blocks: Vec<McpPendingBlock>,
    unsupported: Option<String>,
    storage_error: Option<String>,
}

fn capture_mcp_content(
    blocks: Vec<ContentBlock>,
    structured_content: Option<serde_json::Value>,
    context: &ToolExecutionContext<'_>,
    capture: &mut ToolOutputCapture,
) -> McpContentCapture {
    let mut captured = McpContentCapture {
        pending_blocks: Vec::new(),
        unsupported: None,
        storage_error: None,
    };
    let mut first_textual_block = true;
    {
        let mut push_textual = |text: &str| -> Result<(), String> {
            if !first_textual_block {
                capture.push(
                    "\n",
                    (!capture.is_background()).then_some(context.tool_output),
                )?;
            }
            first_textual_block = false;
            capture.push(
                text,
                (!capture.is_background()).then_some(context.tool_output),
            )
        };
        for block in blocks {
            match block {
                ContentBlock::Text(text) => {
                    if let Err(error) = push_textual(&text.text) {
                        captured.storage_error = Some(error);
                    }
                    captured
                        .pending_blocks
                        .push(McpPendingBlock::Text(text.text));
                }
                ContentBlock::Image(image) => {
                    match base64::engine::general_purpose::STANDARD.decode(image.data.as_bytes()) {
                        Ok(bytes) => captured.pending_blocks.push(McpPendingBlock::Image(bytes)),
                        Err(error) => {
                            captured.unsupported = Some(format!("invalid MCP image data: {error}"));
                        }
                    }
                }
                ContentBlock::Audio(_)
                | ContentBlock::Resource(_)
                | ContentBlock::ResourceLink(_) => {
                    captured.unsupported =
                        Some("MCP content variant has no canonical M7 representation".to_owned());
                }
                _ => {
                    captured.unsupported =
                        Some("unknown MCP content variant is unsupported in M7".to_owned());
                }
            }
        }
    }
    if let Some(value) = structured_content {
        if !first_textual_block
            && let Err(error) = capture.push(
                "\n",
                (!capture.is_background()).then_some(context.tool_output),
            )
        {
            captured.storage_error = Some(error);
        }
        if captured.storage_error.is_none() {
            let foreground_store = (!capture.is_background()).then_some(context.tool_output);
            let mut writer = ToolOutputWriter::new(capture, foreground_store);
            let write_result = serde_json::to_writer(&mut writer, &value)
                .map_err(|error| format!("invalid MCP structured content: {error}"))
                .and_then(|()| writer.finish());
            if let Err(error) = write_result {
                captured.storage_error = Some(error);
            }
        }
        captured.pending_blocks.push(McpPendingBlock::Json(value));
    }
    captured
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

fn failed_mcp_storage(diagnostic: &str, locator: PathBuf, started: Instant) -> ToolExecutionResult {
    let diagnostic = bound_error(diagnostic);
    ToolExecutionResult {
        status: ToolExecutionStatus::Failed {
            error: format!("MCP result output storage failed: {diagnostic}"),
        },
        content: Vec::new(),
        duration_ms: duration_ms(started),
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        managed_output: Some(crate::tools::types::ManagedOutputContinuation::Partial {
            locator,
            diagnostic,
        }),
    }
}

fn failed_mcp(
    message: &str,
    context: &ToolExecutionContext<'_>,
    started: Instant,
) -> ToolExecutionResult {
    mcp_empty_terminal(
        ToolExecutionStatus::Failed {
            error: bound_error(message),
        },
        context,
        started,
    )
}

/// The terminal status of a post-dispatch cancellation attempt.
///
/// The request was already dispatched, so the call crossed the
/// external-effect frontier. An accepted cancellation notification proves
/// only that the request reached the peer, never that the remote operation
/// terminated; a failed cancellation request proves even less. Either way
/// the final external outcome is unknown — never `Cancelled`, never
/// `Failed`.
fn post_dispatch_cancellation_status(
    cancelled: RemoteCancellation,
    generation_tag: &str,
) -> ToolExecutionStatus {
    let detail = match cancelled {
        RemoteCancellation::Sent => format!(
            "cancellation was requested after dispatch, but remote termination could not be \
             confirmed ({generation_tag})"
        ),
        RemoteCancellation::Failed(error) => format!(
            "cancellation was requested after dispatch and the cancellation request itself \
             failed ({generation_tag}): {error}"
        ),
        RemoteCancellation::Abandoned => format!(
            "cancellation was requested after dispatch; this call's own local request \
             ownership settled before the best-effort remote cancellation left rustX, so \
             remote termination could not be confirmed ({generation_tag})"
        ),
    };
    ToolExecutionStatus::OutcomeUnknown {
        detail: bound_error(&detail),
    }
}

/// Retains the dispatch-owned background locator for MCP outcomes that do
/// not carry a remote `CallToolResult` (unknown outcomes after dispatch or
/// a local protocol error). A healthy preallocated empty file is a complete
/// record of a call that produced no logical result; a sink-open failure is
/// explicitly Partial.
fn mcp_empty_terminal(
    status: ToolExecutionStatus,
    context: &ToolExecutionContext<'_>,
    started: Instant,
) -> ToolExecutionResult {
    let Some(execution_id) = context.execution_id else {
        return ToolExecutionResult {
            status,
            content: Vec::new(),
            duration_ms: duration_ms(started),
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        };
    };
    let locator = context.tool_output.background_output_path(execution_id);
    match context
        .tool_output
        .open_background_output_sink(execution_id)
    {
        Ok(sink) => {
            drop(sink);
            ToolExecutionResult {
                status,
                content: Vec::new(),
                duration_ms: duration_ms(started),
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                managed_output: Some(ManagedOutputContinuation::Complete { locator }),
            }
        }
        Err(error) => {
            let diagnostic = format!("cannot open the background MCP result output: {error}");
            // An output-storage failure never rewrites execution-outcome
            // certainty: a known failure gains the storage diagnostic, while
            // any other status keeps its own certainty claim and reports the
            // storage failure only through the managed-output continuation.
            let status = match status {
                ToolExecutionStatus::Failed { error } => ToolExecutionStatus::Failed {
                    error: bound_error(&format!(
                        "{error}; MCP result output storage failed: {diagnostic}"
                    )),
                },
                status => status,
            };
            ToolExecutionResult {
                status,
                content: Vec::new(),
                duration_ms: duration_ms(started),
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                managed_output: Some(ManagedOutputContinuation::Partial {
                    locator,
                    diagnostic: bound_error(&diagnostic),
                }),
            }
        }
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
    use std::time::Instant;

    use base64::Engine as _;
    use rmcp::model::{CallToolResult, ContentBlock};

    use super::{
        McpServerId, RemoteCancellation, mcp_empty_terminal, mcp_tool_id,
        post_dispatch_cancellation_status, translate_result,
    };
    use crate::runtime::identity::{ConversationId, ToolExecutionId};
    use crate::runtime::types::CancellationReason;
    use crate::runtime::{CancellationSignal, ExecutionCancellation};
    use crate::tools::executor::{ProgressReporter, ToolExecutionContext};
    use crate::tools::types::{ManagedOutputContinuation, ToolExecutionStatus, ToolResultContent};

    struct NoProgress;

    impl ProgressReporter for NoProgress {
        fn report(&self, _progress: crate::tools::types::ToolProgress) {}
    }

    fn runtime(
        name: &str,
    ) -> (
        tempfile::TempDir,
        crate::tools::runtime::ConversationToolRuntime,
    ) {
        let directory = tempfile::tempdir().expect("runtime directory");
        let root = directory.path().to_path_buf();
        std::fs::create_dir_all(root.join("workspace")).expect("workspace");
        let runtime = crate::tools::runtime::ConversationToolRuntime::new(
            ConversationId::new(name),
            root.join("workspace"),
            root.join("artifacts"),
        )
        .expect("tool runtime");
        (directory, runtime)
    }

    fn context<'a>(
        runtime: &'a crate::tools::runtime::ConversationToolRuntime,
        execution_id: Option<&'a ToolExecutionId>,
        progress: &'a NoProgress,
    ) -> ToolExecutionContext<'a> {
        ToolExecutionContext::new(
            runtime.conversation_id(),
            execution_id,
            ExecutionCancellation::detached(
                CancellationSignal::new(),
                CancellationReason::UserRequested,
            ),
            runtime.workspace(),
            progress,
            runtime.artifacts(),
            runtime.tool_output(),
            runtime.environment(),
        )
    }

    fn image_block(bytes: usize) -> ContentBlock {
        ContentBlock::image(
            base64::engine::general_purpose::STANDARD.encode(vec![b'i'; bytes]),
            "image/png",
        )
    }

    fn content_kinds(content: &[ToolResultContent]) -> Vec<&'static str> {
        content
            .iter()
            .map(|content| match content {
                ToolResultContent::Text(_) => "text",
                ToolResultContent::Json { .. } => "json",
                ToolResultContent::File(_) => "file",
                ToolResultContent::Image(_) => "image",
            })
            .collect()
    }

    #[test]
    fn foreground_mcp_aggregate_budget_accepts_exact_text_image_boundary() {
        let (_directory, runtime) = runtime("mcp-aggregate-exact");
        let text_bytes = crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
        let image_bytes = crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES - text_bytes;
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                ContentBlock::text("t".repeat(text_bytes)),
                image_block(image_bytes),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert!(result.managed_output.is_none());
        assert!(matches!(
            result.content.first(),
            Some(ToolResultContent::Text(text)) if text.text.len() == text_bytes
        ));
        assert!(matches!(
            result.content.get(1),
            Some(ToolResultContent::Image(_))
        ));
    }

    #[test]
    fn foreground_mcp_multiple_images_share_the_aggregate_budget() {
        let (_directory, runtime) = runtime("mcp-aggregate-images");
        let image_bytes = 40 * 1024;
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![image_block(image_bytes), image_block(image_bytes)]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("aggregate image overflow must fail explicitly: {result:?}");
        };
        assert!(error.contains("aggregate model-facing result limit"));
        assert_eq!(
            result
                .content
                .iter()
                .filter(|content| matches!(content, ToolResultContent::Image(_)))
                .count(),
            1
        );
        assert!(result.managed_output.is_none());
    }

    #[test]
    fn foreground_mcp_text_and_image_share_the_aggregate_budget() {
        let (_directory, runtime) = runtime("mcp-aggregate-text-image");
        let text_bytes = crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES;
        let image_bytes = crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES - text_bytes + 1;
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                ContentBlock::text("t".repeat(text_bytes)),
                image_block(image_bytes),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("text plus image overflow must fail explicitly: {result:?}");
        };
        assert!(error.contains("aggregate model-facing result limit"));
        assert!(matches!(
            result.content.first(),
            Some(ToolResultContent::Text(text)) if text.text.len() == text_bytes
        ));
        assert!(
            !result
                .content
                .iter()
                .any(|content| matches!(content, ToolResultContent::Image(_)))
        );
        assert!(result.managed_output.is_none());
    }

    #[test]
    fn foreground_mcp_structured_json_and_image_share_the_aggregate_budget() {
        let (_directory, runtime) = runtime("mcp-aggregate-json-image");
        let value = serde_json::json!({"payload": "x".repeat(1024)});
        let structured_bytes = serde_json::to_vec(&value).expect("structured JSON").len();
        let image_bytes = crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES - structured_bytes + 1;
        let mut call = CallToolResult::success(vec![image_block(image_bytes)]);
        call.structured_content = Some(value);
        let progress = NoProgress;
        let result = translate_result(call, &context(&runtime, None, &progress), Instant::now());

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("structured JSON plus image overflow must fail explicitly: {result:?}");
        };
        assert!(error.contains("aggregate model-facing result limit"));
        assert!(
            result
                .content
                .iter()
                .any(|content| matches!(content, ToolResultContent::Image(_)))
        );
        assert!(
            !result
                .content
                .iter()
                .any(|content| matches!(content, ToolResultContent::Json { .. }))
        );
        assert!(result.managed_output.is_none());
    }

    #[test]
    fn foreground_mcp_image_before_overflow_text_preserves_order_and_spill() {
        let (_directory, runtime) = runtime("mcp-order-image-text");
        let text = "B".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![image_block(1024), ContentBlock::text(text.clone())]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(content_kinds(&result.content), vec!["image", "text"]);
        assert!(matches!(
            &result.content[1],
            ToolResultContent::Text(text) if text.text.contains('B')
        ));
        let projection = result.model_facing_projection();
        assert!(projection.as_text().contains("Read or Grep"));
        let Some(ManagedOutputContinuation::Complete { locator }) = &result.managed_output else {
            panic!("overflow MCP text must remain Complete: {result:?}");
        };
        assert_eq!(
            std::fs::read(locator).expect("complete text spill"),
            text.as_bytes()
        );
        assert_eq!(
            std::fs::read_dir(locator.parent().expect("results directory"))
                .expect("results directory")
                .count(),
            1
        );
    }

    #[test]
    fn foreground_mcp_text_image_text_overflow_preserves_order() {
        let (_directory, runtime) = runtime("mcp-order-text-image-text");
        let first_bytes = crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES / 2;
        let second_bytes = crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES - first_bytes;
        let first = "A".repeat(first_bytes);
        let second = "B".repeat(second_bytes);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                ContentBlock::text(first.clone()),
                image_block(1024),
                ContentBlock::text(second.clone()),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(
            content_kinds(&result.content),
            vec!["text", "image", "text"]
        );
        assert!(matches!(
            &result.content[0],
            ToolResultContent::Text(text) if text.text == first
        ));
        assert!(matches!(
            &result.content[2],
            ToolResultContent::Text(text) if text.text == second
        ));
        let Some(ManagedOutputContinuation::Complete { locator }) = &result.managed_output else {
            panic!("mixed overflow MCP text must remain Complete: {result:?}");
        };
        assert_eq!(
            std::fs::read(locator).expect("complete mixed text spill"),
            format!("{first}\n{second}").as_bytes()
        );
        assert!(
            result
                .model_facing_projection()
                .as_text()
                .contains("Read or Grep")
        );
    }

    #[test]
    fn foreground_mcp_text_image_overflow_text_image_preserves_order() {
        let (_directory, runtime) = runtime("mcp-order-text-image-text-image");
        let first = "A".repeat(64);
        let second = "B".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                ContentBlock::text(first.clone()),
                image_block(1024),
                ContentBlock::text(second.clone()),
                image_block(1024),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        assert_eq!(result.status, ToolExecutionStatus::Success);
        assert_eq!(
            content_kinds(&result.content),
            vec!["text", "image", "text", "image"]
        );
        assert!(matches!(
            &result.content[0],
            ToolResultContent::Text(text) if text.text == first
        ));
        assert!(matches!(
            &result.content[2],
            ToolResultContent::Text(text) if text.text.contains('B')
        ));
        assert!(matches!(
            &result.content[1],
            ToolResultContent::Image(image) if image.artifact_id.as_str() == "artifact_1"
        ));
        assert!(matches!(
            &result.content[3],
            ToolResultContent::Image(image) if image.artifact_id.as_str() == "artifact_2"
        ));
        let Some(ManagedOutputContinuation::Complete { locator }) = &result.managed_output else {
            panic!("mixed overflow MCP text must remain Complete: {result:?}");
        };
        assert_eq!(
            std::fs::read(locator).expect("complete mixed text spill"),
            format!("{first}\n{second}").as_bytes()
        );
        assert!(
            result
                .model_facing_projection()
                .as_text()
                .contains("Read or Grep")
        );
    }

    #[test]
    fn foreground_mcp_aggregate_pressure_keeps_order_before_rejecting_late_image() {
        let (_directory, runtime) = runtime("mcp-order-budget-pressure");
        let first = "A".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES / 2);
        let second = "B".repeat(
            crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES
                - crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES / 2,
        );
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![
                ContentBlock::text(first.clone()),
                image_block(30 * 1024),
                ContentBlock::text(second.clone()),
                image_block(30 * 1024),
            ]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );

        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("late aggregate image overflow must fail explicitly: {result:?}");
        };
        assert!(error.contains("aggregate model-facing result limit"));
        assert_eq!(
            content_kinds(&result.content),
            vec!["text", "image", "text"]
        );
        assert!(matches!(
            &result.content[0],
            ToolResultContent::Text(text) if text.text == first
        ));
        assert!(matches!(
            &result.content[2],
            ToolResultContent::Text(text) if text.text == second
        ));
        assert!(matches!(
            &result.content[1],
            ToolResultContent::Image(image) if image.artifact_id.as_str() == "artifact_1"
        ));
        assert!(result.managed_output.is_some());
        assert!(
            result
                .model_facing_projection()
                .as_text()
                .contains("Complete output:")
        );
    }

    #[test]
    fn mcp_tool_ids_are_length_prefixed_and_stable() {
        let first = mcp_tool_id(&McpServerId::new("a"), "b");
        assert_eq!(first, mcp_tool_id(&McpServerId::new("a"), "b"));
        assert_ne!(first, mcp_tool_id(&McpServerId::new("ab"), ""));
        assert!(first.starts_with("mcp:sha256:"));
    }

    #[test]
    fn foreground_mcp_storage_allocation_failure_is_unavailable() {
        let (_directory, runtime) = runtime("mcp-storage-allocation");
        runtime.tool_output().set_force_open_failures(true);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![ContentBlock::text(
                "x".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1),
            )]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );
        assert!(matches!(result.status, ToolExecutionStatus::Failed { .. }));
        assert!(matches!(
            result.managed_output,
            Some(ManagedOutputContinuation::Unavailable { .. })
        ));
        assert!(result.content.iter().any(|content| matches!(
            content,
            ToolResultContent::Text(text) if text.text.len() <= crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES
        )));
    }

    #[test]
    fn foreground_mcp_storage_write_failure_is_partial() {
        let (_directory, runtime) = runtime("mcp-storage-write");
        runtime.tool_output().fail_writes_after(0);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![ContentBlock::text(
                "x".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1),
            )]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );
        let Some(ManagedOutputContinuation::Partial { locator, .. }) = result.managed_output else {
            panic!("MCP write failure must remain Partial: {result:?}");
        };
        assert!(locator.exists());
        assert!(matches!(result.status, ToolExecutionStatus::Failed { .. }));
    }

    #[test]
    fn foreground_mcp_utf8_boundary_keeps_preview_valid_and_spill_complete() {
        let (_directory, runtime) = runtime("mcp-utf8-boundary");
        let text = format!(
            "{}😀",
            "a".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES - 1)
        );
        let expected = text.clone();
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![ContentBlock::text(text)]),
            &context(&runtime, None, &progress),
            Instant::now(),
        );
        assert_eq!(result.status, ToolExecutionStatus::Success);
        let Some(ManagedOutputContinuation::Complete { locator }) = result.managed_output else {
            panic!("MCP UTF-8 result must be complete in managed output: {result:?}");
        };
        assert_eq!(
            std::fs::read_to_string(&locator).expect("UTF-8 MCP spill"),
            expected
        );
        assert!(result.content.iter().all(|content| match content {
            ToolResultContent::Text(text) => std::str::from_utf8(text.text.as_bytes()).is_ok(),
            _ => true,
        }));
        assert!(
            result
                .truncation
                .as_ref()
                .is_some_and(|truncation| truncation.truncated)
        );
    }

    #[test]
    fn background_mcp_empty_terminal_reuses_the_dispatch_locator() {
        let (_directory, runtime) = runtime("mcp-background-empty");
        let execution_id = ToolExecutionId::background(11);
        let advertised = runtime
            .tool_output()
            .allocate_background_output(&execution_id)
            .expect("dispatch output");
        let progress = NoProgress;
        let result = mcp_empty_terminal(
            ToolExecutionStatus::Cancelled {
                reason: CancellationReason::UserRequested,
                phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
            },
            &context(&runtime, Some(&execution_id), &progress),
            Instant::now(),
        );
        assert!(matches!(
            result.status,
            ToolExecutionStatus::Cancelled { .. }
        ));
        assert_eq!(
            result.managed_output,
            Some(ManagedOutputContinuation::Complete {
                locator: advertised
            })
        );
        assert_eq!(
            std::fs::read(runtime.tool_output().background_output_path(&execution_id))
                .expect("empty background output"),
            b""
        );
        assert_eq!(
            std::fs::read_dir(runtime.tool_output().root().join("results"))
                .expect("results directory")
                .count(),
            0
        );
    }

    /// Issue #202: an output-sink-open failure never rewrites execution
    /// outcome certainty. A non-`Failed` terminal status keeps its own
    /// certainty claim; the storage failure is reported only through the
    /// managed-output continuation as explicitly Partial.
    #[test]
    fn background_mcp_sink_open_failure_preserves_outcome_certainty() {
        let (_directory, runtime) = runtime("mcp-background-sink-open");
        let execution_id = ToolExecutionId::background(12);
        let advertised = runtime
            .tool_output()
            .allocate_background_output(&execution_id)
            .expect("dispatch output");
        runtime.tool_output().set_force_open_failures(true);
        let progress = NoProgress;
        let result = mcp_empty_terminal(
            ToolExecutionStatus::OutcomeUnknown {
                detail: "MCP transport closed during tools/call without a response".to_owned(),
            },
            &context(&runtime, Some(&execution_id), &progress),
            Instant::now(),
        );
        assert!(
            matches!(result.status, ToolExecutionStatus::OutcomeUnknown { .. }),
            "a storage failure must not rewrite the outcome status: {:?}",
            result.status
        );
        let Some(ManagedOutputContinuation::Partial {
            locator,
            diagnostic,
        }) = &result.managed_output
        else {
            panic!("a sink-open failure is honestly Partial: {result:?}");
        };
        assert_eq!(*locator, advertised);
        assert!(
            diagnostic.contains("cannot open the background MCP result output"),
            "the continuation names the storage failure: {diagnostic}"
        );
    }

    /// Issue #202: with the same sink-open failure, a known `Failed` status
    /// stays `Failed` and gains the storage diagnostic appended.
    #[test]
    fn background_mcp_sink_open_failure_appends_the_diagnostic_to_a_known_failure() {
        let (_directory, runtime) = runtime("mcp-background-sink-open-failed");
        let execution_id = ToolExecutionId::background(13);
        runtime
            .tool_output()
            .allocate_background_output(&execution_id)
            .expect("dispatch output");
        runtime.tool_output().set_force_open_failures(true);
        let progress = NoProgress;
        let result = mcp_empty_terminal(
            ToolExecutionStatus::Failed {
                error: "peer said no".to_owned(),
            },
            &context(&runtime, Some(&execution_id), &progress),
            Instant::now(),
        );
        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("a known failure stays Failed: {result:?}");
        };
        assert!(
            error.contains("peer said no") && error.contains("MCP result output storage failed"),
            "the failure gains the storage diagnostic: {error}"
        );
        assert!(matches!(
            result.managed_output,
            Some(ManagedOutputContinuation::Partial { .. })
        ));
    }

    /// Issue #202: the post-dispatch cancellation mapping is deterministic
    /// for both cancellation-request outcomes — an accepted request and a
    /// failed request both settle as `OutcomeUnknown` with a bounded detail,
    /// because neither proves the remote operation terminated.
    #[test]
    fn post_dispatch_cancellation_is_outcome_unknown_whether_the_request_succeeded_or_failed() {
        let tag = "MCP server 'fixture' connection generation 2";
        let accepted = post_dispatch_cancellation_status(RemoteCancellation::Sent, tag);
        let ToolExecutionStatus::OutcomeUnknown { detail } = &accepted else {
            panic!("an accepted cancel request is OutcomeUnknown: {accepted:?}");
        };
        assert!(
            detail.contains("remote termination could not be confirmed"),
            "the accepted-request detail names the unproven termination: {detail}"
        );
        assert!(
            detail.contains(tag),
            "the detail correlates the ambiguity with its connection generation: {detail}"
        );

        let huge_error = "x".repeat(64 * 1024);
        let failed = post_dispatch_cancellation_status(RemoteCancellation::Failed(huge_error), tag);
        let ToolExecutionStatus::OutcomeUnknown { detail } = &failed else {
            panic!("a failed cancel request is OutcomeUnknown: {failed:?}");
        };
        assert!(
            detail.contains("the cancellation request itself failed"),
            "the failed-request detail names the failed request: {detail}"
        );
        assert!(
            detail.len() <= 1024 + 128,
            "the detail stays bounded far below the 64KiB input: {} bytes",
            detail.len()
        );
    }

    #[test]
    fn background_mcp_storage_write_failure_keeps_the_dispatch_locator_partial() {
        let (_directory, runtime) = runtime("mcp-background-storage-write");
        let execution_id = ToolExecutionId::background(4);
        let advertised = runtime
            .tool_output()
            .allocate_background_output(&execution_id)
            .expect("dispatch output");
        runtime.tool_output().fail_writes_after(0);
        let progress = NoProgress;
        let result = translate_result(
            CallToolResult::success(vec![ContentBlock::text(
                "x".repeat(crate::tools::limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES + 1),
            )]),
            &context(&runtime, Some(&execution_id), &progress),
            Instant::now(),
        );
        let Some(ManagedOutputContinuation::Partial { locator, .. }) = result.managed_output else {
            panic!("background MCP write failure must remain Partial: {result:?}");
        };
        assert_eq!(locator, advertised);
        assert!(matches!(result.status, ToolExecutionStatus::Failed { .. }));
        assert_eq!(
            std::fs::read_dir(runtime.tool_output().root().join("results"))
                .expect("results directory")
                .count(),
            0
        );
    }
}
