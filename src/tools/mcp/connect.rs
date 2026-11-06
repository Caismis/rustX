//! Construction of one MCP transport generation.
//!
//! [`OwnedConnect`] carries the connection owner's cancellation signal;
//! `connect_admitted` builds the stdio or Streamable HTTP transport, runs
//! the revision-negotiating handshake through the single construction seam
//! ([`start_client_service`]), validates the negotiated revision and
//! capabilities, and installs exactly one revision-appropriate invalidation
//! mechanism. Every failure after physical process ownership began awaits
//! the same settlement proof drain uses ([`settle_connect_failure`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use rmcp::ClientServiceExt;
use rmcp::model::ProtocolVersion;
use rmcp::service::{RoleClient, RunningService};

use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::McpServerId;
use crate::runtime::interactive_process::{InteractiveProcessSpec, SupervisedInteractiveProcess};
use crate::tools::workspace::Workspace;

use super::handler::McpClientHandler;
use super::invalidation::McpInvalidationState;
use super::runtime::{McpServerRuntime, protocol_violation_error};
#[cfg(test)]
use super::test_sync;
use super::{
    McpError, McpServerBinding, McpTransportConfig, bound_error, dispatch, framing,
    handshake_lifecycle, streamable_http, supported_protocol_versions, uses_inline_lifecycle,
};

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
        #[cfg(test)]
        crate::local_runtime::static_effects::observe(
            crate::local_runtime::static_effects::Effect::Connect,
        );
        let credentials = request.binding.credentials.clone();
        Self::connect_admitted(request)
            .await
            .map_err(|error| error.redacted(&credentials))
    }

    #[allow(clippy::too_many_lines)] // one existing transport ownership and settlement boundary
    async fn connect_admitted(request: OwnedConnect<'_>) -> Result<Arc<Self>, McpError> {
        let OwnedConnect {
            server_id,
            binding,
            workspace,
            invalidation,
            cancellation,
            #[cfg(test)]
            ownership_pause,
        } = request;
        let credentials = binding
            .credentials
            .resolve()
            .map_err(|reason| McpError::Configuration(format!("source {server_id}: {reason}")))?;
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
        let protocol_violation =
            framing::ProtocolViolationRecorder::new(binding.credentials.clone());
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
                explicit_environment.extend(
                    credentials
                        .environment
                        .iter()
                        .map(|(key, value)| (key.clone(), value.expose().to_owned())),
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
                    pause.record_process(process.lock().await.supervisor_child_pid);
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
                // The outbound ownership seam. Stdio owns no per-request
                // local half, so the seam here carries only the progress
                // token linearization: a request's token becomes known
                // before the request can reach the server.
                let transport = dispatch::ObservingTransport::new(
                    transport,
                    Arc::new(dispatch::McpDispatchSeam::new(
                        Arc::clone(&handler.progress),
                        None,
                    )),
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
                    started = start_client_service(handler.clone(), transport, handshake_lifecycle(server_id)) => started,
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
                for (name, value) in &credentials.headers {
                    let name = http::HeaderName::try_from(name).map_err(|_| {
                        McpError::Configuration("invalid sensitive HTTP header name".into())
                    })?;
                    let mut value = http::HeaderValue::try_from(value.expose()).map_err(|_| {
                        McpError::Configuration("invalid sensitive HTTP header value".into())
                    })?;
                    value.set_sensitive(true);
                    transport_config.custom_headers.insert(name, value);
                }
                transport_config.reinit_on_expired_session = false;
                transport_config.allow_stateless = true;
                // rustX supplies the HTTP client rather than taking rmcp's
                // default one, because the in-flight HTTP request of a
                // dispatched `tools/call` is local ownership a cancelled
                // call has to terminate and prove released. The registry
                // returned here is that authority; it belongs to exactly
                // this connection generation.
                let (client, ownership) = streamable_http::McpHttpClient::new()?;
                request_ownership = Some(Arc::clone(&ownership));
                let transport = rmcp::transport::StreamableHttpClientTransport::with_client(
                    client,
                    transport_config,
                );
                // The outbound ownership seam. Over Streamable HTTP it
                // carries both halves: a request's progress token becomes
                // known before the request can reach the server, and its
                // lifecycle entry takes dispatch ownership before the
                // message is handed to rmcp's transport worker.
                let transport = dispatch::ObservingTransport::new(
                    transport,
                    Arc::new(dispatch::McpDispatchSeam::new(
                        Arc::clone(&handler.progress),
                        Some(Arc::clone(&ownership)),
                    )),
                );
                let service = tokio::select! {
                    biased;
                    () = cancellation.cancelled() => Err(McpError::Discovery(
                        "MCP connection cancelled during the handshake".to_owned(),
                    )),
                    started = start_client_service(handler.clone(), transport, handshake_lifecycle(server_id)) => started,
                };
                let service = match service {
                    Ok(service) => service,
                    Err(error) => {
                        // The handshake future (and its outbound seam) has
                        // ended, but rmcp's HTTP worker may still own a POST.
                        // Terminate and await that local ownership explicitly.
                        ownership.settle_failed_handshake().await;
                        return Err(error);
                    }
                };
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
        let runtime = Arc::new(Self {
            credentials: binding.credentials.clone(),
            server_id: server_id.clone(),
            protocol_version: info.protocol_version.clone(),
            capabilities: super::McpNegotiatedCapabilities {
                tools: info.capabilities.tools.is_some(),
                resources: info.capabilities.resources.is_some(),
            },
            server_tasks: info.capabilities.supports_tasks(),
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
        if !runtime.capabilities.tools && !runtime.capabilities.resources {
            let error =
                McpError::Unsupported("MCP server advertises neither tools nor resources".into());
            return Err(match runtime.close().await {
                Ok(()) => error,
                Err(settlement) => McpError::PhysicalSettlement(format!("{error}; {settlement}")),
            });
        }
        if info
            .capabilities
            .tools
            .as_ref()
            .is_some_and(|tools| tools.list_changed == Some(true))
        {
            // Reached only when the server advertises `tools.listChanged`,
            // so at most one invalidation mechanism exists per connection and
            // exactly one revision-appropriate mechanism is installed here.
            // Both feed the one `ToolListChangedSink`, so neither duplicates
            // the other's admission, invalidation, or notification.
            if uses_inline_lifecycle(&info.protocol_version) {
                let subscribed = tokio::select! {
                    biased;
                    () = cancellation.cancelled() => Err(McpError::Discovery(
                        "MCP connection cancelled during subscription registration".to_owned(),
                    )),
                    result = runtime.subscribe_tool_list_changed() => result,
                };
                if let Err(error) = subscribed {
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
                    .install_tool_list_changed_sink(runtime.tool_list_changed_sink());
            }
        }
        if cancellation.is_cancelled() {
            let error = McpError::Discovery("MCP connection cancelled before registration".into());
            return Err(match runtime.close().await {
                Ok(()) => error,
                Err(settlement) => McpError::PhysicalSettlement(format!("{error}; {settlement}")),
            });
        }
        Ok(runtime)
    }
}

/// Establishes one MCP client service: the **single connection-construction
/// seam** of the adapter.
///
/// Every rustX MCP connection generation is born here — the initial stdio
/// connection, the initial Streamable HTTP connection, and every
/// replacement generation a bounded reconnect establishes — because
/// `connect_owned` is the only path that builds a transport and
/// `McpConnection::acquire` reaches it again for a replacement. Anything
/// that must hold for *every* generation therefore belongs here, and only
/// here.
///
/// The one such policy is the SDK response cache: it is disabled on the
/// running peer the instant the peer exists and before rustX issues any
/// semantic request over it. See [`disable_sdk_response_cache`].
async fn start_client_service<T, E, A>(
    handler: McpClientHandler,
    transport: T,
    // Chosen per server by `handshake_lifecycle`: `Auto` runs the real
    // negotiation for an unknown peer (probing the inline `server/discover`
    // lifecycle, walking the offered revisions down on
    // `UNSUPPORTED_PROTOCOL_VERSION`, and falling back to the legacy
    // `initialize` handshake when the peer proves it is pre-2026), while a
    // rustX-materialized managed package uses `Discover` and never issues a
    // second, collidable handshake request.
    lifecycle: rmcp::ClientLifecycleMode,
) -> Result<RunningService<RoleClient, McpClientHandler>, McpError>
where
    T: rmcp::transport::IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let service = handler
        .serve_with_lifecycle(transport, lifecycle)
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
        })?;
    disable_sdk_response_cache(service.peer()).await;
    Ok(service)
}

/// Removes the SDK response cache from one freshly established peer.
///
/// # Why the SDK cache is not a rustX cache
///
/// rmcp 3.x caches `tools/list` (and the other list/read results) per peer
/// whenever a server supplies a positive `ttlMs`, and by default also serves
/// an *expired* entry when a re-fetch fails. Both behaviours are semantic:
/// they decide what a `tools/list` refresh returns.
///
/// rustX already owns that decision. Its layering is
///
/// ```text
/// remote MCP peer -> fresh protocol response -> adapter translation
///   -> prepared capability candidate -> invalidation-epoch validation
///   -> capability commit -> canonical rustX tool snapshot
/// ```
///
/// and the committed snapshot is the *only* semantic cache of MCP tool
/// availability. An SDK cache in that chain would be a second, unowned
/// capability store: a positive `ttlMs` could hide a real catalog change
/// from a refresh, and stale-on-error could turn a failed refresh into a
/// fabricated success — exactly the fact the capability coordinator must
/// see in order to keep the last-known-good generation honestly.
///
/// The cache is therefore disabled outright, not merely narrowed by turning
/// stale-on-error off: rustX issues no MCP request whose semantics may come
/// from an SDK cache.
///
/// # Why here
///
/// This runs on the peer of a *running* service, so the lifecycle handshake
/// (`server/discover` or `initialize`) is already complete, and it runs
/// before [`McpServerRuntime::connect_owned`] performs any semantic
/// operation — capability validation, `subscriptions/listen`, `tools/list`,
/// `tools/call`. The lifecycle handshake itself never consults the cache
/// (rmcp reads it only from the list/read request methods), so no rustX
/// request can observe SDK response-cache semantics at any point in a
/// generation's life.
async fn disable_sdk_response_cache(peer: &rmcp::service::Peer<RoleClient>) {
    peer.set_response_cache_config(rmcp::ClientCacheConfig::disabled())
        .await;
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

fn resolve_workspace_cwd(workspace: &Workspace, cwd: Option<&Path>) -> Result<PathBuf, McpError> {
    let Some(cwd) = cwd else {
        return Ok(workspace.root().to_path_buf());
    };
    let resolved = if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        workspace.root().join(cwd)
    };
    if !resolved.is_dir() {
        return Err(McpError::Configuration(
            "stdio cwd is not a directory".to_owned(),
        ));
    }
    Ok(resolved)
}
