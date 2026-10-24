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
//! That negotiation exists for peers whose revision rustX does not know. A
//! rustX-materialized managed Python package is pinned to a `FastMCP` build
//! that always speaks the inline lifecycle, so it is offered
//! [`rmcp::ClientLifecycleMode::Discover`] instead: exactly one handshake
//! request and no probe deadline, so a slow cold start can never produce a
//! late-probe/fallback request-id collision. See `handshake_lifecycle`.
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
//!
//! # Invalidation ownership
//!
//! The two mechanisms are revision-specific and mutually exclusive, and
//! neither is capability authority. A received `tools/list_changed` — from
//! either mechanism — does exactly one thing: it advances the shared
//! invalidation epoch once, through the single
//! [`McpInvalidationState`] boundary, applied by the one
//! `invalidation::ToolListChangedSink` both mechanisms feed. What that epoch *means* is decided by
//! the capability coordinator, which owns candidate preparation, epoch
//! validation, and publication. A connection generation is a transport and
//! session authority; it never publishes, retires, or edits capability
//! knowledge.
//!
//! # There is exactly one semantic capability cache
//!
//! rustX's committed capability snapshot is the single semantic source of
//! truth for MCP tool availability:
//!
//! ```text
//! remote MCP peer -> fresh protocol response -> adapter translation
//!   -> prepared capability candidate -> invalidation-epoch validation
//!   -> capability commit -> canonical rustX tool snapshot
//! ```
//!
//! The SDK is transport and protocol machinery in that chain, never a second
//! capability store. rmcp 3.x can cache list results per peer when a server
//! supplies `ttlMs`, and can serve an expired entry when a re-fetch fails;
//! both would decide what a `tools/list` refresh returns. rustX therefore
//! disables that cache outright on every connection generation, at the one
//! construction seam every generation passes through
//! (`disable_sdk_response_cache`, called from `start_client_service` in
//! `connect`).
//! There is deliberately no rustX option, feature flag, or compatibility
//! mode that re-enables it.
//!
//! # Streamable HTTP ownership
//!
//! The Streamable HTTP protocol — session semantics (or their 2026-07-28
//! absence), SEP-2243 `Mcp-Method` / `Mcp-Name` / `Mcp-Param-*` routing
//! metadata, SSE framing, resumption — is rmcp's. rustX supplies its own
//! [`StreamableHttpClient`](rmcp::transport::streamable_http_client::StreamableHttpClient)
//! for exactly one reason: a cancelled tool call must be able to terminate,
//! and prove the release of, its own in-flight HTTP request. That wrapper
//! forwards rmcp's generated headers unchanged and synthesizes none of its
//! own, and rustX introduces no session identity of its own — a modern peer
//! that issues no MCP session id stays stateless.
//!
//! # Module layout
//!
//! ```text
//! mod.rs            protocol revisions, bindings, transport config, McpError
//! connect.rs        OwnedConnect and transport-generation construction
//! runtime.rs        McpServerRuntime state, health, catalog read, close
//! handler.rs        rmcp client handler (progress, legacy list_changed)
//! invalidation.rs   epoch state and the one tools/list_changed sink
//! generation.rs     runtime resource generations, leases, retirement
//! connection.rs     stable connection owner and transport replacement
//! executor.rs       canonical tools, McpToolExecutor, tool identities
//! invocation.rs     MRTR invocation orchestration of one tools/call
//! request.rs        dispatch_owned_request: the one physical request owner
//! outcome.rs        physical-request outcome facts
//! task_driver.rs    SEP-2663 remote task driver
//! progress.rs       progress router, caller leases, subscriptions
//! result.rs         CallToolResult materialization and terminal results
//! dispatch.rs       outbound Transport::send ownership seam
//! streamable_http.rs  rustX-owned HTTP request ownership
//! framing.rs mrtr.rs tasks.rs identity.rs  protocol translation layers
//! ```
//!
//! Every `impl McpServerRuntime` block outside `runtime.rs` extends the same
//! owner and its existing locks, owned tasks, and close ordering; the split
//! adds no lock, background task, or cleanup timing of its own.

#[cfg(feature = "mcp-fixture")]
#[doc(hidden)]
pub mod fixture;

mod connect;
mod connection;
mod dispatch;
mod executor;
mod framing;
mod generation;
mod handler;
pub mod identity;
mod invalidation;
mod invocation;
mod mrtr;
mod outcome;
mod progress;
mod request;
mod result;
mod runtime;
mod streamable_http;
mod task_driver;
mod tasks;
#[cfg(test)]
mod test_context;
/// Deterministic MCP ownership synchronization seams used by the M9c
/// regressions. Test-only: no production path constructs these.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod test_sync;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use rmcp::model::ProtocolVersion;
use serde::{Deserialize, Serialize};

use crate::runtime::identity::McpServerId;
use crate::tools::limits::bounded_text_preview;
use crate::tools::types::ToolInvocationPolicy;

pub(crate) use connect::OwnedConnect;
pub(crate) use connection::McpConnection;
pub(crate) use executor::definitions_owned_for_source;
pub use executor::{
    CanonicalMcpTool, McpToolExecutor, definitions, mcp_tool_id, source_server_id,
    source_tool_identity,
};
pub(crate) use generation::{
    McpRuntimeBinding, McpRuntimeGeneration, McpRuntimeLeaseAuthority, McpRuntimeLeaseSet,
    McpRuntimeRetirementRegistry,
};
pub use invalidation::{McpInvalidationGuard, McpInvalidationState};
/// The fixed rustX-owned multi-round-trip bound, re-exported for the
/// in-crate boundary suite that proves both sides of it.
#[cfg(all(test, feature = "mcp-fixture"))]
pub(crate) use mrtr::MCP_MRTR_MAX_ROUNDS;
pub use runtime::McpServerRuntime;

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

