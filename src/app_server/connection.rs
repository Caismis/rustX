//! One connection negotiates once and routes independent Session attachments.
//! Only the attachment map is connection-owned; all semantic work is delegated.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::protocol::{
    APP_SERVER_PROTOCOL_VERSION, AttachmentTarget, ErrorData, Failure, InitializeParams,
    JsonRpcVersion, Method, MethodResult, Notification, NotificationMethod, Request, RequestId,
    Response, RpcError, ServerCapabilities, Success, UserInputBlock,
};
use crate::local_runtime::session::SessionId;
use crate::local_runtime::session_controller::SessionController;
use crate::local_runtime::session_runtime_manager::{ManagedRuntimeClient, RuntimeManagerError};
use crate::runtime_client::attachment::RuntimeAttachment;
use crate::runtime_client::host::EventDelivery;
use crate::runtime_client::types::{RuntimeClientError, RuntimeClientResult};

use super::host::{AppServerHost, AttachmentPermit, HostAdmissionError};

const MAX_ATTACHMENTS: usize = 32;

fn valid_request_id(id: &RequestId) -> bool {
    !matches!(id, RequestId::Integer(n) if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(n))
}

struct RouteTable {
    attachment_limit: usize,
    closed: bool,
    active: BTreeMap<SessionId, Arc<Route>>,
    reserved: std::collections::BTreeSet<SessionId>,
}

impl Default for RouteTable {
    fn default() -> Self {
        Self {
            attachment_limit: MAX_ATTACHMENTS,
            closed: false,
            active: BTreeMap::new(),
            reserved: std::collections::BTreeSet::new(),
        }
    }
}

struct AttachReservation {
    table: Arc<Mutex<RouteTable>>,
    session: SessionId,
    committed: bool,
}

impl AttachReservation {
    fn new(
        table: &Arc<Mutex<RouteTable>>,
        session: &SessionId,
        host: &AppServerHost,
    ) -> Result<Self, RpcError> {
        let mut routes = table.lock().expect("routes mutex");
        if routes.closed {
            return Err(domain(ErrorData::StaleAttachment));
        }
        if routes.active.contains_key(session) || routes.reserved.contains(session) {
            return Err(domain(ErrorData::ControllerInUse));
        }
        if routes.active.len() + routes.reserved.len() >= routes.attachment_limit {
            host.attachment_capacity_refused();
            return Err(domain(ErrorData::AttachmentCapacity));
        }
        routes.reserved.insert(session.clone());
        Ok(Self {
            table: table.clone(),
            session: session.clone(),
            committed: false,
        })
    }

    fn commit<T>(
        mut self,
        attach: impl FnOnce() -> Result<(Arc<Route>, T), RpcError>,
    ) -> Result<T, RpcError> {
        let mut routes = self.table.lock().expect("routes mutex");
        routes.reserved.remove(&self.session);
        if routes.closed {
            return Err(domain(ErrorData::StaleAttachment));
        }
        // Claim and publish under the same close boundary: a closed connection
        // must never acquire even a temporary external attachment. No await.
        let (route, result) = attach()?;
        routes.active.insert(self.session.clone(), route);
        self.committed = true;
        Ok(result)
    }
}

impl Drop for AttachReservation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        self.table
            .lock()
            .expect("routes mutex")
            .reserved
            .remove(&self.session);
    }
}

fn release_route(host: &AppServerHost, table: &Mutex<RouteTable>, route: &Arc<Route>) {
    let mut routes = table.lock().expect("routes mutex");
    if routes
        .active
        .get(&route.target.session_id)
        .is_some_and(|current| Arc::ptr_eq(current, route))
    {
        routes.active.remove(&route.target.session_id);
        route.attachment.detach();
        host.uploads().revoke_route(route);
        route.external.release();
        route.capacity.release();
    }
}

pub(super) struct Route {
    pub(super) target: AttachmentTarget,
    pub(super) client: ManagedRuntimeClient,
    pub(super) attachment: RuntimeAttachment,
    external: crate::local_runtime::session_runtime_manager::RuntimeResidencyPin,
    capacity: AttachmentPermit,
}

/// A transport may share this connection across concurrent request handlers.
/// A single notification consumer drives fan-in directly, without pump tasks.
#[derive(Clone)]
pub struct AppServerConnection {
    host: AppServerHost,
    sessions: SessionController,
    initialized: Arc<Mutex<Option<InitializeParams>>>,
    routes: Arc<Mutex<RouteTable>>,
    changed: Arc<tokio::sync::Notify>,
    reader: Arc<tokio::sync::Mutex<()>>,
    next_route: Arc<std::sync::atomic::AtomicUsize>,
    configuration_versions: Arc<Mutex<std::collections::BTreeMap<String, u64>>>,
    /// The native Session-metadata invalidation log (Issue #386) and this
    /// connection's cursor into it.
    ///
    /// The cursor is taken at **construction**, before this connection can
    /// serve any request, so the bootstrap ordering is closed by construction:
    /// a publication older than the connection is already reflected in every
    /// read the connection can make, and a publication newer than it is
    /// delivered live. The log is level-triggered, so nothing is lost if the
    /// notification reader starts late, pauses, or never runs.
    summary_invalidations: Arc<crate::local_runtime::session::SessionSummaryInvalidations>,
    summary_invalidations_delivered: Arc<Mutex<u64>>,
    ownership_invalidations_delivered: Arc<Mutex<u64>>,
}

impl AppServerConnection {
    pub(crate) fn server_draining(&self) -> bool {
        self.host.server_draining()
    }

    pub(crate) fn transport_failure(&self) {
        self.host.transport_failure();
    }

    #[cfg(test)]
    pub(crate) fn attachment_counts(&self) -> (usize, usize) {
        let routes = self.routes.lock().expect("routes mutex");
        (routes.active.len(), routes.reserved.len())
    }

    #[cfg(test)]
    pub(crate) fn new_with_attachment_limit_for_test(host: AppServerHost, limit: usize) -> Self {
        let connection = Self::new(host);
        connection
            .routes
            .lock()
            .expect("routes mutex")
            .attachment_limit = limit;
        connection
    }

