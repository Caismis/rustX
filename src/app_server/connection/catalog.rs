//! Session catalog requests: listing, reads, lineage commits, node
//! selection, deletion and export. The catalog owner commits; this family
//! only maps its outcomes onto the wire.
use super::{AppServerConnection, release_route};
use crate::app_server::host::ServerOperation;
use crate::app_server::protocol::{ErrorData, Method, MethodResult, RpcError};
use crate::app_server::rpc_error::{domain, manager_error, rpc_error, session_error};
use crate::local_runtime::session_controller::SessionController;

impl AppServerConnection {
    /// Serve one admitted catalog request. Server-owned commits keep the
    /// request's host admission until they settle, even if the caller leaves.
    #[allow(clippy::too_many_lines)] // One arm per catalog method.
    pub(super) async fn catalog(
        &self,
        method: Method,
        request_owner: ServerOperation,
    ) -> Result<MethodResult, RpcError> {
        match method {
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
                        // Node selection is committed. The successor attaches
                        // independently; observation admission must not turn a
                        // confirmed catalog mutation into an ambiguous failure.
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
            Method::SessionExportPrepare { session_id } => {
                let download =
                    crate::app_server::archive_download::prepare(&self.host, session_id.clone())
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
            Method::SessionRead { session_id } => Ok(MethodResult::Session {
                session: self
                    .sessions
                    .read_session(&session_id)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionCreate { settings } => {
                let result = self
                    .host
                    .manager()
                    .create_session(settings)
                    .await
                    .map_err(session_error)?;
                transition(result, &self.sessions).await
            }
            Method::SessionName { session_id, name } => Ok(MethodResult::Session {
                session: self
                    .sessions
                    .rename_session(&session_id, &name)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionNode {
                session_id,
                node_id,
            } => Ok(MethodResult::Node {
                node: self
                    .sessions
                    .observe_node(&session_id, &node_id)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionNodeValueRead {
                reference,
                offset,
                max_bytes,
            } => Ok(MethodResult::NodeValueChunk {
                chunk: self
                    .sessions
                    .node_value_read(&reference, offset, max_bytes)
                    .await
                    .map_err(session_error)?,
            }),
            Method::SessionTree {
                session_id,
                offset,
                limit,
            } => {
                let (nodes, next_offset) = self
                    .sessions
                    .observe_tree(&session_id, offset, limit)
                    .await
                    .map_err(session_error)?;
                Ok(MethodResult::Tree { nodes, next_offset })
            }
            Method::SessionForkProjection {
                session_id,
                node_id,
                ledger,
                activation,
            } => {
                let result = self
                    .sessions
                    .fork_projection(&session_id, node_id.as_ref(), ledger, activation)
                    .await
                    .map_err(session_error)?;
                transition(result, &self.sessions).await
            }
            Method::SessionFork {
                session_id,
                node_id,
                surface_revision,
                boundary,
                side,
            } => {
                let boundary = match boundary {
                    Some(locator) => Some(
                        self.sessions
                            .resolve_boundary(
                                &session_id,
                                node_id.as_ref(),
                                surface_revision,
                                &locator,
                                side,
                            )
                            .await
                            .map_err(session_error)?,
                    ),
                    None => None,
                };
                let result = self
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
                    .map_err(session_error)?;
                transition(result, &self.sessions).await
            }
            Method::SessionBranch {
                session_id,
                node_id,
                surface_revision,
                boundary,
                side,
            } => {
                let boundary = self
                    .sessions
                    .resolve_boundary(
                        &session_id,
                        Some(&node_id),
                        surface_revision,
                        &boundary,
                        side,
                    )
                    .await
                    .map_err(session_error)?;
                let result = self
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
                    .map_err(session_error)?;
                transition(result, &self.sessions).await
            }
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
            // `Family::of` routes only catalog requests here.
            _ => Err(domain(ErrorData::OperationFailed)),
        }
    }
}

async fn transition(
    result: crate::local_runtime::session_controller::SessionTransitionResult,
    sessions: &SessionController,
) -> Result<MethodResult, RpcError> {
    let node = sessions
        .observe_node(&result.session.id, &result.session.active_node)
        .await
        .map_err(|_| domain(ErrorData::OutcomeUnknown))?;
    let editor = crate::app_server::editor_read::locator(result.session.id.clone(), &node);
    Ok(MethodResult::SessionTransition {
        session: result.session,
        editor,
        durability_diagnostic: result
            .durability_diagnostic
            .map(|_| "Committed; durable barrier not confirmed".into()),
    })
}
fn deletion(result: crate::local_runtime::session::deletion::SessionDeleteResult) -> MethodResult {
    MethodResult::Deletion {
        result: crate::app_server::session_deletion::project(result),
    }
}
