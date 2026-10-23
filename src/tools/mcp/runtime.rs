//! One negotiated MCP transport generation: [`McpServerRuntime`].
//!
//! The runtime is the owner type every other module of the adapter extends
//! with focused `impl McpServerRuntime` blocks. This module holds its state,
//! its health facts (protocol poison, transport loss, close), the catalog
//! read, and the close/settlement sequence. Connection construction lives in
//! `connect`, invocation orchestration in `invocation`, the one physical
//! request owner in `request`, and the SEP-2663 task driver in
//! `task_driver`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rmcp::model::ProtocolVersion;
use rmcp::service::{RoleClient, RunningService};

use crate::runtime::identity::McpServerId;
use crate::runtime::interactive_process::SupervisedInteractiveProcess;

use super::executor::CanonicalMcpTool;
use super::handler::McpClientHandler;
use super::invalidation::McpInvalidationState;
#[cfg(test)]
use super::test_sync;
use super::{McpError, bound_error, framing, streamable_http};

/// One shared runtime for every tool exposed by a configured server.
pub struct McpServerRuntime {
    pub(super) credentials: crate::credentials::SourceCredentials,
    pub(super) server_id: McpServerId,
    pub(super) protocol_version: ProtocolVersion,
    /// Whether this peer advertised the SEP-2663 Tasks extension in its
    /// negotiated server capabilities (Issue #243).
    ///
    /// It is the **server side** of the capability contract, retained because
    /// advertisement is authority: a peer that never declared the extension
    /// may not silently acquire the right to answer `tools/call` with a
    /// `CreateTaskResult` and hand rustX a remote lifecycle to drive.
    pub(super) server_tasks: bool,
    pub(super) peer: rmcp::Peer<RoleClient>,
    pub(super) service:
        Arc<tokio::sync::Mutex<Option<RunningService<RoleClient, McpClientHandler>>>>,
    pub(super) handler: McpClientHandler,
    pub(super) process: Option<Arc<tokio::sync::Mutex<SupervisedInteractiveProcess>>>,
    /// Closed by the owning capability drain before physical process
    /// settlement is awaited. Stale executor handles cannot start another
    /// remote request after the runtime's process boundary closes.
    pub(super) closed: Arc<AtomicBool>,
    /// Serializes the close write boundary against every in-flight MCP
    /// catalog/request operation. The capability drain waits for this write
    /// lock, so a remote effect that began before close is settled before the
    /// runtime can publish quiescence and a stale reader that begins after
    /// close observes `closed` before it reaches the peer.
    pub(super) call_gate: Arc<tokio::sync::RwLock<()>>,
    /// The owned inline-lifecycle notification task. Capability drain joins
    /// it after closing the service, so a buffered notification cannot call
    /// back into capability invalidation after physical/runtime settlement.
    pub(super) subscription: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(super) invalidation: Arc<McpInvalidationState>,
    pub(super) change_notify: Arc<tokio::sync::Notify>,
    /// The generic stdio protocol-corruption observation seam: a confirmed
    /// structurally invalid peer message recorded by the transport tee.
    /// Once set, this generation is poisoned — no operation on it may
    /// settle as healthy.
    pub(super) protocol_violation: Arc<framing::ProtocolViolationRecorder>,
    /// The rustX-owned local ownership of this generation's in-flight HTTP
    /// requests (Issue #205), for Streamable HTTP only.
    ///
    /// Over stdio an outbound write owns no local resource that outlives it,
    /// so there is nothing per-request to terminate and this is `None`. Over
    /// Streamable HTTP each dispatched request is a live HTTP request whose
    /// POST future and response body rustX must be able to terminate and
    /// prove released before it may report unconfirmed settlement — see
    /// [`streamable_http::McpHttpRequestOwnership`].
    pub(super) request_ownership: Option<Arc<streamable_http::McpHttpRequestOwnership>>,
    /// The first observed proof that this transport generation can no longer
    /// carry MCP traffic (Issue #205): a transport-class rmcp failure on a
    /// request, or a response channel that ended without a reply. Recording
    /// it is monotonic and idempotent — repeated loss signals never produce
    /// a second retirement — and the owning [`connection::McpConnection`]
    /// reads it to retire this generation before any later request is
    /// dispatched.
    pub(super) transport_failure: Mutex<Option<String>>,
    /// Test-only close synchronization/fault seam, installed at most once.
    #[cfg(test)]
    pub(super) close_probe: std::sync::OnceLock<Arc<test_sync::CloseProbe>>,
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

impl McpServerRuntime {
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
    pub(super) fn note_transport_loss(&self, reason: &str) {
        let mut failure = self
            .transport_failure
            .lock()
            .expect("MCP transport failure lock poisoned");
        if failure.is_none() {
            *failure = Some(bound_error(&self.credentials.redact(reason)));
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
            // Registered before the epoch is read, for the reason above: an
            // unenabled `Notified` is not yet a waiter, and the invalidation
            // notify stores no permit.
            let notified = self.change_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
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
        self.list_tools_admitted()
            .await
            .map_err(|error| error.redacted(&self.credentials))
    }

    /// First-use revalidation is owned by the `ToolInvocation`. Each page uses
    /// the same cancellation and local-release proof as its eventual call.
    pub(super) async fn list_tools_for_invocation(
        &self,
        context: &crate::tools::executor::ToolExecutionContext<'_>,
        generation: u64,
    ) -> Result<Vec<CanonicalMcpTool>, McpError> {
        use super::outcome::{McpPhysicalOutcome, McpResponseOutcome};
        use rmcp::model::{ClientRequest, ListToolsRequest, PaginatedRequestParams, ServerResult};
        let mut cursor = None;
        let mut tools = Vec::new();
        loop {
            let mut params = PaginatedRequestParams::default();
            params.cursor = cursor;
            let mut extensions = rmcp::model::Extensions::default();
            extensions.insert(super::dispatch::InvocationCatalogRequest);
            let outcome = self
                .dispatch_owned_request(
                    ClientRequest::ListToolsRequest(ListToolsRequest {
                        method: rmcp::model::ListToolsRequestMethod,
                        params: Some(params),
                        extensions,
                    }),
                    super::request::McpRequestOwnership::control(),
                    context,
                    generation,
                    std::time::Instant::now(),
                )
                .await;
            let page = match outcome {
                McpPhysicalOutcome::Observed {
                    response: McpResponseOutcome::Answered(answered),
                    ..
                } => match *answered {
                    Ok(ServerResult::ListToolsResult(page)) => page,
                    Ok(_) => {
                        return Err(McpError::Discovery("unexpected tools/list response".into()));
                    }
                    Err(error) => {
                        return Err(McpError::Discovery(bound_error(&error.to_string()))
                            .redacted(&self.credentials));
                    }
                },
                McpPhysicalOutcome::Refused { detail, .. } => {
                    return Err(McpError::Discovery(detail));
                }
                _ => {
                    return Err(McpError::Discovery(
                        "MCP catalog validation cancelled or disconnected before tools/call".into(),
                    ));
                }
            };
            tools.extend(
                page.tools
                    .into_iter()
                    .map(CanonicalMcpTool::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
            );
            cursor = page.next_cursor;
            if cursor.is_none() {
                tools.sort_by(|left, right| left.name.cmp(&right.name));
                return Ok(tools);
            }
        }
    }

    async fn list_tools_admitted(&self) -> Result<Vec<CanonicalMcpTool>, McpError> {
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

    /// How many request lifecycle entries this generation's Streamable HTTP
    /// ownership registry currently holds.
    ///
    /// The memory bound of the local request ownership layer, exposed so a
    /// boundary regression can assert it directly rather than infer it.
    #[cfg(all(test, feature = "mcp-fixture"))]
    pub(crate) fn outstanding_http_requests(&self) -> usize {
        self.request_ownership
            .as_ref()
            .map_or(0, |ownership| ownership.outstanding_requests())
    }

    #[cfg(all(test, feature = "mcp-fixture"))]
    pub(crate) fn hold_http_release(
        &self,
        method: &'static str,
    ) -> Arc<streamable_http::HttpReleaseProbe> {
        self.request_ownership
            .as_ref()
            .expect("HTTP transport")
            .hold_http_release(method)
    }

    #[cfg(all(test, feature = "mcp-fixture"))]
    pub(crate) fn http_request_states(
        &self,
    ) -> Vec<(rmcp::model::RequestId, Option<String>, String, bool)> {
        self.request_ownership
            .as_ref()
            .expect("HTTP transport")
            .request_states()
    }

    /// Current progress cardinality, including pre-admission tombstones.
    #[cfg(all(test, feature = "mcp-fixture"))]
    pub(crate) fn tracked_progress_requests(&self) -> usize {
        self.handler.progress.tracked_requests()
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
    pub(super) async fn wait_before_retirement_failure_publication(&self) {
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

    /// Poisons the generation after a confirmed peer protocol violation and
    /// returns the bounded call diagnostic, when the observation seam
    /// recorded one. Call sites choose the terminal status: a violation
    /// observed before dispatch is a known `Failed`; one observed after
    /// dispatch leaves the external outcome unknown.
    pub(super) async fn poisoned_protocol_violation(&self) -> Option<String> {
        let violation = self.protocol_violation.violation()?;
        self.poison_after_protocol_violation().await;
        Some(protocol_violation_call_diagnostic(
            &self.server_id,
            &violation,
        ))
    }

    /// The bounded transport-generation tag carried by MCP diagnostics.
    ///
    /// It is what correlates one call's outcome with one concrete negotiated
    /// transport authority: an ambiguous call names the generation it died
    /// on, and a later successful call names the replacement.
    pub(super) fn generation_tag(&self, generation: u64) -> String {
        format!(
            "MCP server '{}' connection generation {generation}",
            self.server_id
        )
    }
}

/// The discovery-time protocol-violation error: names the server identity
/// (for a managed Python package its synthesized `python:<folder>` id) and
/// carries the recorder's bounded diagnostic.
pub(super) fn protocol_violation_error(server_id: &McpServerId, violation: &str) -> McpError {
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