    #[must_use]
    pub fn new(host: AppServerHost) -> Self {
        let sessions = host.manager().session_controller();
        let summary_invalidations = sessions.summary_invalidations();
        let delivered = summary_invalidations.frontier();
        Self {
            summary_invalidations,
            summary_invalidations_delivered: Arc::new(Mutex::new(delivered)),
            ownership_invalidations_delivered: Arc::new(Mutex::new(delivered)),
            sessions,
            host,
            initialized: Arc::new(Mutex::new(None)),
            routes: Arc::new(Mutex::new(RouteTable::default())),
            changed: Arc::new(tokio::sync::Notify::new()),
            reader: Arc::new(tokio::sync::Mutex::new(())),
            configuration_versions: Arc::default(),
            next_route: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Permanently release this connection's external claims, even while callers
    /// retain references. The route-table lock linearizes close against attach
    /// reservation/commit. Already admitted runtime operations remain server-owned.
    /// Calling close again has no effect; it never unloads or cancels a runtime.
    /// # Panics
    /// Panics if the routing mutex is poisoned.
    pub fn close(&self) {
        let mut routes = self.routes.lock().expect("routes mutex");
        routes.closed = true;
        for (_, route) in std::mem::take(&mut routes.active) {
            route.attachment.detach();
            self.host.uploads().revoke_route(&route);
            route.external.release();
            route.capacity.release();
        }
        self.changed.notify_one();
    }

    /// Decode a strict JSON-RPC request before performing any semantic action.
    /// Notifications never receive responses and do not invoke request-only methods.
    /// # Panics
    /// Panics if a connection routing mutex is poisoned.
    pub async fn handle_json(&self, json: &str) -> Option<Response> {
        let value: serde_json::Value = match serde_json::from_str(json) {
            Ok(value) => value,
            Err(_) => return Some(failure(None, rpc_error(-32700, "Parse error", None))),
        };
        let Some(object) = value.as_object() else {
            return Some(failure(None, rpc_error(-32600, "Invalid Request", None)));
        };
        let id = object
            .get("id")
            .and_then(|value| serde_json::from_value::<RequestId>(value.clone()).ok())
            .filter(valid_request_id);
        if object.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0")
            || !object
                .get("method")
                .is_some_and(serde_json::Value::is_string)
            || object
                .keys()
                .any(|key| !matches!(key.as_str(), "jsonrpc" | "id" | "method" | "params"))
            || (object.contains_key("id") && id.is_none())
        {
            return Some(failure(id, rpc_error(-32600, "Invalid Request", None)));
        }
        let id = id?;
        // Decode the original bytes, not the Value above: materializing a Value
        // first would erase duplicate fields before typed validation.
        if let Ok(request) = serde_json::from_str::<Request>(json) {
            Some(self.handle_request(request).await)
        } else {
            // Probe only the method tag, using the same derived vocabulary.
            // A closed enum mismatch inside params must never be classified
            // as an unknown method (even when its value equals the method).
            let unknown = serde_json::from_value::<Method>(serde_json::json!({
                "method": object["method"]
            }))
            .is_err_and(|error| error.to_string().starts_with("unknown variant"));
            Some(failure(
                Some(id),
                rpc_error(
                    if unknown { -32601 } else { -32602 },
                    if unknown {
                        "Method not found"
                    } else {
                        "Invalid params"
                    },
                    None,
                ),
            ))
        }
    }

    /// Correlation is preserved even when unrelated requests complete out of order.
    /// # Panics
    /// Panics if a connection routing mutex is poisoned.
    pub async fn handle_request(&self, request: Request) -> Response {
        let id = request.id;
        if !valid_request_id(&id) {
            return failure(None, domain(ErrorData::InvalidParams));
        }
        // Catalog mutations retain their owner if the protocol waiter leaves.
        // Runtime operations already transfer ownership at start_operation;
        // attachment reservations intentionally remain caller-cancellable.
        let result = if matches!(
            &request.call,
            Method::SessionCreate { .. }
                | Method::SessionName { .. }
                | Method::SessionDelete { .. }
                | Method::SessionFork { .. }
                | Method::SessionBranch { .. }
                | Method::SessionRecoverDeletion { .. }
                | Method::SourcesWrite { .. }
                | Method::ConfigurationReconcile { .. }
                | Method::McpConnect { .. }
                | Method::McpDisconnect { .. }
                | Method::AdoptConfiguration { .. }
        ) {
            let connection = self.clone();
            tokio::spawn(async move { Box::pin(connection.dispatch(request.call)).await })
                .await
                .unwrap_or_else(|_| Err(domain(ErrorData::OperationFailed)))
        } else {
            Box::pin(self.dispatch(request.call)).await
        };
        match result {
            Ok(result) => Response::Success(Box::new(Success {
                jsonrpc: JsonRpcVersion::V2,
                id,
                result,
            })),
            Err(error) => failure(Some(id), error),
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn dispatch(&self, method: Method) -> Result<MethodResult, RpcError> {
        if self.routes.lock().expect("routes mutex").closed {
            return Err(domain(ErrorData::StaleAttachment));
        }
        if let Method::Initialize(params) = method {
            if params.protocol_version != APP_SERVER_PROTOCOL_VERSION {
                return Err(domain(ErrorData::UnsupportedVersion {
                    supported: APP_SERVER_PROTOCOL_VERSION,
                    requested: params.protocol_version,
                }));
            }
            if params.client.name.is_empty()
                || params.client.name.len() > 128
                || params.client.version.len() > 128
            {
                return Err(domain(ErrorData::InvalidParams));
            }
            let mut state = self.initialized.lock().expect("initialize mutex");
            if state.is_some() {
                return Err(domain(ErrorData::AlreadyInitialized));
            }
            *state = Some(params);
            return Ok(MethodResult::Initialized {
                authority_id: self.host.authority_id().to_owned(),
                protocol_version: APP_SERVER_PROTOCOL_VERSION,
                capabilities: ServerCapabilities::default(),
            });
        }
        if self.initialized.lock().expect("initialize mutex").is_none() {
            return Err(domain(ErrorData::NotInitialized));
        }
        if matches!(method, Method::ServerDiagnostics {}) {
            return Ok(MethodResult::Diagnostics {
                snapshot: self.host.diagnostics(),
            });
        }
        if let Some(target) = runtime_target(&method) {
            let route = self.route(target)?;
            let client = route.client.clone();
            let changed = self.changed.clone();
            let sessions = self.sessions.clone();
            let manager = self.host.manager().clone();
            let upload_host = self.host.clone();
            let receiver = {
                let routes = self.routes.lock().expect("routes mutex");
                if routes.closed
                    || !routes
                        .active
                        .get(&target.session_id)
                        .is_some_and(|active| Arc::ptr_eq(active, &route))
                {
                    return Err(domain(ErrorData::StaleAttachment));
                }
                // Route close -> host drain -> residency claim: host drain
                // cannot interpose after request admission but before its native
                // operation lease. Only capture authority here; execution is
                // owned by the manager task outside these admission locks.
                self.host
                    .admit_request(|request_owner| {
                        client.start_operation(move || {
                            let authority = route.attachment.operation_authority();
                            async move {
                                let _request = request_owner;
                                dispatch_runtime(
                                    method,
                                    route,
                                    authority.map_err(client_error)?,
                                    changed,
                                    sessions,
                                    manager,
                                    upload_host,
                                )
                                .await
                            }
                        })
                    })
                    .map_err(host_error)?
                    .map_err(manager_error)?
            };
            return receiver
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?;
        }
        let request_owner = self
            .host
            .admit_request(std::convert::identity)
            .map_err(host_error)?;
        match method {
            Method::Initialize(_) | Method::ServerDiagnostics {} => unreachable!(),
            Method::SessionDetach { target } => {
                // Removing a connection relationship needs no live-runtime lease.
                let route = self.route(&target)?;
                release_route(&self.host, &self.routes, &route);
                self.changed.notify_one();
                Ok(MethodResult::Detached {})
            }
            Method::SessionDelete {
                session_id,
                expected_target_revision,
            } => {
                let manager = self.host.manager().clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                tokio::spawn(async move {
                    let _request = request_owner;
                    let result = manager
                        .delete_session(&session_id, &expected_target_revision)
                        .await
                        .map(deletion)
                        .map_err(manager_error);
                    let _ = sender.send(result);
                });
                receiver
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))?
            }
            Method::SessionSwitchNode { target, node_id } => {
                let route = self.route(&target)?;
                let manager = self.host.manager().clone();
                let host = self.host.clone();
                let routes = self.routes.clone();
                let changed = self.changed.clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                // Cleanup is server-owned even if the initiating caller goes away.
                tokio::spawn(async move {
                    let _request = request_owner;
                    let result = async {
                        manager
                            .unload_incarnation(&target.conversation_id, target.runtime_incarnation)
                            .await
                            .map_err(manager_error)?;
                        // Explicit selection is a catalog commit, unlike a cold
                        // attachment/read. Publish ownership retirement before
                        // admitting the successor runtime; failed composition
                        // leaves the committed node selected, never resurrects A.
                        let session = manager
                            .session_controller()
                            .set_current_node(&target.session_id, &node_id)
                            .await
                            .map_err(session_error)?;
                        manager
                            .load(&target.session_id, Some(&node_id))
                            .await
                            .map_err(manager_error)?;
                        Ok(MethodResult::Session { session })
                    }
                    .await;
                    release_route(&host, &routes, &route);
                    changed.notify_one();
                    let _ = sender.send(result);
                });
                receiver
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))?
            }
            Method::ServerInfo {} => Ok(MethodResult::ServerInfo {
                capabilities: ServerCapabilities::default(),
            }),
            Method::SessionExportPrepare { session_id } => {
                let download = super::archive_download::prepare(&self.host, session_id.clone())
                    .await
                    .map_err(|reason| {
                        let data = match reason {
                            crate::session_archive::SessionArchivePrepareError::UnknownSession => {
                                ErrorData::UnknownSession { session_id }
                            }
                            crate::session_archive::SessionArchivePrepareError::Busy => {
                                ErrorData::RequestCapacity
                            }
                            reason => ErrorData::ArchivePreparationFailed { reason },
                        };
                        rpc_error(-32000, &reason.to_string(), Some(data))
                    })?;
                Ok(MethodResult::SessionArchive { download })
            }
            Method::SessionList {
                query,
                offset,
                limit,
            } => {
                let page = self
                    .sessions
                    .list_sessions(query.as_deref(), offset, limit)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::Sessions {
                    sessions: page.sessions,
                    next_offset: page.next_offset,
                })
            }
            Method::SessionSummary { session_id } => Ok(MethodResult::SessionSummary {
                summary: self
                    .sessions
                    .read_session_summary(&session_id)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionHistory {
                session_id,
                node_id,
                at,
                limit,
            } => {
                if limit == 0 || limit > 256 {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (conversation_id, window) = self
                    .sessions
                    .read_history(&session_id, node_id.as_ref(), at, limit)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionHistory {
                    conversation_id,
                    window,
                })
            }
            Method::SessionTraceHistory {
                session_id,
                node_id,
                before,
                limit,
            } => {
                if limit == 0 || limit > crate::runtime_client::trace::TRACE_PAGE_LIMIT {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (conversation_id, page) = self
                    .sessions
                    .read_trace(&session_id, node_id.as_ref(), move |projection| {
                        projection.page(before.as_ref(), limit)
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionTraceHistory {
                    conversation_id,
                    page,
                })
            }
            Method::SessionTraceHistoryDetail {
                session_id,
                node_id,
                record_id,
            } => {
                if record_id.len() > 256 {
                    return Err(domain(ErrorData::InvalidParams));
                }
                let (conversation_id, detail) = self
                    .sessions
                    .read_trace(&session_id, node_id.as_ref(), move |projection| {
                        projection.detail(&record_id)
                    })
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionTraceHistoryDetail {
                    conversation_id,
                    detail: detail.map(Box::new),
                })
            }
            Method::SessionStatistics {
                session_id,
                node_id,
            } => {
                let (conversation_id, statistics, occupancy) = self
                    .sessions
                    .read_statistics(&session_id, node_id.as_ref())
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionStatistics {
                    conversation_id,
                    statistics,
                    occupancy,
                })
            }
            Method::SessionRead { session_id } => Ok(MethodResult::Session {
                session: self
                    .sessions
                    .read_session(&session_id)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionCreate { settings } => self
                .host
                .manager()
                .create_session(settings)
                .await
                .map(transition)
                .map_err(session_error),
            Method::SessionName { session_id, name } => Ok(MethodResult::Session {
                session: self
                    .sessions
                    .rename_session(&session_id, &name)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionTree {
                session_id,
                offset,
                limit,
            } => {
                let page = self
                    .sessions
                    .tree(&session_id, offset, limit)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::Tree {
                    nodes: page.nodes,
                    next_offset: page.next_offset,
                })
            }
            Method::SessionFork {
                session_id,
                node_id,
                surface_revision,
                boundary,
                side,
            } => self
                .sessions
                .copy_lineage(
                    &session_id,
                    node_id.as_ref(),
                    surface_revision,
                    boundary.as_ref(),
                    false,
                    side,
                )
                .await
                .map(transition)
                .map_err(session_error),
            Method::SessionBranch {
                session_id,
                node_id,
                surface_revision,
                boundary,
                side,
            } => self
                .sessions
                .copy_lineage(
                    &session_id,
                    Some(&node_id),
                    surface_revision,
                    Some(&boundary),
                    true,
                    side,
                )
                .await
                .map(transition)
                .map_err(session_error),
            Method::SessionDeletePreview { session_id } => {
                Ok(deletion(self.sessions.delete_preview(&session_id).await))
            }
            Method::SessionRecoverDeletion { session_id } => {
                let manager = self.host.manager().clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                tokio::spawn(async move {
                    let _request = request_owner;
                    let result = manager
                        .recover_session_deletion(&session_id)
                        .await
                        .map(deletion)
                        .map_err(manager_error);
                    let _ = sender.send(result);
                });
                receiver
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))?
            }
            Method::ConfigurationReconcile { target } => {
                Ok(MethodResult::ConfigurationApplication {
                    application: self
                        .host
                        .manager()
                        .reconcile_configuration(&target)
                        .await
                        .map_err(source_settings_error)?,
                })
            }
            Method::AdoptConfiguration {
                session_id,
                candidate,
                expected_binding,
            } => Ok(MethodResult::ConfigurationApplication {
                application: self
                    .host
                    .manager()
                    .adopt_configuration(&session_id, &candidate, expected_binding)
                    .map_err(|rejection| domain(ErrorData::ConfigurationAdoption { rejection }))?,
            }),
            Method::SessionConfiguration { session_id } => {
                self.sessions
                    .read_settings(&session_id)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::SessionConfiguration {
                    application: self.host.manager().configuration_application(&session_id),
                })
            }
            Method::McpConnect {
                target,
                id,
                expected_revision,
                refresh,
            } => Ok(MethodResult::McpConnections {
                connections: vec![
                    self.host
                        .manager()
                        .connect_mcp(target, id, expected_revision, refresh)
                        .await
                        .map_err(source_error)?,
                ],
            }),
            Method::McpStatus { target } => Ok(MethodResult::McpConnections {
                connections: self
                    .host
                    .manager()
                    .mcp_status(&target)
                    .map_err(source_error)?,
            }),
            Method::McpDisconnect { target, id } => {
                target.validate().map_err(source_error)?;
                self.host.manager().disconnect_mcp(&target, &id).await;
                Ok(MethodResult::McpConnections {
                    connections: self
                        .host
                        .manager()
                        .mcp_status(&target)
                        .map_err(source_error)?,
                })
            }
            Method::SourcesRead { target } => {
                let projection = self
                    .host
                    .manager()
                    .source_settings(&target, None)
                    .await
                    .map_err(source_settings_error)?;
                Ok(MethodResult::SourceSettings {
                    projection: Box::new(projection),
                })
            }
            Method::SourcesWrite {
                target,
                expected_revision,
                mutation,
            } => {
                let projection = self
                    .host
                    .manager()
                    .source_settings(&target, Some((expected_revision, mutation)))
                    .await
                    .map_err(source_settings_error)?;
                Ok(MethodResult::SourceSettings {
                    projection: Box::new(projection),
                })
            }
            Method::SettingsRead { session_id } => {
                let (revision, settings) = self
                    .sessions
                    .read_settings(&session_id)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::Settings { revision, settings })
            }
            Method::SessionAttach {
                session_id,
                node_id,
            } => {
                let capacity = self.host.admit_attachment().map_err(host_error)?;
                let reservation = AttachReservation::new(&self.routes, &session_id, &self.host)?;
                // Reservation is request-scoped; once claimed, Loading is
                // manager-scoped. Dropping this request does not roll it back.
                let runtime = self
                    .host
                    .manager()
                    .load(&session_id, node_id.as_ref())
                    .await
                    .map_err(manager_error)?;
                let result = reservation.commit(|| {
                    let client = runtime.client();
                    let (
                        crate::runtime_client::attachment::AttachedSnapshot {
                            attachment,
                            snapshot,
                            cursor,
                        },
                        external,
                    ) = client.attach().map_err(manager_error)?;
                    let target = AttachmentTarget {
                        session_id: session_id.clone(),
                        conversation_id: runtime.conversation_id().clone(),
                        runtime_incarnation: runtime.incarnation_id(),
                        attachment_id: attachment.attachment_id().clone(),
                    };
                    Ok((
                        Arc::new(Route {
                            target: target.clone(),
                            client,
                            attachment,
                            external,
                            capacity,
                        }),
                        MethodResult::Attached {
                            configuration: self
                                .host
                                .manager()
                                .configuration_application(&session_id),
                            target,
                            snapshot: Box::new(snapshot),
                            cursor,
                        },
                    ))
                })?;
                if let MethodResult::Attached { target, .. } = &result {
                    self.host.register_file_route(&self.route(target)?);
                }
                self.changed.notify_one();
                Ok(result)
            }
            _ => unreachable!("runtime methods admitted above"),
        }
    }

