//! One connection negotiates once and routes independent Session attachments.
//! Only the attachment map is connection-owned; all semantic work is delegated.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::protocol::{
    APP_SERVER_PROTOCOL_VERSION, AttachmentTarget, ErrorData, Failure, InitializeParams,
    JsonRpcVersion, Method, MethodResult, Notification, NotificationMethod, Request, RequestId,
    Response, RpcError, ServerCapabilities, Success,
};
use crate::local_runtime::session::SessionId;
use crate::local_runtime::session_controller::SessionController;
use crate::local_runtime::session_runtime_manager::{
    ManagedRuntimeClient, RuntimeManagerError, SessionRuntimeManager,
};
use crate::runtime_client::attachment::RuntimeAttachment;
use crate::runtime_client::host::EventDelivery;
use crate::runtime_client::types::{RuntimeClientError, RuntimeClientResult};

const MAX_ATTACHMENTS: usize = 32;

fn valid_request_id(id: &RequestId) -> bool {
    !matches!(id, RequestId::Integer(n) if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(n))
}

#[derive(Default)]
struct RouteTable {
    active: BTreeMap<SessionId, Arc<Route>>,
    reserved: std::collections::BTreeSet<SessionId>,
}

struct AttachReservation {
    table: Arc<Mutex<RouteTable>>,
    session: SessionId,
    committed: bool,
}

impl AttachReservation {
    fn new(table: &Arc<Mutex<RouteTable>>, session: &SessionId) -> Result<Self, RpcError> {
        let mut routes = table.lock().expect("routes mutex");
        if routes.active.contains_key(session) || routes.reserved.contains(session) {
            return Err(domain(ErrorData::ControllerInUse));
        }
        if routes.active.len() + routes.reserved.len() >= MAX_ATTACHMENTS {
            return Err(domain(ErrorData::InvalidState));
        }
        routes.reserved.insert(session.clone());
        Ok(Self {
            table: table.clone(),
            session: session.clone(),
            committed: false,
        })
    }

