//! Native draft requests linearize at synchronous connection admission. The
//! caller's future never owns discovery, durable creation or the submission clock.
use super::{
    AppServerConnection, ErrorData, JsonRpcVersion, Method, MethodResult, Prepared, Reply, Request,
    Response, Success, catalog, domain, failure, host_error,
};
use crate::app_server::{RequestLane, rpc_error::session_error};
impl AppServerConnection {
    pub(super) fn prepare_initialization_retry(
        &self,
        request: &Request,
        association: &super::super::admission::Association,
    ) -> Option<Prepared> {
        let Method::SessionRetryInitialization {
            session_id,
            node_id,
        } = &request.call
        else {
            return None;
        };
        if let Err(error) = self.admitted() {
            return Some(Prepared::ready(Reply::plain(failure(
                Some(request.id.clone()),
                error,
            ))));
        }
        let receipt = crate::local_runtime::composition::SubmissionIntent::now();
        let manager = self.host.manager().clone();
        let session_id = session_id.clone();
        let node_id = node_id.clone();
        let admitted = self.host.admit_request(RequestLane::Control, |owner| {
            let (send, receive) = tokio::sync::oneshot::channel();
            let association = association.clone();
            tokio::spawn(async move {
                let _owner = owner;
                let _association = association;
                let result = manager
                    .retry_failed_initialization(&session_id, node_id.as_ref(), receipt)
                    .await
                    .map_err(super::manager_error);
                let _ = send.send(result);
            });
            receive
        });
        let receiver = match admitted {
            Ok(receiver) => receiver,
            Err(error) => {
                return Some(Prepared::ready(Reply::plain(failure(
                    Some(request.id.clone()),
                    host_error(error),
                ))));
            }
        };
        let id = request.id.clone();
        let association = association.clone();
        Some(Prepared::Pending(Box::pin(async move {
            let result = receiver
                .await
                .unwrap_or_else(|_| Err(domain(ErrorData::OperationFailed)));
            let response = match result {
                Ok(()) => Response::Success(Box::new(Success {
                    jsonrpc: JsonRpcVersion::V2,
                    id,
                    result: MethodResult::SubmissionIntentAccepted {},
                })),
                Err(error) => failure(Some(id), error),
            };
            Some(Reply {
                response,
                publication: None,
                association: Some(association),
                may_have_committed: true,
                registration: None,
            })
        })))
    }

    pub(super) fn prepare_mcp_draft(
        &self,
        request: &Request,
        association: &super::super::admission::Association,
    ) -> Option<Prepared> {
        if !matches!(
            &request.call,
            Method::McpDraftPrepare { .. } | Method::McpDraftCancel { .. }
        ) {
            return None;
        }
        let id = &request.id;
        if let Err(error) = self.admitted() {
            return Some(Prepared::ready(Reply::plain(failure(
                Some(id.clone()),
                error,
            ))));
        }
        let result = self
            .host
            .admit_request(
                RequestLane::for_method(&request.call),
                |_owner| match &request.call {
                    Method::McpDraftPrepare { target, cwd } => self
                        .mcp_draft
                        .select(target.clone(), cwd.clone())
                        .map_err(|_| domain(ErrorData::InvalidParams)),
                    Method::McpDraftCancel { target } if target.validate() => {
                        self.mcp_draft.cancel(target);
                        Ok(())
                    }
                    _ => Err(domain(ErrorData::InvalidParams)),
                },
            )
            .map_err(host_error)
            .and_then(std::convert::identity);
        let response = match result {
            Ok(()) => Response::Success(Box::new(Success {
                jsonrpc: JsonRpcVersion::V2,
                id: id.clone(),
                result: MethodResult::McpDraftAccepted {},
            })),
            Err(error) => failure(Some(id.clone()), error),
        };
        Some(Prepared::ready(Reply {
            response,
            publication: None,
            association: Some(association.clone()),
            may_have_committed: true,
            registration: None,
        }))
    }
    pub(super) fn prepare_draft_submission(
        &self,
        request: &Request,
        association: &super::super::admission::Association,
    ) -> Option<Prepared> {
        let Method::SessionCreate {
            settings,
            submission: Some(target),
        } = &request.call
        else {
            return None;
        };
        let id = &request.id;
        if let Err(error) = self.admitted() {
            return Some(Prepared::ready(Reply::plain(failure(
                Some(id.clone()),
                error,
            ))));
        }
        if !target.validate() {
            return Some(Prepared::ready(Reply::plain(failure(
                Some(id.clone()),
                domain(ErrorData::InvalidParams),
            ))));
        }
        let receipt = crate::local_runtime::composition::SubmissionIntent::now();
        let manager = self.host.manager().clone();
        let settings = settings.clone();
        let admitted = self.host.admit_request(RequestLane::Metadata, |owner| {
            let claim = self.mcp_draft.claim(target, receipt);
            let sessions = self.sessions.clone();
            let (send, receive) = tokio::sync::oneshot::channel();
            let association = association.clone();
            tokio::spawn(async move {
                let _owner = owner;
                let _association = association;
                let result = match manager.create_session_submitted(settings, claim).await {
                    Ok(result) => catalog::transition(result, &sessions).await,
                    Err(error) => Err(session_error(error)),
                };
                let _ = send.send(result);
            });
            receive
        });
        let receiver = match admitted {
            Ok(receiver) => receiver,
            Err(error) => {
                return Some(Prepared::ready(Reply::plain(failure(
                    Some(id.clone()),
                    host_error(error),
                ))));
            }
        };
        let id = id.clone();
        let association = association.clone();
        Some(Prepared::Pending(Box::pin(async move {
            let result = receiver
                .await
                .unwrap_or_else(|_| Err(domain(ErrorData::OperationFailed)));
            let response = match result {
                Ok(result) => Response::Success(Box::new(Success {
                    jsonrpc: JsonRpcVersion::V2,
                    id,
                    result,
                })),
                Err(error) => failure(Some(id), error),
            };
            Some(Reply {
                response,
                publication: None,
                association: Some(association),
                may_have_committed: true,
                registration: None,
            })
        })))
    }
}