    fn route(&self, target: &AttachmentTarget) -> Result<Arc<Route>, RpcError> {
        let route = self
            .routes
            .lock()
            .expect("routes mutex")
            .active
            .get(&target.session_id)
            .cloned()
            .ok_or_else(|| domain(ErrorData::StaleAttachment))?;
        if route.target != *target {
            return Err(domain(ErrorData::StaleAttachment));
        }
        Ok(route)
    }

    /// The next configuration application this connection has not delivered yet.
    ///
    /// Level-triggered against the manager's current applications and this
    /// connection's per-scope version cursor.
    fn next_configuration_change(&self) -> Option<NotificationMethod> {
        for application in self.host.manager().configuration_applications() {
            let mut versions = self
                .configuration_versions
                .lock()
                .expect("configuration notification versions");
            let version = versions.entry(application.scope.clone()).or_default();
            if application.version > *version {
                *version = application.version;
                return Some(NotificationMethod::ConfigurationChanged { application });
            }
        }
        None
    }

    /// The next Session-metadata invalidation this connection has not delivered
    /// yet (Issue #386).
    ///
    /// Scanned level-triggered from the native log, exactly like configuration
    /// applications: no per-connection queue, no durable replay, no scheduler.
    /// One Session is announced per call, in publication order, and the cursor
    /// advances only for a notification actually returned.
    fn next_ownership_invalidation(&self) -> Option<NotificationMethod> {
        let mut delivered = self
            .ownership_invalidations_delivered
            .lock()
            .expect("ownership invalidation cursor");
        let (sequence, session_id, retired_through) = self
            .summary_invalidations
            .next_ownership_after(*delivered)?;
        *delivered = sequence;
        Some(NotificationMethod::OwnershipRetired {
            session_id,
            retired_through: retired_through.to_string(),
        })
    }

