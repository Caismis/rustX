//! The App Server's one mapping from native failures to JSON-RPC errors.
//!
//! Every request family and the transports map owner failures here, so a
//! native refusal keeps one typed wire classification wherever it surfaces.
use super::host::HostAdmissionError;
use super::protocol::{ErrorData, Failure, JsonRpcVersion, RequestId, Response, RpcError};
use crate::local_runtime::session_runtime_manager::RuntimeManagerError;
use crate::runtime_client::types::RuntimeClientError;

pub(super) fn client_error(error: RuntimeClientError) -> RpcError {
    domain(match error {
        RuntimeClientError::Auxiliary { reason } => ErrorData::Auxiliary { reason },
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
        // The App Server selects models through `session/setModel`; a Runtime
        // Client model refusal here is a turn or compaction refused because
        // the published catalog no longer admits the configured selection.
        RuntimeClientError::InvalidModelConfiguration { message } => ErrorData::ModelUnavailable {
            diagnostic: message,
        },
        RuntimeClientError::InteractionNotPending { interaction } => {
            ErrorData::InteractionNotPending { interaction }
        }
        RuntimeClientError::InteractionAuditFailed { interaction } => {
            ErrorData::InteractionAuditFailed { interaction }
        }
        RuntimeClientError::AttachmentInUse { .. } => ErrorData::ControllerInUse,
        RuntimeClientError::NotAttached => ErrorData::StaleAttachment,
        RuntimeClientError::ResyncRequired { .. } => ErrorData::ResyncRequired,
        RuntimeClientError::ObservationUnavailable { message } => {
            let mut end = message.len().min(512);
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            ErrorData::ObservationUnavailable {
                diagnostic: message[..end].into(),
            }
        }
        RuntimeClientError::ObservationCapacityExceeded => ErrorData::ObservationCapacityExceeded,
        RuntimeClientError::ContentSourceUnavailable => ErrorData::ContentSourceUnavailable,
        RuntimeClientError::InvalidRange => ErrorData::InvalidRange,
        RuntimeClientError::ReadCutExpired => ErrorData::ReadCutExpired,
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
        RuntimeManagerError::ScheduleStore(error) => {
            rpc_error(-32000, &error.to_string(), Some(ErrorData::OperationFailed))
        }
        RuntimeManagerError::TransitionFailed(detail) => {
            rpc_error(-32000, &detail, Some(ErrorData::OperationFailed))
        }
        RuntimeManagerError::SessionAlreadyResident { .. } => domain(ErrorData::OperationFailed),
    }
}
pub(super) fn model_selection_error(
    error: crate::local_runtime::session_runtime_manager::ModelSelectionError,
) -> RpcError {
    use crate::local_runtime::session_runtime_manager::ModelSelectionError;
    use crate::runtime::conversation_runtime::ModelUpdateError;
    match error {
        ModelSelectionError::Model(ModelUpdateError::SessionOwned) => {
            domain(ErrorData::OperationFailed)
        }
        ModelSelectionError::Model(ModelUpdateError::Conflict)
        | ModelSelectionError::Persistence(
            crate::local_runtime::session::SessionError::StaleSettings { .. },
        ) => domain(ErrorData::ConfigurationAdoption {
            rejection: crate::runtime::adoption::AdoptionError::Conflict,
        }),
        ModelSelectionError::Model(ModelUpdateError::Inactive) => domain(ErrorData::StaleRuntime),
        ModelSelectionError::Model(ModelUpdateError::InvalidConfiguration(diagnostic)) => {
            domain(ErrorData::ModelUnavailable { diagnostic })
        }
        ModelSelectionError::Model(ModelUpdateError::DurabilityFailed { message }) => {
            rpc_error(-32000, &message, Some(ErrorData::OperationFailed))
        }
        ModelSelectionError::Persistence(error) => session_error(error),
    }
}

pub(super) fn session_error(error: crate::local_runtime::session::SessionError) -> RpcError {
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
        ErrorData::Auxiliary { reason } => match reason {
            crate::runtime::auxiliary::submission::AuxiliaryFailure::InputTooLarge => {
                "Auxiliary input exceeds the available context or preparation budget"
            }
            crate::runtime::auxiliary::submission::AuxiliaryFailure::Busy => {
                "Auxiliary capacity is full"
            }
            crate::runtime::auxiliary::submission::AuxiliaryFailure::Conflict => {
                "Correlation names a different original command"
            }
            crate::runtime::auxiliary::submission::AuxiliaryFailure::ScopeClosed => {
                "Original auxiliary submission scope is closed"
            }
            crate::runtime::auxiliary::submission::AuxiliaryFailure::Unavailable => {
                "Auxiliary evidence or input is unavailable"
            }
        },
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
                "Delivered Session file exceeds the requested read budget"
            }
            crate::tools::session_files::SessionFileReadFailure::Capacity => {
                "Session file read capacity reached"
            }
            crate::tools::session_files::SessionFileReadFailure::ReadFailed => {
                "Session file read failed"
            }
        },
        ErrorData::ObservationCancelled => "Child conversation observation was cancelled",
        ErrorData::DeliveryCancelled => "Delivery request was cancelled before publication",
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
        ErrorData::ModelUnavailable { .. } => {
            "The Session's configured Model is not in its published catalog; select one it provides"
        }
        _ => "Operation rejected",
    };
    rpc_error(-32000, message, Some(data))
}
pub(super) fn rpc_error(code: i32, message: &str, data: Option<ErrorData>) -> RpcError {
    let mut end = message.len().min(512);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    RpcError {
        code,
        message: message[..end].into(),
        data,
    }
}
pub(super) fn failure(id: Option<RequestId>, error: RpcError) -> Response {
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

pub(super) fn source_error(
    error: crate::local_runtime::configuration::settings::SettingsError,
) -> RpcError {
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
        SettingsError::InvalidWorkflow(diagnostic) => ErrorData::WorkflowInvalid { diagnostic },
        SettingsError::Io => ErrorData::OperationFailed,
        SettingsError::Committed => ErrorData::CommittedDurabilityUncertain,
    })
}

pub(super) fn source_settings_error(
    error: crate::local_runtime::session_runtime_manager::SourceSettingsError,
) -> RpcError {
    match error {
        crate::local_runtime::session_runtime_manager::SourceSettingsError::Source(error) => {
            source_error(error)
        }
    }
}

pub(super) fn source_read_error(error: &str) -> RpcError {
    domain(if error == "source read cut expired" {
        ErrorData::ReadCutExpired
    } else if error == "configuration source unavailable" {
        ErrorData::ObservationUnavailable {
            diagnostic: "configuration source unavailable".into(),
        }
    } else {
        ErrorData::InvalidParams
    })
}

#[cfg(test)]
mod tests {
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
}
