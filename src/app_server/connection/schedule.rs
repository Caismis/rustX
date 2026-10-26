//! Exact-node Scheduler controls, independent of runtime attachment or residency.
use super::AppServerConnection;
use crate::app_server::host::ServerOperation;
use crate::app_server::protocol::{ErrorData, Method, MethodResult, RpcError};
use crate::app_server::rpc_error::{domain, manager_error};

impl AppServerConnection {
    #[allow(clippy::too_many_lines)] // Exhaustive routing retains each operation owner through its native result.
    pub(super) async fn schedule(
        &self,
        method: Method,
        request_owner: ServerOperation,
    ) -> Result<MethodResult, RpcError> {
        match method {
            Method::WorkflowScheduledRecovery {
                session_id,
                node_id,
                start_id,
                after_effect_ordinal,
                limit,
            } => {
                let recovery = self
                    .host
                    .manager()
                    .scheduled_workflow_recovery(
                        &session_id,
                        &node_id,
                        start_id,
                        after_effect_ordinal,
                        limit,
                    )
                    .await
                    .map_err(manager_error)?;
                Ok(MethodResult::ScheduledWorkflowRecovery { recovery })
            }

            Method::ScheduleList {
                session_id,
                node_id,
                after_ordinal,
                limit,
            } => {
                let schedules = self
                    .host
                    .manager()
                    .schedule_list(&session_id, &node_id, after_ordinal, limit)
                    .await
                    .map_err(manager_error)?;
                let full = usize::try_from(limit).is_ok_and(|limit| schedules.len() == limit);
                let next_ordinal = schedules
                    .last()
                    .filter(|_| full)
                    .map(|schedule| schedule.id.ordinal);
                let schedules = schedules
                    .into_iter()
                    .map(crate::scheduler::ScheduleHeader::from)
                    .collect();
                Ok(MethodResult::Schedules {
                    schedules,
                    next_ordinal,
                })
            }
            Method::ScheduleGet {
                session_id,
                node_id,
                id,
            } => {
                let schedule = self
                    .host
                    .manager()
                    .schedule_get(&session_id, &node_id, id)
                    .await
                    .map_err(manager_error)?;
                Ok(MethodResult::Schedule { schedule })
            }
            method @ (Method::ScheduleCommand { .. }
            | Method::ScheduleRecoverStorage { .. }
            | Method::WorkflowScheduledCancel { .. }
            | Method::WorkflowScheduledReconcile { .. }) => {
                let manager = self.host.manager().clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                // Retain host admission through native settlement even if this
                // connection or its response waiter disappears after acceptance.
                tokio::spawn(async move {
                    let _request = request_owner;
                    let result = match method {
                        Method::WorkflowScheduledCancel {
                            session_id,
                            node_id,
                            start_id,
                        } => manager
                            .cancel_scheduled_workflow(&session_id, &node_id, start_id)
                            .await
                            .map(|recovery| MethodResult::ScheduledWorkflowRecovery { recovery })
                            .map_err(manager_error),
                        Method::WorkflowScheduledReconcile {
                            session_id,
                            node_id,
                            start_id,
                            after_effect_ordinal,
                            limit,
                        } => manager
                            .reconcile_scheduled_workflow(
                                &session_id,
                                &node_id,
                                start_id,
                                after_effect_ordinal,
                                limit,
                            )
                            .await
                            .map(|recovery| MethodResult::ScheduledWorkflowRecovery { recovery })
                            .map_err(manager_error),

                        Method::ScheduleCommand {
                            session_id,
                            node_id,
                            command,
                        } => manager
                            .schedule_command(&session_id, &node_id, command)
                            .await
                            .map(|result| MethodResult::ScheduleCommand { result })
                            .map_err(manager_error),
                        Method::ScheduleRecoverStorage {
                            session_id,
                            node_id,
                        } => manager
                            .recover_schedule_storage(&session_id, &node_id)
                            .await
                            .map(|summary| MethodResult::ScheduleStorageRecovered {
                                generation: summary.generation,
                            })
                            .map_err(manager_error),
                        _ => unreachable!("closed Scheduler mutation family"),
                    };
                    let _ = sender.send(result);
                });
                receiver
                    .await
                    .map_err(|_| domain(ErrorData::OperationFailed))?
            }
            _ => unreachable!("Scheduler request family"),
        }
    }
}