    fn next_summary_invalidation(&self) -> Option<NotificationMethod> {
        let mut delivered = self
            .summary_invalidations_delivered
            .lock()
            .expect("summary invalidation cursor");
        let (sequence, session_id, catalog_changed) =
            self.summary_invalidations.next_after(*delivered)?;
        *delivered = sequence;
        Some(NotificationMethod::SummaryInvalidated {
            session_id,
            catalog_changed,
        })
    }

    /// Wait for one routed notification. No per-Session event queues or pump tasks.
    /// # Panics
    /// Panics if a connection routing mutex is poisoned.
    pub async fn next_notification(&self) -> Notification {
        let _reader = self.reader.lock().await;
        let mut configuration_changes = self.host.manager().configuration_changes();
        let mut summary_invalidations = self.summary_invalidations.changes();
        loop {
            if let Some(notification) = self
                .next_ownership_invalidation()
                .or_else(|| self.next_summary_invalidation())
            {
                return Notification {
                    jsonrpc: JsonRpcVersion::V2,
                    notification,
                };
            }
            if let Some(notification) = self.next_configuration_change() {
                return Notification {
                    jsonrpc: JsonRpcVersion::V2,
                    notification,
                };
            }
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let mut routes: Vec<_> = self
                .routes
                .lock()
                .expect("routes mutex")
                .active
                .values()
                .cloned()
                .collect();
            if routes.is_empty() {
                tokio::select! {
                    () = &mut changed => {},
                    _ = configuration_changes.changed() => {},
                    _ = summary_invalidations.changed() => {},
                }
                continue;
            }
            // A continuously ready Session cannot starve other attachments.
            let start = self
                .next_route
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                % routes.len();
            routes.rotate_left(start);
            let pending: Vec<_> = routes
                .into_iter()
                .map(|route| {
                    Box::pin(async move {
                        // The exact registration this poll parks on travels with
                        // its delivery: a `Closed` only ends residency if it is
                        // still the attachment's current registration.
                        let observed = route.attachment.subscription();
                        let delivery = match (route.client.validate(), &observed) {
                            (Ok(()), Some(subscription)) => subscription.next().await,
                            _ => EventDelivery::Closed,
                        };
                        (route, delivery, observed)
                    })
                })
                .collect();
            let (route, delivery, observed) = tokio::select! {
                () = &mut changed => continue,
                _ = configuration_changes.changed() => continue,
                _ = summary_invalidations.changed() => continue,
                result = futures_util::future::select_all(pending) => result.0,
            };
            let target = route.target.clone();
            let notification = match delivery {
                EventDelivery::Event(event) => NotificationMethod::Event {
                    target,
                    cursor: event.cursor,
                    event: Box::new(event.event),
                },
                EventDelivery::ResyncRequired {
                    after_cursor,
                    earliest_serviceable,
                } => NotificationMethod::ResyncRequired {
                    target,
                    after_cursor,
                    earliest_serviceable,
                },
                EventDelivery::Closed
                    if observed
                        .is_some_and(|subscription| route.attachment.superseded(&subscription)) =>
                {
                    // A resync replaced this registration while the consumer was
                    // parked on it. The attachment is alive and has a newer
                    // registration; retiring its route here would end the
                    // attachment that asked for the repair.
                    continue;
                }
                EventDelivery::Closed | EventDelivery::Exhausted => {
                    release_route(&self.host, &self.routes, &route);
                    NotificationMethod::Closed { target }
                }
                EventDelivery::Pending => unreachable!("async delivery never returns Pending"),
            };
            return Notification {
                jsonrpc: JsonRpcVersion::V2,
                notification,
            };
        }
    }
}