    fn commit(mut self, route: Arc<Route>) {
        let mut routes = self.table.lock().expect("routes mutex");
        routes.reserved.remove(&self.session);
        routes.active.insert(self.session.clone(), route);
        self.committed = true;
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

fn release_route(table: &Mutex<RouteTable>, route: &Arc<Route>) {
    let mut routes = table.lock().expect("routes mutex");
    if routes
        .active
        .get(&route.target.session_id)
        .is_some_and(|current| Arc::ptr_eq(current, route))
    {
        routes.active.remove(&route.target.session_id);
        route.attachment.detach();
    }
}

struct Route {
    target: AttachmentTarget,
    client: ManagedRuntimeClient,
    attachment: RuntimeAttachment,
}

/// A transport may share this connection across concurrent request handlers.
/// A single notification consumer drives fan-in directly, without pump tasks.
pub struct AppServerConnection {
    manager: SessionRuntimeManager,
    sessions: SessionController,
    initialized: Mutex<Option<InitializeParams>>,
    routes: Arc<Mutex<RouteTable>>,
    changed: Arc<tokio::sync::Notify>,
    reader: tokio::sync::Mutex<()>,
    next_route: std::sync::atomic::AtomicUsize,
}

impl AppServerConnection {
    #[cfg(test)]
    pub(crate) fn attachment_counts(&self) -> (usize, usize) {
        let routes = self.routes.lock().expect("routes mutex");
        (routes.active.len(), routes.reserved.len())
    }

    #[must_use]
    pub fn new(manager: SessionRuntimeManager) -> Self {
        Self {
            sessions: manager.session_controller(),
            manager,
            initialized: Mutex::new(None),
            routes: Arc::new(Mutex::new(RouteTable::default())),
            changed: Arc::new(tokio::sync::Notify::new()),
            reader: tokio::sync::Mutex::new(()),
            next_route: std::sync::atomic::AtomicUsize::new(0),
        }
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
        match Box::pin(self.dispatch(request.call)).await {
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
                protocol_version: APP_SERVER_PROTOCOL_VERSION,
                capabilities: ServerCapabilities::default(),
            });
        }
        if self.initialized.lock().expect("initialize mutex").is_none() {
            return Err(domain(ErrorData::NotInitialized));
        }
        if let Some(target) = runtime_target(&method) {
            let route = self.route(target)?;
            let client = route.client.clone();
            let changed = self.changed.clone();
            let receiver = client
                .start_operation(
                    move || async move { dispatch_runtime(method, route, changed).await },
                )
                .map_err(manager_error)?;
            return receiver
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))?;
        }
        match method {
            Method::Initialize(_) => unreachable!(),
            Method::SessionDetach { target } => {
                // Removing a connection relationship needs no live-runtime lease.
                let route = self.route(&target)?;
                release_route(&self.routes, &route);
                self.changed.notify_one();
                Ok(MethodResult::Detached {})
            }
            Method::SessionDelete {
                session_id,
                expected_target_revision,
            } => self
                .sessions
                .delete_session(&session_id, &expected_target_revision)
                .await
                .map(deletion)
                .map_err(session_error),
            Method::SessionUnload { target } => {
                let route = self.route(&target)?;
                let manager = self.manager.clone();
                let routes = self.routes.clone();
                let changed = self.changed.clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                // Cleanup is server-owned even if the initiating caller goes away.
                tokio::spawn(async move {
                    let result = manager
                        .unload_incarnation(&target.conversation_id, target.runtime_incarnation)
                        .await
                        .map_err(manager_error);
                    // Every terminal result retires this exact external route,
                    // including stale residency and fail-closed shutdown errors.
                    release_route(&routes, &route);
                    changed.notify_one();
                    let _ = sender.send(result.map(|()| MethodResult::Unloaded {}));
                });
                receiver
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))?
            }
            Method::ServerInfo {} => Ok(MethodResult::ServerInfo {
                capabilities: ServerCapabilities::default(),
            }),
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
            Method::SessionRead { session_id } => Ok(MethodResult::Session {
                session: self
                    .sessions
                    .read_session(&session_id)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionCreate { settings } => self
                .sessions
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
            } => self
                .sessions
                .fork_session(
                    &session_id,
                    node_id.as_ref(),
                    surface_revision,
                    boundary.as_ref(),
                )
                .await
                .map(transition)
                .map_err(session_error),
            Method::SessionBranch {
                session_id,
                node_id,
                surface_revision,
                boundary,
            } => self
                .sessions
                .branch_session_node(&session_id, &node_id, surface_revision, &boundary)
                .await
                .map(transition)
                .map_err(session_error),
            Method::SessionDeletePreview { session_id } => {
                Ok(deletion(self.sessions.delete_preview(&session_id).await))
            }
            Method::SessionRecoverDeletion { session_id } => {
                Ok(deletion(self.sessions.recover_deletion(&session_id).await))
            }
            Method::SettingsRead { session_id } => {
                let (revision, settings) = self
                    .sessions
                    .read_settings(&session_id)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::Settings { revision, settings })
            }
            Method::SettingsReplace {
                session_id,
                expected_revision,
                settings,
            } => Ok(MethodResult::SettingsReplaced {
                revision: self
                    .sessions
                    .replace_settings(&session_id, expected_revision, settings)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionAttach {
                session_id,
                node_id,
            } => {
                let reservation = AttachReservation::new(&self.routes, &session_id)?;
                // Reservation is request-scoped; once claimed, Loading is
                // manager-scoped. Dropping this request does not roll it back.
                let runtime = self
                    .manager
                    .load(&session_id, node_id.as_ref())
                    .await
                    .map_err(manager_error)?;
                let client = runtime.client();
                let crate::runtime_client::attachment::AttachedSnapshot {
                    attachment,
                    snapshot,
                    cursor,
                } = client.attach().map_err(manager_error)?;
                let target = AttachmentTarget {
                    session_id: session_id.clone(),
                    conversation_id: runtime.conversation_id().clone(),
                    runtime_incarnation: runtime.incarnation_id(),
                    attachment_id: attachment.attachment_id().clone(),
                };
                reservation.commit(Arc::new(Route {
                    target: target.clone(),
                    client,
                    attachment,
                }));
                self.changed.notify_one();
                Ok(MethodResult::Attached {
                    target,
                    snapshot: Box::new(snapshot),
                    cursor,
                })
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

    /// Wait for one routed notification. No per-Session event queues or pump tasks.
    /// # Panics
    /// Panics if a connection routing mutex is poisoned.
    pub async fn next_notification(&self) -> Notification {
        let _reader = self.reader.lock().await;
        loop {
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
                changed.await;
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
                        let delivery = if route.client.validate().is_err() {
                            EventDelivery::Closed
                        } else {
                            route.attachment.next_event().await
                        };
                        (route, delivery)
                    })
                })
                .collect();
            let (route, delivery) = tokio::select! {
                () = &mut changed => continue,
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
                EventDelivery::Closed | EventDelivery::Exhausted => {
                    release_route(&self.routes, &route);
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
        for route in self.routes.lock().expect("routes mutex").active.values() {
            route.attachment.detach();
        }
    }
}

fn runtime_target(method: &Method) -> Option<&AttachmentTarget> {
    match method {
        Method::DefaultsRead { target, .. }
        | Method::DefaultSave { target, .. }
        | Method::ModelGet { target, .. }
        | Method::ModelCatalog { target, .. }
        | Method::ModelSet { target, .. }
        | Method::ApprovalModeSet { target, .. }
        | Method::Capability { target, .. }
        | Method::Transcript { target, .. }
        | Method::Goal { target, .. }
        | Method::BackgroundStatus { target, .. }
        | Method::BackgroundCancel { target, .. }
        | Method::SubagentStatus { target, .. }
        | Method::SubagentCancel { target, .. }
        | Method::SubagentDispose { target, .. }
        | Method::CompactContext { target, .. }
        | Method::SessionSnapshot { target, .. }
        | Method::SessionSubscribe { target, .. }
        | Method::TurnStart { target, .. }
        | Method::TurnSteer { target, .. }
        | Method::TurnCancel { target, .. }
        | Method::InteractionRespond { target, .. }
        | Method::InteractionCancel { target, .. }
        | Method::ResourcesReload { target, .. } => Some(target),
        _ => None,
    }
}

#[allow(clippy::too_many_lines)]
async fn dispatch_runtime(
    method: Method,
    route: Arc<Route>,
    changed: Arc<tokio::sync::Notify>,
) -> Result<MethodResult, RpcError> {
    match method {
        Method::DefaultsRead { target: _, scope } => {
            native_result(route.attachment.defaults_read(scope).await)
        }
        Method::DefaultSave {
            target: _,
            scope,
            expected_revision,
            setting,
        } => native_result(
            route
                .attachment
                .defaults_save(scope, expected_revision, setting)
                .await,
        ),
        Method::ModelGet { target: _ } => native_result(route.attachment.model_get()),
        Method::ModelCatalog { target: _ } => native_result(route.attachment.model_catalog()),
        Method::ModelSet { target: _, config } => {
            native_result(route.attachment.model_set(*config))
        }
        Method::ApprovalModeSet { target: _, mode } => {
            native_result(route.attachment.approval_mode_set(mode))
        }
        Method::Capability { target: _ } => native_result(route.attachment.capability()),
        Method::Transcript {
            target: _,
            before,
            limit,
        } => native_result(route.attachment.transcript_page(before, limit)),
        Method::Goal { target: _, control } => {
            native_result(route.attachment.goal_control(control))
        }
        Method::BackgroundStatus {
            target: _,
            execution_id,
        } => native_result(route.attachment.background_status(&execution_id)),
        Method::BackgroundCancel {
            target: _,
            execution_id,
        } => native_result(route.attachment.background_cancel(&execution_id)),
        Method::SubagentStatus {
            target: _,
            subagent_id,
        } => native_result(route.attachment.subagent_status(&subagent_id)),
        Method::SubagentCancel {
            target: _,
            subagent_id,
        } => native_result(route.attachment.subagent_cancel(&subagent_id)),
        Method::SubagentDispose {
            target: _,
            subagent_id,
        } => native_result(
            route
                .attachment
                .subagent_workspace_dispose(&subagent_id)
                .await,
        ),
        Method::CompactContext { target: _ } => {
            native_result(route.attachment.compact_context().await)
        }
        Method::SessionSnapshot { target: _ } => native_result(route.attachment.snapshot()),
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
        Method::TurnStart { target: _, content } | Method::TurnSteer { target: _, content } => {
            native_result(route.attachment.submit_inbound(content))
        }
        Method::TurnCancel { target: _ } => {
            native_result(route.attachment.cancel_current_attempt())
        }
        Method::InteractionRespond {
            target: _,
            interaction,
            response,
        } => native_result(
            route
                .attachment
                .respond_interaction(&interaction, response)
                .await,
        ),
        Method::InteractionCancel {
            target: _,
            interaction,
        } => native_result(route.attachment.cancel_interaction(&interaction).await),
        Method::ResourcesReload { target: _ } => {
            native_result(route.attachment.reload_resources().await)
        }
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
        result: crate::local_runtime::supervisor::project_session_deletion(result),
    }
}
fn native_result(
    result: Result<RuntimeClientResult, RuntimeClientError>,
) -> Result<MethodResult, RpcError> {
    Ok(match result.map_err(client_error)? {
        RuntimeClientResult::Defaults { document } => MethodResult::Defaults { document },
        RuntimeClientResult::DefaultSaved { result } => MethodResult::DefaultSaved { result },
        RuntimeClientResult::Model { model } | RuntimeClientResult::ModelSet { model } => {
            MethodResult::Model { model }
        }
        RuntimeClientResult::ModelCatalog { catalog } => MethodResult::Models { catalog },
        RuntimeClientResult::ApprovalModeSet {
            effective_approval_mode,
            pending_approval_mode,
            revision,
        } => MethodResult::ApprovalMode {
            effective_approval_mode,
            pending_approval_mode,
            revision,
        },
        RuntimeClientResult::Capability { capabilities } => {
            MethodResult::Capabilities { capabilities }
        }
        RuntimeClientResult::ContextCompacted { context } => MethodResult::Context { context },
        RuntimeClientResult::TranscriptPage { page } => MethodResult::Transcript { page },
        RuntimeClientResult::Goal { view } => MethodResult::Goal { view },
        RuntimeClientResult::BackgroundStatus { execution }
        | RuntimeClientResult::BackgroundCancelAccepted { execution } => {
            MethodResult::Background { execution }
        }
        RuntimeClientResult::SubagentStatus { subagent }
        | RuntimeClientResult::SubagentCancelAccepted { subagent } => MethodResult::Subagent {
            subagent: Box::new(subagent),
        },
        RuntimeClientResult::SubagentWorkspaceDisposed { subagent, outcome } => {
            MethodResult::WorkspaceDisposed {
                subagent: Box::new(subagent),
                outcome,
            }
        }
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
        RuntimeClientResult::ResourcesReloaded {
            resource_revision,
            capability_revision,
        } => MethodResult::ResourcesReloaded {
            resource_revision,
            capability_revision,
        },
        _ => return Err(domain(ErrorData::OperationFailed)),
    })
}
fn client_error(error: RuntimeClientError) -> RpcError {
    domain(match error {
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
fn manager_error(error: RuntimeManagerError) -> RpcError {
    match error {
        RuntimeManagerError::StaleIncarnation => domain(ErrorData::StaleRuntime),
        RuntimeManagerError::Client(error) => client_error(error),
        _ => domain(ErrorData::OperationFailed),
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
fn domain(data: ErrorData) -> RpcError {
    rpc_error(-32000, "Operation rejected", Some(data))
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