/// The handshake lifecycle rustX offers one server.
///
/// # Why managed servers do not use `Auto`
///
/// `Auto` probes `server/discover` and, when the peer does not answer inside
/// rmcp's bounded ten-second probe window, falls back to `initialize` **on the
/// same transport without draining the abandoned probe**
/// (`serve_client_with_ct_inner`, rmcp 3.2.0 through 3.4.0). A probe response
/// that arrives after the window therefore carries the abandoned request id
/// while the client is awaiting the fallback's id, and rmcp's
/// `expect_response` treats that mismatch as fatal
/// (`ConflictInitResponseId`) instead of discarding a stale response:
///
/// ```text
/// server/discover (id 0)   -> (peer still starting)
/// probe window elapses     -> fall back on the same transport
/// initialize      (id 1)   -> awaiting id 1
///                          <- late response for id 0
///                          => ConflictInitResponseId { expected: 1, got: 0 }
/// ```
///
/// Any server whose cold start can exceed the probe window is exposed to
/// that race, and it surfaces only as an intermittent connection failure.
///
/// rustX materializes managed Python packages itself against the pinned
/// [`MANAGED_FASTMCP_VERSION`](crate::tools::python::MANAGED_FASTMCP_VERSION),
/// which always speaks the inline lifecycle, so those servers never need the
/// legacy fallback at all. Offering `Discover` sends exactly one handshake
/// request and imposes no probe deadline, which makes the collision
/// structurally impossible rather than merely rarer. The
/// [`MANAGED_MCP_NAMESPACE`](crate::tools::python::MANAGED_MCP_NAMESPACE) is
/// reserved by the session configuration parser, so only a rustX-materialized
/// package can ever present such an id.
///
/// Externally configured servers keep `Auto`: their revision is genuinely
/// unknown, and the legacy fallback is exactly the point.
fn handshake_lifecycle(server_id: &McpServerId) -> rmcp::ClientLifecycleMode {
    if server_id
        .as_str()
        .starts_with(crate::tools::python::MANAGED_MCP_NAMESPACE)
    {
        rmcp::ClientLifecycleMode::Discover {
            preferred_versions: supported_protocol_versions(),
        }
    } else {
        rmcp::ClientLifecycleMode::Auto {
            preferred_versions: supported_protocol_versions(),
            legacy_version: Some(legacy_handshake_version()),
        }
    }
}

/// A configured MCP server transport.
///
/// The type is serializable because a subagent child materializes the exact
/// transport its parent generation froze (Issue #145): the frozen binding
/// crosses the private subagent control channel rather than being
/// rediscovered from `rustx.toml` in the child.
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
    /// Host-authorized references and private resolved instance state.
    #[serde(default)]
    pub credentials: crate::credentials::SourceCredentials,
    /// The configured transport.
    pub transport: McpTransportConfig,
    /// One origin-independent policy for all tools from this server.
    pub policy: ToolInvocationPolicy,
}

impl std::fmt::Debug for McpServerBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpServerBinding")
            .field("credentials", &self.credentials)
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

impl McpError {
    fn redacted(mut self, credentials: &crate::credentials::SourceCredentials) -> Self {
        let message = match &mut self {
            Self::Configuration(message)
            | Self::Discovery(message)
            | Self::ProtocolCompatibility(message)
            | Self::ProtocolViolation(message)
            | Self::Execution(message)
            | Self::PhysicalSettlement(message) => message,
        };
        *message = credentials.redact(message);
        self
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
    use super::{
        McpServerId, handshake_lifecycle, legacy_handshake_version, supported_protocol_versions,
    };

    /// **The handshake request-id collision regression.** `Auto` falls back
    /// to `initialize` on the same transport without draining the abandoned
    /// `server/discover` probe, so a peer that answers the probe after rmcp's
    /// bounded window collides that late id with the fallback's id and fails
    /// an otherwise healthy connection:
    ///
    /// ```text
    /// server/discover (id 0) -> (peer still starting)
    /// probe window elapses   -> initialize (id 1) on the same transport
    ///                        <- late response for id 0
    ///                        => ConflictInitResponseId { expected: 1, got: 0 }
    /// ```
    ///
    /// A rustX-materialized managed package is pinned to an inline-lifecycle
    /// `FastMCP`, so it is offered `Discover`: exactly one handshake request,
    /// no probe deadline, and therefore no second id to collide with. Every
    /// other peer keeps the real negotiation, because its revision is
    /// genuinely unknown and the legacy fallback is the point.
    #[test]
    fn a_managed_package_is_offered_discover_while_unknown_peers_keep_auto() {
        // The synthesized id proves `python_server_id` lands in the namespace;
        // the literal proves the rule is the namespace itself, not one folder.
        let synthesized = crate::tools::python::python_server_id("tasks-tool");
        for managed in [synthesized.as_str(), "python:anything"] {
            assert_eq!(
                handshake_lifecycle(&McpServerId::new(managed)),
                rmcp::ClientLifecycleMode::Discover {
                    preferred_versions: supported_protocol_versions(),
                },
                "{managed}: a managed package must issue exactly one handshake request"
            );
        }

        // Only the reserved `python:` namespace is managed; near-misses are
        // ordinary configured servers and keep the negotiation.
        for external in [
            "vendor-server",
            "python",
            "pythonish",
            "not-python:tasks-tool",
        ] {
            assert_eq!(
                handshake_lifecycle(&McpServerId::new(external)),
                rmcp::ClientLifecycleMode::Auto {
                    preferred_versions: supported_protocol_versions(),
                    legacy_version: Some(legacy_handshake_version()),
                },
                "{external}: an unknown peer keeps the legacy fallback"
            );
        }
    }
}