impl Drop for AppServerConnection {
    fn drop(&mut self) {
        if Arc::strong_count(&self.initialized) == 1 {
            self.close();
        }
    }
}

fn runtime_target(method: &Method) -> Option<&AttachmentTarget> {
    match method {
        Method::ConfigurationGet { target, .. }
        | Method::ModelGet { target, .. }
        | Method::ModelCatalog { target, .. }
        | Method::ModelSet { target, .. }
        | Method::Capability { target, .. }
        | Method::ArtifactRead { target, .. }
        | Method::SessionUploadPrepare { target, .. }
        | Method::SessionUploadStatus { target, .. }
        | Method::Trace { target, .. }
        | Method::TraceDetail { target, .. }
        | Method::Transcript { target, .. }
        | Method::ConversationTurns { target, .. }
        | Method::Goal { target, .. }
        | Method::JobStatus { target, .. }
        | Method::JobList { target, .. }
        | Method::JobWait { target, .. }
        | Method::JobCancel { target, .. }
        | Method::AgentStatus { target, .. }
        | Method::AgentList { target, .. }
        | Method::AgentSendMessage { target, .. }
        | Method::AgentWait { target, .. }
        | Method::AgentInterrupt { target, .. }
        | Method::AgentTranscript { target, .. }
        | Method::SubagentDispose { target, .. }
        | Method::CompactContext { target, .. }
        | Method::SessionBoundaries { target, .. }
        | Method::SessionSnapshot { target, .. }
        | Method::SessionSubscribe { target, .. }
        | Method::TurnStart { target, .. }
        | Method::TurnSteer { target, .. }
        | Method::InboundEdit { target, .. }
        | Method::InboundRemove { target, .. }
        | Method::TurnCancel { target, .. }
        | Method::InteractionRespond { target, .. }
        | Method::InteractionCancel { target, .. } => Some(target),
        _ => None,
    }
}

