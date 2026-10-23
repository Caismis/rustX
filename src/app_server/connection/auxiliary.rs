//! Every external auxiliary carrier uses native read/control authority and the
//! existing physical publication fence, including submission receipts.
use super::{
    AppServerConnection, Arc, ErrorData, Method, MethodResult, RequestId, RpcError, client_error,
    domain, host_error, manager_error,
};
impl AppServerConnection {
    #[allow(clippy::too_many_lines)] // One exact association/publication owner surrounds all named dispatch branches.
    pub(super) async fn auxiliary_request(
        &self,
        id: &RequestId,
        method: Method,
        association: super::super::admission::Association,
    ) -> (
        Result<MethodResult, RpcError>,
        Option<super::super::delivery_access::Publication>,
    ) {
        if let Err(error) = self.admitted() {
            return (Err(error), None);
        }
        let (Method::AuxiliaryClassify { target, .. }
        | Method::AuxiliaryBtw { target, .. }
        | Method::AuxiliaryStatus { target, .. }
        | Method::AuxiliaryResult { target, .. }
        | Method::AuxiliaryCancel { target, .. }
        | Method::AuxiliaryDiscard { target, .. }
        | Method::AuxiliarySubmissionLookup { target, .. }) = &method
        else {
            return (Err(domain(ErrorData::InvalidParams)), None);
        };
        let Some(access) = self
            .delivery_access
            .clone()
            .filter(|token| !token.is_cancelled())
        else {
            return (Err(super::super::delivery_access::unauthorized()), None);
        };
        let route = match self.route(target) {
            Ok(route) => route,
            Err(error) => return (Err(error), None),
        };
        let Some(publication) = self.deliveries.register(
            id,
            &access,
            route.clone(),
            self.host.revocations().clone(),
            #[cfg(test)]
            self.host.file_read_probe(),
        ) else {
            return (Err(domain(ErrorData::InvalidParams)), None);
        };
        let lane = super::super::RequestLane::for_method(&method);
        let permit = association.submission_permit();
        let client = route.client.clone();
        let token = publication.token();
        let result = {
            let routes = self.routes.lock().expect("routes mutex");
            if routes.closed
                || !routes
                    .active
                    .get(&route.target.session_id)
                    .is_some_and(|active| Arc::ptr_eq(active, &route))
            {
                return (Err(domain(ErrorData::StaleAttachment)), Some(publication));
            }
            self.host
                .admit_request(lane, |request_owner| {
                    client.start_operation(move |_| async move {
                        let blocking = !matches!(
                            &method,
                            Method::AuxiliaryCancel { .. } | Method::AuxiliaryDiscard { .. }
                        );
                        let work = move || {
                            let _request = request_owner;
                            let _association = association;
                            super::super::delivery_access::check_rpc(&token)?;
                            let control = matches!(
                                method,
                                Method::AuxiliaryClassify { .. }
                                    | Method::AuxiliaryBtw { .. }
                                    | Method::AuxiliaryCancel { .. }
                                    | Method::AuxiliaryDiscard { .. }
                            );
                            let authority = if control {
                                route.attachment.operation_authority()
                            } else {
                                route.attachment.read_authority()
                            }
                            .map_err(client_error)?;
                            let mut copy = None;
                            let mut admission = None;
                            let result = match method {
                                Method::AuxiliaryClassify { submission, .. } => {
                                    admission = Some(
                                        route
                                            .attachment
                                            .classify_admitted(
                                                submission,
                                                permit.as_ref().ok_or_else(|| {
                                                    domain(ErrorData::InvalidParams)
                                                })?,
                                            )
                                            .map_err(client_error)?,
                                    );
                                    None
                                }
                                Method::AuxiliaryBtw { submission, .. } => {
                                    admission = Some(
                                        route
                                            .attachment
                                            .btw_admitted(
                                                submission,
                                                permit.as_ref().ok_or_else(|| {
                                                    domain(ErrorData::InvalidParams)
                                                })?,
                                            )
                                            .map_err(client_error)?,
                                    );
                                    None
                                }
                                Method::AuxiliaryStatus { call_id, .. } => {
                                    Some(MethodResult::AuxiliaryStatus {
                                        status: authority
                                            .auxiliary_status(&call_id)
                                            .map_err(client_error)?,
                                    })
                                }
                                Method::AuxiliaryResult { call_id, .. } => {
                                    copy = authority
                                        .auxiliary_result(&call_id)
                                        .map_err(client_error)?;
                                    Some(MethodResult::AuxiliaryResult {
                                        result: copy.as_mut().and_then(|copy| copy.result.take()),
                                    })
                                }
                                Method::AuxiliarySubmissionLookup {
                                    correlation, scope, ..
                                } => Some(MethodResult::AuxiliarySubmissionLookup {
                                    lookup: authority
                                        .auxiliary_lookup(&correlation, scope.as_ref())
                                        .map_err(client_error)?,
                                }),
                                Method::AuxiliaryCancel { call_id, .. } => {
                                    Some(MethodResult::AuxiliaryCancelled {
                                        accepted: route
                                            .attachment
                                            .auxiliary_cancel(&call_id)
                                            .map_err(client_error)?,
                                    })
                                }
                                Method::AuxiliaryDiscard { call_id, .. } => {
                                    Some(MethodResult::AuxiliaryDiscarded {
                                        discarded: route
                                            .attachment
                                            .auxiliary_discard(&call_id)
                                            .map_err(client_error)?,
                                    })
                                }
                                _ => return Err(domain(ErrorData::InvalidParams)),
                            };
                            Ok((result, copy, admission))
                        };
                        if blocking {
                            tokio::task::spawn_blocking(work)
                                .await
                                .map_err(|_| domain(ErrorData::OperationFailed))?
                        } else {
                            work()
                        }
                    })
                })
                .map_err(host_error)
                .and_then(|result| result.map_err(manager_error))
        };
        let result = match result {
            Ok(receiver) => receiver
                .await
                .map_err(|_| domain(ErrorData::OperationFailed))
                .and_then(std::convert::identity),
            Err(error) => Err(error),
        };
        match result {
            Ok((result, copy, admission)) => {
                publication.retain_auxiliary_copy(copy);
                let result = if let Some(admission) = admission {
                    admission
                        .confirmed()
                        .await
                        .map(|receipt| MethodResult::AuxiliarySubmitted { receipt })
                        .map_err(|error| {
                            client_error(crate::runtime_client::host::auxiliary::error(&error))
                        })
                } else {
                    result.ok_or_else(|| domain(ErrorData::OperationFailed))
                };
                (result, Some(publication))
            }
            Err(error) => (Err(error), Some(publication)),
        }
    }
}
