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
    routes: Mutex<BTreeMap<SessionId, Arc<Route>>>,
    changed: tokio::sync::Notify,
    reader: tokio::sync::Mutex<()>,
    next_route: std::sync::atomic::AtomicUsize,
}

impl AppServerConnection {
    #[must_use]
    pub fn new(manager: SessionRuntimeManager) -> Self {
        Self {
            sessions: manager.session_controller(),
            manager,
            initialized: Mutex::new(None),
            routes: Mutex::new(BTreeMap::new()),
            changed: tokio::sync::Notify::new(),
            reader: tokio::sync::Mutex::new(()),
            next_route: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Decode a strict JSON-RPC request before performing any semantic action.
    /// Notifications never receive responses and do not invoke request-only methods.
    /// # Panics
    /// Panics if a connection routing mutex is poisoned.
    pub async fn handle_json(&self, json: &str) -> Option<Response<MethodResult>> {
        let value: serde_json::Value = match serde_json::from_str(json) {
            Ok(value) => value,
            Err(_) => return Some(failure(None, rpc_error(-32700, "Parse error", None))),
        };
        let Some(object) = value.as_object() else {
            return Some(failure(None, rpc_error(-32600, "Invalid Request", None)));
        };
        let id = object
            .get("id")
            .and_then(|value| serde_json::from_value::<RequestId>(value.clone()).ok());
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
    pub async fn handle_request(&self, request: Request) -> Response<MethodResult> {
        let id = request.id;
        if matches!(&id, RequestId::Integer(n) if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(n))
        {
            return failure(Some(id), domain(ErrorData::InvalidParams));
        }
        match Box::pin(self.dispatch(request.call)).await {
            Ok(result) => Response::Success(Success {
                jsonrpc: JsonRpcVersion::V2,
                id,
                result,
            }),
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
        match method {
            Method::Initialize(_) => unreachable!(),
            Method::DefaultsRead { target, scope } => {
                native_result(self.route(&target)?.attachment.defaults_read(scope).await)
            }
            Method::DefaultSave {
                target,
                scope,
                expected_revision,
                setting,
            } => native_result(
                self.route(&target)?
                    .attachment
                    .defaults_save(scope, expected_revision, setting)
                    .await,
            ),
            Method::SessionUnload { target } => {
                self.route(&target)?;
                self.manager
                    .unload_incarnation(&target.conversation_id, target.runtime_incarnation)
                    .await
                    .map_err(manager_error)?;
                self.changed.notify_one();
                Ok(MethodResult::Unloaded {})
            }
            Method::ModelGet { target } => {
                native_result(self.route(&target)?.attachment.model_get())
            }
            Method::ModelCatalog { target } => {
                native_result(self.route(&target)?.attachment.model_catalog())
            }
            Method::ModelSet { target, config } => {
                native_result(self.route(&target)?.attachment.model_set(*config))
            }
            Method::ApprovalModeSet { target, mode } => {
                native_result(self.route(&target)?.attachment.approval_mode_set(mode))
            }
            Method::Capability { target } => {
                native_result(self.route(&target)?.attachment.capability())
            }
            Method::Transcript {
                target,
                before,
                limit,
            } => native_result(
                self.route(&target)?
                    .attachment
                    .transcript_page(before, limit),
            ),
            Method::Goal { target, control } => {
                native_result(self.route(&target)?.attachment.goal_control(control))
            }
            Method::BackgroundStatus {
                target,
                execution_id,
            } => native_result(
                self.route(&target)?
                    .attachment
                    .background_status(&execution_id),
            ),
            Method::BackgroundCancel {
                target,
                execution_id,
            } => native_result(
                self.route(&target)?
                    .attachment
                    .background_cancel(&execution_id),
            ),
            Method::SubagentStatus {
                target,
                subagent_id,
            } => native_result(
                self.route(&target)?
                    .attachment
                    .subagent_status(&subagent_id),
            ),
            Method::SubagentCancel {
                target,
                subagent_id,
            } => native_result(
                self.route(&target)?
                    .attachment
                    .subagent_cancel(&subagent_id),
            ),
            Method::SubagentDispose {
                target,
                subagent_id,
            } => native_result(
                self.route(&target)?
                    .attachment
                    .subagent_workspace_dispose(&subagent_id)
                    .await,
            ),
            Method::CompactContext { target } => {
                native_result(self.route(&target)?.attachment.compact_context().await)
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
            Method::SessionDelete {
                session_id,
                expected_target_revision,
            } => self
                .sessions
                .delete_session(&session_id, &expected_target_revision)
                .await
                .map(deletion)
                .map_err(session_error),
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
                let runtime = self
                    .manager
                    .load(&session_id, node_id.as_ref())
                    .await
                    .map_err(manager_error)?;
                let client = runtime.client();
                if self.routes.lock().expect("routes mutex").len() >= MAX_ATTACHMENTS {
                    return Err(domain(ErrorData::InvalidState));
                }
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
                let mut routes = self.routes.lock().expect("routes mutex");
                if routes.len() >= MAX_ATTACHMENTS {
                    return Err(domain(ErrorData::InvalidState));
                }
                routes.insert(
                    session_id,
                    Arc::new(Route {
                        target: target.clone(),
                        client,
                        attachment,
                    }),
                );
                self.changed.notify_one();
                Ok(MethodResult::Attached {
                    target,
                    snapshot: Box::new(snapshot),
                    cursor,
                })
            }
            Method::SessionDetach { target } => {
                let route = self.route(&target)?;
                let mut routes = self.routes.lock().expect("routes mutex");
                if routes
                    .get(&target.session_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &route))
                {
                    routes.remove(&target.session_id);
                    route.attachment.detach();
                }
                self.changed.notify_one();
                Ok(MethodResult::Detached {})
            }
            Method::SessionSnapshot { target } => {
                native_result(self.route(&target)?.attachment.snapshot())
            }
            Method::SessionSubscribe {
                target,
                after_cursor,
            } => {
                self.route(&target)?
                    .attachment
                    .subscribe_events(after_cursor)
                    .map_err(client_error)?;
                self.changed.notify_one();
                Ok(MethodResult::Subscribed { after_cursor })
            }
            Method::TurnStart { target, content } | Method::TurnSteer { target, content } => {
                native_result(self.route(&target)?.attachment.submit_inbound(content))
            }
            Method::TurnCancel { target } => {
                native_result(self.route(&target)?.attachment.cancel_current_attempt())
            }
            Method::InteractionRespond {
                target,
                interaction,
                response,
            } => native_result(
                self.route(&target)?
                    .attachment
                    .respond_interaction(&interaction, response)
                    .await,
            ),
            Method::InteractionCancel {
                target,
                interaction,
            } => native_result(
                self.route(&target)?
                    .attachment
                    .cancel_interaction(&interaction)
                    .await,
            ),
            Method::ResourcesReload { target } => {
                native_result(self.route(&target)?.attachment.reload_resources().await)
            }
        }
    }

    fn route(&self, target: &AttachmentTarget) -> Result<Arc<Route>, RpcError> {
        let route = self
            .routes
            .lock()
            .expect("routes mutex")
            .get(&target.session_id)
            .cloned()
            .ok_or_else(|| domain(ErrorData::StaleAttachment))?;
        if route.target != *target {
            return Err(domain(ErrorData::StaleAttachment));
        }
        route.client.validate().map_err(manager_error)?;
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
                    let mut routes = self.routes.lock().expect("routes mutex");
                    if routes
                        .get(&target.session_id)
                        .is_some_and(|current| Arc::ptr_eq(current, &route))
                    {
                        routes.remove(&target.session_id);
                    }
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
        for route in self.routes.get_mut().expect("routes mutex").values() {
            route.attachment.detach();
        }
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
fn failure(id: Option<RequestId>, error: RpcError) -> Response<MethodResult> {
    Response::Failure(Failure {
        jsonrpc: JsonRpcVersion::V2,
        id,
        error,
    })
}