#[allow(clippy::too_many_lines)]
async fn dispatch_runtime(
    method: Method,
    route: Arc<Route>,
    authority: Arc<crate::runtime_client::host::ClientInner>,
    changed: Arc<tokio::sync::Notify>,
    sessions: SessionController,
    manager: crate::local_runtime::session_runtime_manager::SessionRuntimeManager,
    upload_host: AppServerHost,
) -> Result<MethodResult, RpcError> {
    match method {
        Method::ConfigurationGet { target } => {
            let mut projection = authority.configuration().map_err(client_error)?;
            projection.application = manager.configuration_application(&target.session_id);
            projection.process_bindings = Some(manager.process_policy());
            Ok(MethodResult::EffectiveConfiguration {
                projection: Box::new(projection),
            })
        }
        Method::ArtifactRead {
            target: _,
            artifact_id,
        } => Ok(MethodResult::ArtifactBytes {
            data: authority
                .artifact_read(&artifact_id)
                .map_err(client_error)?,
        }),
        Method::SessionUploadPrepare {
            target: _,
            operation_id,
            files,
        } => {
            let transfer =
                super::upload_transfer::prepare(&upload_host, route, operation_id, files)
                    .await
                    .map_err(|_| domain(ErrorData::InvalidParams))?;
            Ok(MethodResult::UploadPrepared { transfer })
        }
        Method::SessionUploadStatus {
            target,
            operation_id,
        } => {
            let outcome =
                super::upload_transfer::status(&upload_host, &target.session_id, &operation_id)
                    .await
                    .map_err(|_| domain(ErrorData::InvalidParams))?;
            Ok(MethodResult::UploadStatus { outcome })
        }
        Method::ModelGet { target: _ } => native_result(authority.model_get()),
        Method::ModelCatalog { target: _ } => native_result(authority.model_catalog()),
        Method::ModelSet { target, config } => Ok(MethodResult::Model {
            model: Box::new(
                manager
                    .set_model(&target.session_id, *config)
                    .await
                    .map_err(|rejection| domain(ErrorData::ConfigurationAdoption { rejection }))?,
            ),
        }),
        Method::Capability { target: _ } => native_result(authority.capability()),
        Method::Trace {
            target: _,
            records,
            before,
            limit,
        } => native_result(authority.trace_page(before, limit, records)),
        Method::TraceDetail {
            target: _,
            record_id,
        } => native_result(authority.trace_detail(record_id)),
        Method::Transcript {
            target: _,
            at,
            limit,
        } => Ok(MethodResult::TranscriptWindow {
            window: authority
                .conversation_window(&at, limit)
                .map_err(client_error)?,
        }),
        Method::ConversationTurns {
            target: _,
            offset,
            limit,
        } => Ok(MethodResult::ConversationTurns {
            page: authority
                .conversation_turns(offset, limit)
                .map_err(client_error)?,
        }),
        Method::Goal { target: _, control } => native_result(authority.goal_control(control)),
        Method::JobStatus { job_id, .. } => native_result(authority.job_status(&job_id)),
        Method::JobList { .. } => native_result(Ok(authority.job_list())),
        Method::JobWait { job_id, .. } => native_result(authority.job_wait(&job_id, false).await),
        Method::JobCancel { job_id, .. } => native_result(authority.job_wait(&job_id, true).await),
        Method::AgentStatus { agent_id, .. } => native_result(authority.agent_status(&agent_id)),
        Method::AgentList { .. } => native_result(authority.agent_list()),
        Method::AgentSendMessage {
            agent_id, message, ..
        } => native_result(authority.agent_send_message(&agent_id, message).await),
        Method::AgentWait { agent_id, .. } => {
            native_result(authority.agent_wait(&agent_id, false).await)
        }
        Method::AgentInterrupt { agent_id, .. } => {
            native_result(authority.agent_wait(&agent_id, true).await)
        }
        Method::AgentTranscript {
            agent_id,
            before,
            limit,
            ..
        } => match authority.agent_transcript_page(&agent_id, before, limit) {
            Err(RuntimeClientError::RuntimeFailure { .. }) => {
                Err(domain(ErrorData::AgentHistoryUnavailable { agent_id }))
            }
            result => native_result(result),
        },
        Method::SubagentDispose {
            target: _,
            subagent_id,
        } => native_result(authority.subagent_workspace_dispose(&subagent_id).await),
        Method::CompactContext {
            target: _,
            request_id,
        } => native_result(authority.compact_context(Some(request_id)).await),
        Method::SessionBoundaries {
            target: _,
            offset,
            limit,
        } => {
            let (surface_revision, boundaries, next_offset) = authority
                .user_message_boundaries(offset, limit)
                .map_err(client_error)?;
            Ok(MethodResult::Boundaries {
                surface_revision,
                boundaries,
                next_offset,
            })
        }
        Method::SessionSnapshot {
            target: _,
            trace_records,
        } => native_result(
            authority
                .snapshot_with_trace(&trace_records)
                .map(|(snapshot, cursor)| RuntimeClientResult::Snapshot { snapshot, cursor }),
        ),
        Method::SessionSubscribe {
            target: _,
            after_cursor,
        } => {
            route
                .attachment
                .subscribe_events(after_cursor)
                .map_err(client_error)?;
            changed.notify_one();
            Ok(MethodResult::Subscribed { after_cursor })
        }
        Method::TurnStart { target, content } | Method::TurnSteer { target, content } => {
            let receipts: Vec<_> = content
                .iter()
                .filter_map(|block| match block {
                    UserInputBlock::Upload(receipt) => Some(receipt.clone()),
                    UserInputBlock::Text(_) => None,
                })
                .collect();
            let mut uploads = sessions
                .uploaded_content(&target.session_id, &receipts)
                .await
                .map_err(session_error)?
                .into_iter();
            let mut canonical = Vec::with_capacity(content.len());
            for block in content {
                match block {
                    UserInputBlock::Text(text) => {
                        canonical.push(crate::message::types::UserContentBlock::Text(text));
                    }
                    UserInputBlock::Upload(_) => {
                        canonical.push(uploads.next().expect("validated receipt collection"));
                    }
                }
            }
            native_result(authority.submit_session_inbound(canonical))
        }
        Method::InboundEdit {
            target: _,
            expected,
            text,
        } => Ok(MethodResult::InboundMutation {
            outcome: authority
                .edit_pending(&expected, &text)
                .map_err(client_error)?,
        }),
        Method::InboundRemove {
            target: _,
            expected,
        } => Ok(MethodResult::InboundMutation {
            outcome: authority.remove_pending(&expected).map_err(client_error)?,
        }),
        Method::TurnCancel { target: _ } => native_result(authority.cancel_current_attempt()),
        Method::InteractionRespond {
            target: _,
            interaction,
            response,
        } => native_result(authority.respond_interaction(&interaction, response).await),
        Method::InteractionCancel {
            target: _,
            interaction,
        } => native_result(authority.cancel_interaction(&interaction).await),

        _ => unreachable!("only admitted runtime methods"),
    }
}

fn transition(
    result: crate::local_runtime::session_controller::SessionTransitionResult,
) -> MethodResult {
    MethodResult::SessionTransition {
        session: result.session,
        editor_content: result.editor_content,
        durability_diagnostic: result.durability_diagnostic,
    }
}
fn deletion(result: crate::local_runtime::session::deletion::SessionDeleteResult) -> MethodResult {
    MethodResult::Deletion {
        result: crate::app_server::session_deletion::project(result),
    }
}
fn native_result(
    result: Result<RuntimeClientResult, RuntimeClientError>,
) -> Result<MethodResult, RpcError> {
    Ok(match result.map_err(client_error)? {
        RuntimeClientResult::Model { model } | RuntimeClientResult::ModelSet { model } => {
            MethodResult::Model { model }
        }
        RuntimeClientResult::ModelCatalog { catalog } => MethodResult::Models { catalog },
        RuntimeClientResult::Capability { capabilities } => {
            MethodResult::Capabilities { capabilities }
        }
        RuntimeClientResult::ContextCompacted { context } => MethodResult::Context { context },
        RuntimeClientResult::TracePage { page } => MethodResult::Trace { page },
        RuntimeClientResult::TraceDetail { detail } => MethodResult::TraceDetail { detail },
        RuntimeClientResult::TranscriptPage { page } => MethodResult::Transcript { page },
        RuntimeClientResult::Goal { view } => MethodResult::Goal { view },
        RuntimeClientResult::Job { job } => MethodResult::Job { job },
        RuntimeClientResult::Jobs {
            jobs,
            returned,
            matched,
            limit,
            truncated,
        } => MethodResult::Jobs {
            jobs,
            returned,
            matched,
            limit,
            truncated,
        },
        RuntimeClientResult::Agent { agent } => MethodResult::Agent {
            agent: Box::new(agent),
        },
        RuntimeClientResult::Agents {
            agents,
            returned,
            matched,
            limit,
            truncated,
        } => MethodResult::Agents {
            agents,
            returned,
            matched,
            limit,
            truncated,
        },
        RuntimeClientResult::AgentMessage { accepted } => MethodResult::AgentMessage {
            agent_id: accepted.agent_id,
            activation_id: accepted.activation_id,
            resumed: accepted.resumed,
        },
        RuntimeClientResult::AgentWait {
            agent_id,
            activation_id,
            outcome,
            agent,
        } => MethodResult::AgentWait {
            agent_id,
            activation_id,
            outcome,
            agent: Box::new(agent),
        },
        RuntimeClientResult::SubagentWorkspaceDisposed {
            subagent_id,
            workspace,
            outcome,
        } => MethodResult::WorkspaceDisposed {
            subagent_id,
            workspace,
            outcome,
        },
        RuntimeClientResult::Snapshot { snapshot, cursor } => MethodResult::Snapshot {
            snapshot: Box::new(snapshot),
            cursor,
        },
        RuntimeClientResult::InboundAccepted {
            message_id,
            inbound_sequence,
        } => MethodResult::InboundAccepted {
            message_id,
            inbound_sequence,
        },
        RuntimeClientResult::AttemptCancellationAccepted { attempt_id } => {
            MethodResult::CancellationAccepted { attempt_id }
        }
        RuntimeClientResult::InteractionResponseAccepted { interaction } => {
            MethodResult::InteractionSettled { interaction }
        }

        _ => return Err(domain(ErrorData::OperationFailed)),
    })
}
pub(super) fn client_error(error: RuntimeClientError) -> RpcError {
    domain(match error {
        RuntimeClientError::AgentNotDelivered { agent_id } => {
            ErrorData::AgentNotDelivered { agent_id }
        }
        RuntimeClientError::AgentDeliveryUnknown { agent_id } => {
            ErrorData::AgentDeliveryUnknown { agent_id }
        }
        RuntimeClientError::JobPublicationAbandoned { job_id } => {
            ErrorData::JobPublicationAbandoned { job_id }
        }
        RuntimeClientError::AgentStopping { agent_id } => ErrorData::AgentStopping { agent_id },
        RuntimeClientError::AgentSettlement { agent_id } => ErrorData::AgentSettlement { agent_id },
        RuntimeClientError::UnknownAgent { agent_id } => ErrorData::UnknownAgent { agent_id },
        RuntimeClientError::ConfigurationAdoption { rejection } => {
            ErrorData::ConfigurationAdoption { rejection }
        }
        RuntimeClientError::InteractionNotPending { interaction } => {
            ErrorData::InteractionNotPending { interaction }
        }
        RuntimeClientError::InteractionAuditFailed { interaction } => {
            ErrorData::InteractionAuditFailed { interaction }
        }
        RuntimeClientError::AttachmentInUse { .. } => ErrorData::ControllerInUse,
        RuntimeClientError::NotAttached => ErrorData::StaleAttachment,
        RuntimeClientError::ResyncRequired { .. } => ErrorData::ResyncRequired,
        RuntimeClientError::InvalidRequest { .. }
        | RuntimeClientError::InteractionInvalidResponse { .. } => ErrorData::InvalidParams,
        _ => ErrorData::InvalidState,
    })
}
pub(super) fn manager_error(error: RuntimeManagerError) -> RpcError {
    match error {
        RuntimeManagerError::ResidencyCapacity => domain(ErrorData::ResidencyCapacity),
        RuntimeManagerError::StaleIncarnation => domain(ErrorData::StaleRuntime),
        RuntimeManagerError::Client(error) => client_error(error),
        RuntimeManagerError::TransitionFailed(detail) => {
            rpc_error(-32000, &detail, Some(ErrorData::OperationFailed))
        }
        RuntimeManagerError::SessionAlreadyResident { .. } => domain(ErrorData::OperationFailed),
    }
}
fn session_error(error: crate::local_runtime::session::SessionError) -> RpcError {
    use crate::local_runtime::session::SessionError;
    domain(match error {
        SessionError::UnknownSession { session_id } => ErrorData::UnknownSession { session_id },
        SessionError::UnknownNode {
            session_id,
            node_id,
        } => ErrorData::UnknownNode {
            session_id,
            node_id,
        },
        SessionError::StaleSettings { expected, actual } => {
            ErrorData::StaleSettings { expected, actual }
        }
        SessionError::InvalidName => ErrorData::InvalidParams,
        error if error.committed() => ErrorData::CommittedDurabilityUncertain,
        _ => ErrorData::OperationFailed,
    })
}
pub(super) fn domain(data: ErrorData) -> RpcError {
    let message = match &data {
        ErrorData::SessionFileRead { reason } => match reason {
            crate::tools::session_files::SessionFileReadFailure::Missing => {
                "Delivered Session file is missing"
            }
            crate::tools::session_files::SessionFileReadFailure::Unauthorized => {
                "Session file access is not authorized"
            }
            crate::tools::session_files::SessionFileReadFailure::Unavailable => {
                "Original delivery Session filesystem is unavailable"
            }
            crate::tools::session_files::SessionFileReadFailure::NotRegular => {
                "Delivered target is not a regular file"
            }
            crate::tools::session_files::SessionFileReadFailure::Replaced => {
                "Session file was replaced during read; reopen explicitly"
            }
            crate::tools::session_files::SessionFileReadFailure::TooLarge => {
                "Delivered Session file exceeds 512 KiB"
            }
            crate::tools::session_files::SessionFileReadFailure::Capacity => {
                "Session file read capacity reached"
            }
            crate::tools::session_files::SessionFileReadFailure::ReadFailed => {
                "Session file read failed"
            }
        },
        ErrorData::AgentNotDelivered { .. } => "Agent input was not delivered",
        ErrorData::AgentDeliveryUnknown { .. } => {
            "Agent input acceptance was not acknowledged; delivery is unknown, do not replay automatically"
        }
        ErrorData::JobPublicationAbandoned { .. } => {
            "Job terminal publication was abandoned; no durable terminal result is available"
        }
        ErrorData::AgentStopping { .. } => {
            "Agent is stopping or admitting an activation; retry after settlement"
        }
        ErrorData::AgentSettlement { .. } => {
            "Agent is unavailable; physical settlement, publication, or workspace authority requires explicit repair"
        }
        ErrorData::UnknownAgent { .. } => "Unknown Agent in this conversation",
        _ => "Operation rejected",
    };
    rpc_error(-32000, message, Some(data))
}
fn rpc_error(code: i32, message: &str, data: Option<ErrorData>) -> RpcError {
    RpcError {
        code,
        message: message.into(),
        data,
    }
}
fn failure(id: Option<RequestId>, error: RpcError) -> Response {
    Response::Failure(Failure {
        jsonrpc: JsonRpcVersion::V2,
        id,
        error,
    })
}

pub(super) fn host_error(error: HostAdmissionError) -> RpcError {
    domain(match error {
        HostAdmissionError::ServerDraining => ErrorData::ServerDraining,
        HostAdmissionError::RequestCapacity => ErrorData::RequestCapacity,
        HostAdmissionError::AttachmentCapacity => ErrorData::AttachmentCapacity,
    })
}

fn source_error(error: crate::local_runtime::configuration::settings::SettingsError) -> RpcError {
    use crate::local_runtime::configuration::settings::SettingsError;
    domain(match error {
        SettingsError::Conflict {
            scope,
            expected,
            actual,
        } => ErrorData::SourceConflict {
            scope,
            expected,
            actual,
        },
        SettingsError::Invalid => ErrorData::InvalidParams,
        SettingsError::Io => ErrorData::OperationFailed,
        SettingsError::Committed => ErrorData::CommittedDurabilityUncertain,
    })
}

fn source_settings_error(
    error: crate::local_runtime::session_runtime_manager::SourceSettingsError,
) -> RpcError {
    match error {
        crate::local_runtime::session_runtime_manager::SourceSettingsError::Source(error) => {
            source_error(error)
        }
    }
}

#[cfg(test)]
mod capacity_tests {
    #[test]
    fn agent_delivery_failures_preserve_the_owner_three_way_contract() {
        let agent_id = crate::runtime::identity::AgentId::new("agent-delivery");
        let missing = super::client_error(
            crate::runtime_client::types::RuntimeClientError::AgentNotDelivered {
                agent_id: agent_id.clone(),
            },
        );
        assert_eq!(
            missing.data,
            Some(super::ErrorData::AgentNotDelivered {
                agent_id: agent_id.clone()
            })
        );
        assert!(missing.message.contains("input was not delivered"));
        let unknown = super::client_error(
            crate::runtime_client::types::RuntimeClientError::AgentDeliveryUnknown {
                agent_id: agent_id.clone(),
            },
        );
        assert_eq!(
            unknown.data,
            Some(super::ErrorData::AgentDeliveryUnknown { agent_id })
        );
        assert!(unknown.message.contains("do not replay automatically"));
    }

    #[test]
    fn job_publication_abandonment_preserves_typed_failure_and_identity() {
        let job_id = crate::runtime::identity::ToolExecutionId::new(
            "exec_0199c989-03a0-7000-8000-000000000001",
        );
        let error = super::native_result(Err(
            crate::runtime_client::types::RuntimeClientError::JobPublicationAbandoned {
                job_id: job_id.clone(),
            },
        ))
        .unwrap_err();
        assert_eq!(
            error.data,
            Some(super::ErrorData::JobPublicationAbandoned { job_id })
        );
        assert!(error.message.contains("no durable terminal result"));
    }

    #[test]
    fn job_listing_preserves_omission_metadata_on_the_wire() {
        let result = super::native_result(Ok(
            crate::runtime_client::types::RuntimeClientResult::Jobs {
                jobs: Vec::new(),
                returned: 0,
                matched: 67,
                limit: 64,
                truncated: true,
            },
        ))
        .unwrap();
        assert!(matches!(
            result,
            super::MethodResult::Jobs {
                returned: 0,
                matched: 67,
                limit: 64,
                truncated: true,
                ..
            }
        ));
    }

    #[test]
    fn agent_settlement_wire_error_does_not_advise_retry() {
        let error = super::client_error(
            crate::runtime_client::types::RuntimeClientError::AgentSettlement {
                agent_id: crate::runtime::identity::AgentId::new("agent-unavailable"),
            },
        );
        assert!(
            matches!(&error.data, Some(super::ErrorData::AgentSettlement { agent_id }) if agent_id.as_str() == "agent-unavailable")
        );
        assert!(error.message.contains("explicit repair"));
        assert!(!error.message.contains("retry"));
    }

    #[test]
    fn production_route_table_starts_with_32_attachment_slots() {
        let routes = super::RouteTable::default();
        assert_eq!(routes.attachment_limit, 32);
        assert!(routes.active.is_empty());
        assert!(routes.reserved.is_empty());
    }
}
