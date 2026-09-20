//! Archive v1 boundaries for native types with mixed historical/private ownership.
use crate::model::snapshot::RequestSnapshot;
use serde::Serialize;
use serde_json::{Value, json};

/// Explicit schema: new durable fields cannot silently become archive fields.
#[derive(Serialize)]
struct ArchiveRequestSnapshotV1<'a> {
    upload_projection: &'a crate::model::uploads::UploadProjection,
    request_id: &'a crate::runtime::identity::RequestId,
    identity: &'a crate::model::snapshot::RequestIdentity,
    provisional_message_id: &'a crate::runtime::identity::MessageId,
    surface_revision: &'a crate::conversation::SurfaceRevision,
    effective_system_prompt: &'a String,
    system_sections: &'a Vec<crate::context::assembly::AcceptedSystemSection>,
    context_window_tokens: &'a u64,
    reasoning_profile: &'a Option<crate::model::catalog::ReasoningProfileId>,
    reasoning_enabled: &'a bool,
    tool_definitions: &'a Vec<crate::tools::types::ModelToolDefinition>,
    capability_revision: &'a crate::runtime::identity::CapabilityRevision,
    context_generation: &'a crate::context::assembly::ContextGeneration,
    request_context_ids: &'a Vec<crate::runtime::identity::MessageId>,
    unresolved_output_carryover_source: &'a Option<crate::runtime::identity::PublicationStreamId>,
    unresolved_output_carryover: &'a Option<crate::model::input::RenderedUnresolvedOutputCarryover>,
    unresolved_output_carryover_anchor: &'a Option<crate::model::input::RequestOnlyInsertionAnchor>,
    contributions: &'a [crate::model::snapshot::ContributionStart],
    invocation: ArchiveInvocationV1<'a>,
}
#[derive(Serialize)]
struct ArchiveInvocationV1<'a> {
    model: &'a str,
    protocol: crate::model::types::ModelProtocol,
    max_output_tokens: u32,
    request_options: std::collections::BTreeMap<&'a str, &'a Value>,
    omitted_option_count: usize,
}
pub(super) fn request(snapshot: &RequestSnapshot) -> Value {
    let invocation = &snapshot.invocation;
    let request_options: std::collections::BTreeMap<_, _> = invocation
        .request_params
        .iter()
        .filter(|(name, _)| {
            crate::model::inspection::REQUEST_OPTION_ALLOWLIST.contains(&name.as_str())
        })
        .map(|(name, value)| (name.as_str(), value))
        .collect();
    json!(ArchiveRequestSnapshotV1 {
        upload_projection: &snapshot.upload_projection,
        request_id: &snapshot.request_id,
        identity: &snapshot.identity,
        provisional_message_id: &snapshot.provisional_message_id,
        surface_revision: &snapshot.surface_revision,
        effective_system_prompt: &snapshot.effective_system_prompt,
        system_sections: &snapshot.system_sections,
        context_window_tokens: &snapshot.context_window_tokens,
        reasoning_profile: &snapshot.reasoning_profile,
        reasoning_enabled: &snapshot.reasoning_enabled,
        tool_definitions: &snapshot.tool_definitions,
        capability_revision: &snapshot.capability_revision,
        context_generation: &snapshot.context_generation,
        request_context_ids: &snapshot.request_context_ids,
        unresolved_output_carryover_source: &snapshot.unresolved_output_carryover_source,
        unresolved_output_carryover: &snapshot.unresolved_output_carryover,
        unresolved_output_carryover_anchor: &snapshot.unresolved_output_carryover_anchor,
        contributions: &snapshot.contributions,
        invocation: ArchiveInvocationV1 {
            model: &invocation.model,
            protocol: invocation.protocol,
            max_output_tokens: invocation.max_output_tokens,
            omitted_option_count: invocation.request_params.len() - request_options.len(),
            request_options,
        },
    })
}

pub(super) fn message(message: &crate::message::types::MessageBlock) -> Value {
    use crate::message::types::{AssistantContentBlock, MessageBlock};
    match message {
        MessageBlock::User(_) | MessageBlock::Tool(_) => json!(message),
        MessageBlock::Assistant(assistant) => json!({
            "role": "assistant", "id": assistant.id,
            "content": assistant.content.iter().map(|block| match block {
                AssistantContentBlock::Reasoning(reasoning) => json!({"type":"reasoning", "text":reasoning.text}),
                AssistantContentBlock::Text(_) | AssistantContentBlock::Refusal(_) |
                AssistantContentBlock::ToolCall(_) | AssistantContentBlock::Image(_) => json!(block),
            }).collect::<Vec<_>>()
        }),
    }
}

/// Journal envelopes keep their native coordinates; mixed diagnostic events
/// have explicit v1 payloads. The exhaustive match requires review of new facts.
pub(super) fn journal(envelope: &crate::events::types::RuntimeEventEnvelope) -> Value {
    json!({"schema_version": envelope.schema_version, "event_id": envelope.event_id,
        "sequence": envelope.sequence, "conversation_id": envelope.conversation_id,
        "attempt_id": envelope.attempt_id, "turn_id": envelope.turn_id,
        "timestamp": envelope.timestamp, "event": event(&envelope.event)})
}
/// Archive Journal status is outcome evidence, not executor diagnostic prose.
/// Exhaustive by design: a new status must receive an explicit v1 decision.
fn tool_execution_status(status: &crate::tools::types::ToolExecutionStatus) -> Value {
    use crate::tools::types::ToolExecutionStatus;
    match status {
        ToolExecutionStatus::Success => json!({"type":"success"}),
        ToolExecutionStatus::Failed { .. } => {
            json!({"type":"failed", "diagnostic_unavailable":"executor diagnostic excluded"})
        }
        ToolExecutionStatus::Denied { .. } => {
            json!({"type":"denied", "diagnostic_unavailable":"executor diagnostic excluded"})
        }
        ToolExecutionStatus::Cancelled { reason, phase } => {
            json!({"type":"cancelled", "reason":reason, "phase":phase})
        }
        ToolExecutionStatus::TimedOut => json!({"type":"timed_out"}),
        ToolExecutionStatus::OutcomeUnknown { .. } => {
            json!({"type":"outcome_unknown", "diagnostic_unavailable":"executor diagnostic excluded"})
        }
    }
}

/// Continuation state and locators are history; storage diagnostics are not.
/// Exhaustive by design so new continuation variants require a v1 decision.
fn managed_output_continuation(
    continuation: &crate::tools::types::ManagedOutputContinuation,
) -> Value {
    use crate::tools::types::ManagedOutputContinuation;
    match continuation {
        ManagedOutputContinuation::Complete { locator } => {
            json!({"type":"complete", "locator":locator})
        }
        ManagedOutputContinuation::Partial { locator, .. } => {
            json!({"type":"partial", "locator":locator,
                "diagnostic_unavailable":"output-storage diagnostic excluded"})
        }
        ManagedOutputContinuation::Unavailable { .. } => {
            json!({"type":"unavailable",
                "diagnostic_unavailable":"output-storage diagnostic excluded"})
        }
    }
}

/// Tool-owned content and structured execution facts remain intact. Journal
/// status and output-storage diagnostics cross explicit restricted projections;
/// canonical Tool messages retain the original result exactly via `message()`.
fn tool_execution_result(result: &crate::tools::types::ToolExecutionResult) -> Value {
    json!({
        "status":tool_execution_status(&result.status), "content":result.content,
        "duration_ms":result.duration_ms, "exit_code":result.exit_code,
        "artifacts":result.artifacts, "truncation":result.truncation,
        "workflow":result.workflow,
        "managed_output":result.managed_output.as_ref().map(managed_output_continuation),
    })
}

fn finish_reason(reason: &crate::model::ModelFinishReason) -> Value {
    use crate::model::ModelFinishReason;
    match reason {
        ModelFinishReason::Other { .. } => {
            json!({"type":"other","diagnostic_unavailable":"provider finish code excluded"})
        }
        ModelFinishReason::Stop
        | ModelFinishReason::ToolCalls
        | ModelFinishReason::Length
        | ModelFinishReason::ContentFilter
        | ModelFinishReason::Refusal => json!(reason),
    }
}
fn model_error(error: &crate::model::error::ModelError) -> Value {
    json!({"kind": error.kind, "retry_disposition":error.retry_disposition,
        "retry_after_ms":error.retry_after_ms, "context_overflow":error.context_overflow,
        "malformed_tool_proposal":error.malformed_tool_proposal,
        "timeout_phase":error.timeout_phase, "generation":error.generation,
        "diagnostic_unavailable":"provider diagnostic and code excluded"})
}
fn attempt_failure(failure: &crate::events::types::AttemptFailure) -> Value {
    use crate::events::types::AttemptFailure;
    match failure {
        AttemptFailure::Model { error } => json!({"type":"model", "error":model_error(error)}),
        AttemptFailure::Runtime { error } => {
            json!({"type":"runtime", "error":runtime_error(error)})
        }
    }
}
fn runtime_error(error: &crate::runtime::types::RuntimeError) -> Value {
    use crate::runtime::types::RuntimeError;
    if let RuntimeError::UnknownTool { name } = error {
        return json!({"type":"unknown_tool","name":name});
    }
    let kind = match error {
        RuntimeError::Internal { .. } => "internal",
        RuntimeError::InvalidState { .. } => "invalid_state",
        RuntimeError::Unsupported { .. } => "unsupported",
        RuntimeError::UnknownTool { .. } => "unknown_tool",
        RuntimeError::DurableStore { .. } => "durable_store",
        RuntimeError::ContractViolation { .. } => "contract_violation",
        RuntimeError::ContextPreparationFailed { .. } => "context_preparation_failed",
        RuntimeError::ContextCompactionFailed { .. } => "context_compaction_failed",
        RuntimeError::PreStepRejected { .. } => "pre_step_rejected",
        RuntimeError::PreStepPolicyFailed { .. } => "pre_step_policy_failed",
        RuntimeError::ToolResultObservationFailed { .. } => "tool_result_observation_failed",
        RuntimeError::RestartInterrupted { .. } => "restart_interrupted",
        RuntimeError::DeferredContextRejected { .. } => "deferred_context_rejected",
    };
    json!({"type":kind,"diagnostic_unavailable":"runtime diagnostic excluded"})
}
fn subagent_resource(resource: &crate::events::types::SubagentWorkspaceTerminalResource) -> Value {
    use crate::events::types::SubagentWorkspaceTerminalResource;
    match resource {
        SubagentWorkspaceTerminalResource::None
        | SubagentWorkspaceTerminalResource::Retained { .. } => json!(resource),
        SubagentWorkspaceTerminalResource::PreservedUnresolved { reason, .. } => {
            json!({"state":"preserved_unresolved", "reason":reason,"diagnostic_unavailable":"physical settlement diagnostic excluded"})
        }
    }
}
fn workspace(workspace: &crate::runtime::workspace::WorkspaceSettlement) -> Value {
    use crate::runtime::workspace::WorkspaceSettlementDisposition;
    let disposition = match &workspace.disposition {
        WorkspaceSettlementDisposition::Borrowed
        | WorkspaceSettlementDisposition::Shared
        | WorkspaceSettlementDisposition::Removed => json!(workspace.disposition),
        WorkspaceSettlementDisposition::Retained {
            handoff,
            cleanup_error,
        } => json!({"type":"retained", "handoff":handoff,"cleanup_failed":cleanup_error.is_some()}),
        WorkspaceSettlementDisposition::PreservedUnresolved { reason, .. } => {
            json!({"type":"preserved_unresolved", "reason":reason})
        }
    };
    json!({"snapshot":workspace.snapshot,"disposition":disposition})
}
#[allow(clippy::too_many_lines)] // Exhaustive authority audit; new event variants require a decision.
fn event(event: &crate::events::types::RuntimeEvent) -> Value {
    use crate::events::types::RuntimeEvent;
    match event {
        RuntimeEvent::WorkflowWorkspaceDisposalSettled { run_id, settlement } => {
            use crate::runtime::workspace::WorkspaceDisposalSettlement;
            let kind = match settlement {
                WorkspaceDisposalSettlement::NothingRemoved { .. } => "nothing_removed",
                WorkspaceDisposalSettlement::WorktreeRemoved { .. } => "worktree_removed",
                WorkspaceDisposalSettlement::Disposed => "disposed",
                WorkspaceDisposalSettlement::AlreadyDisposed => "already_disposed",
            };
            json!({"type":"workflow_workspace_disposal_settled","run_id":run_id,"settlement":{"type":kind}})
        }
        RuntimeEvent::AttemptCompleted {
            attempt_id,
            finish_reason: reason,
        } => {
            json!({"type":"attempt_completed","attempt_id":attempt_id,"finish_reason":finish_reason(reason)})
        }
        RuntimeEvent::ModelRequestCompleted {
            request_id,
            finish_reason: reason,
            usage,
            generation,
        } => {
            json!({"type":"model_request_completed","request_id":request_id,"finish_reason":finish_reason(reason),"usage":usage,"generation":generation})
        }
        RuntimeEvent::ModelRequestFailed {
            request_id,
            error,
            usage,
            generation,
        } => {
            json!({"type":"model_request_failed","request_id":request_id,"error":model_error(error),"usage":usage,"generation":generation})
        }
        RuntimeEvent::AttemptFailed { attempt_id, error } => {
            json!({"type":"attempt_failed","attempt_id":attempt_id,"error":attempt_failure(error)})
        }
        RuntimeEvent::CompactionFailed { .. } => {
            json!({"type":"compaction_failed","diagnostic_unavailable":"runtime diagnostic excluded"})
        }
        RuntimeEvent::ToolExecutionFailed {
            tool_call_id,
            tool_id,
            ..
        } => {
            json!({"type":"tool_execution_failed","tool_call_id":tool_call_id,"tool_id":tool_id,"diagnostic_unavailable":"executor diagnostic excluded"})
        }
        RuntimeEvent::ToolExecutionSettlementControlFailed {
            tool_call_id,
            tool_id,
            ..
        } => {
            json!({"type":"tool_execution_settlement_control_failed","tool_call_id":tool_call_id,"tool_id":tool_id,"diagnostic_unavailable":"executor diagnostic excluded"})
        }
        RuntimeEvent::ToolExecutionCompleted {
            tool_call_id,
            tool_id,
            result,
        } => {
            json!({"type":"tool_execution_completed", "tool_call_id":tool_call_id, "tool_id":tool_id, "result":tool_execution_result(result)})
        }
        RuntimeEvent::WorkflowCandidateInvocation {
            node,
            input,
            result,
            candidate_unchanged,
        } => {
            json!({"type":"workflow_candidate_invocation", "node":node, "input":input, "result":tool_execution_status(result), "candidate_unchanged":candidate_unchanged})
        }
        RuntimeEvent::WorkflowFailed {
            workflow_id,
            run_id,
            status,
            ..
        } => {
            json!({"type":"workflow_failed","workflow_id":workflow_id,"run_id":run_id,"status":tool_execution_status(status),"diagnostic_unavailable":"runtime diagnostic excluded"})
        }
        RuntimeEvent::SubagentTerminalPublished {
            subagent_id,
            child_agent_id,
            message_id,
            state,
            workspace_resource,
        } => {
            json!({"type":"subagent_terminal_published","subagent_id":subagent_id,"child_agent_id":child_agent_id,"message_id":message_id,"state":state,"workspace_resource":subagent_resource(workspace_resource)})
        }
        RuntimeEvent::SubagentTerminalSettled {
            subagent_id,
            child_agent_id,
            state,
            workspace_resource,
        } => {
            json!({"type":"subagent_terminal_settled","subagent_id":subagent_id,"child_agent_id":child_agent_id,"state":state,"workspace_resource":subagent_resource(workspace_resource)})
        }
        RuntimeEvent::WorkflowWorkspaceSettled {
            run_id,
            workspace: settlement,
            candidate,
            ..
        } => {
            json!({"type":"workflow_workspace_settled","run_id":run_id,"workspace":workspace(settlement),"candidate":candidate})
        }
        RuntimeEvent::NativeToolInvocation {
            invocation_id,
            tool_id,
            fact,
        } => {
            use crate::tools::invocation::{InvocationFact, NativeInvocationFact};
            let fact = match fact {
                NativeInvocationFact::Lifecycle {
                    fact: InvocationFact::SettlementControlFailed { .. },
                } => {
                    json!({"type":"lifecycle","fact":{"type":"settlement_control_failed","diagnostic_unavailable":"executor diagnostic excluded"}})
                }
                NativeInvocationFact::Completed { status } => {
                    json!({"type":"completed", "status":tool_execution_status(status)})
                }
                NativeInvocationFact::Prepared { .. }
                | NativeInvocationFact::Started
                | NativeInvocationFact::Progress { .. }
                | NativeInvocationFact::Lifecycle {
                    fact:
                        InvocationFact::Deadline { .. }
                        | InvocationFact::CancellationRequested { .. }
                        | InvocationFact::SettlementObserved { .. },
                } => json!(fact),
            };
            json!({"type":"native_tool_invocation","invocation_id":invocation_id,"tool_id":tool_id,"fact":fact})
        }
        // Native values here carry identities, authored content, closed semantic
        // outcomes, measurements or model-visible Tool results; no provider binding.
        RuntimeEvent::Goal { .. }
        | RuntimeEvent::AttemptStarted { .. }
        | RuntimeEvent::AttemptCancelled { .. }
        | RuntimeEvent::AttemptTimedOut { .. }
        | RuntimeEvent::AttemptLimitExceeded { .. }
        | RuntimeEvent::TurnStarted
        | RuntimeEvent::TurnCompleted
        | RuntimeEvent::ModelRequestStarted { .. }
        | RuntimeEvent::ContextContributionEmitted { .. }
        | RuntimeEvent::ModelRetryScheduled { .. }
        | RuntimeEvent::InboundTurnAdopted { .. }
        | RuntimeEvent::AssistantMessageCommitted { .. }
        | RuntimeEvent::ToolExecutionStarted { .. }
        | RuntimeEvent::ToolExecutionProgress { .. }
        | RuntimeEvent::ToolExecutionDeadlineFired { .. }
        | RuntimeEvent::ToolExecutionCancellationRequested { .. }
        | RuntimeEvent::ToolExecutionSettlementObserved { .. }
        | RuntimeEvent::ToolMessageCommitted { .. }
        | RuntimeEvent::CompactionStarted
        | RuntimeEvent::CompactionCompleted { .. }
        | RuntimeEvent::BackgroundExecutionCommitted { .. }
        | RuntimeEvent::BackgroundTerminalPublished { .. }
        | RuntimeEvent::SubagentOwnershipCommitted { .. }
        | RuntimeEvent::SubagentWorkspaceDisposalStarted { .. }
        | RuntimeEvent::SubagentWorkspaceDisposalSettled { .. }
        | RuntimeEvent::WorkflowWorkspaceOwned { .. }
        | RuntimeEvent::WorkflowWorkspaceDisposalStarted { .. }
        | RuntimeEvent::WorkflowStarted { .. }
        | RuntimeEvent::WorkflowBlockStarted { .. }
        | RuntimeEvent::WorkflowLoopIterationAdmitted { .. }
        | RuntimeEvent::WorkflowLoopIterationSettled { .. }
        | RuntimeEvent::WorkflowLoopExited { .. }
        | RuntimeEvent::WorkflowBlockSettled { .. }
        | RuntimeEvent::WorkflowNodeStarted { .. }
        | RuntimeEvent::WorkflowNodeSettled { .. }
        | RuntimeEvent::WorkflowAgentAdmitted { .. }
        | RuntimeEvent::WorkflowAgentOutputCommitted { .. }
        | RuntimeEvent::WorkflowBranchSelected { .. }
        | RuntimeEvent::WorkflowParallelAdmitted { .. }
        | RuntimeEvent::WorkflowParallelSettled { .. }
        | RuntimeEvent::WorkflowCompleted { .. }
        | RuntimeEvent::WorkflowCancelled { .. }
        | RuntimeEvent::InteractionRequested { .. }
        | RuntimeEvent::InteractionSettled { .. } => json!(event),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_managed_output_preserves_state_and_exact_locators() {
        use crate::tools::types::ManagedOutputContinuation as Continuation;
        let locator =
            std::path::PathBuf::from("/home/user/project/.rustx/tool-output/tasks/exec_42.output");
        for (continuation, expected) in [
            (
                Continuation::Complete {
                    locator: locator.clone(),
                },
                json!({"type":"complete","locator":locator}),
            ),
            (
                Continuation::Partial {
                    locator: locator.clone(),
                    diagnostic: "ARCHIVE_MANAGED_PARTIAL_DIAGNOSTIC".into(),
                },
                json!({"type":"partial","locator":locator,
                    "diagnostic_unavailable":"output-storage diagnostic excluded"}),
            ),
            (
                Continuation::Unavailable {
                    diagnostic: "ARCHIVE_MANAGED_UNAVAILABLE_DIAGNOSTIC".into(),
                },
                json!({"type":"unavailable",
                    "diagnostic_unavailable":"output-storage diagnostic excluded"}),
            ),
        ] {
            assert_eq!(managed_output_continuation(&continuation), expected);
        }
    }

    #[test]
    fn archive_tool_status_preserves_only_closed_execution_semantics() {
        use crate::runtime::types::CancellationReason;
        use crate::tools::types::{ToolCancellationPhase, ToolExecutionStatus as Status};
        for (status, expected) in [
            (Status::Success, json!({"type":"success"})),
            (
                Status::Failed {
                    error: "ARCHIVE_NATIVE_FAILED_DIAGNOSTIC".into(),
                },
                json!({"type":"failed","diagnostic_unavailable":"executor diagnostic excluded"}),
            ),
            (
                Status::Denied {
                    reason: "ARCHIVE_NATIVE_DENIED_DIAGNOSTIC".into(),
                },
                json!({"type":"denied","diagnostic_unavailable":"executor diagnostic excluded"}),
            ),
            (
                Status::OutcomeUnknown {
                    detail: "ARCHIVE_NATIVE_UNKNOWN_DIAGNOSTIC".into(),
                },
                json!({"type":"outcome_unknown","diagnostic_unavailable":"executor diagnostic excluded"}),
            ),
            (Status::TimedOut, json!({"type":"timed_out"})),
        ] {
            assert_eq!(tool_execution_status(&status), expected);
            let native = crate::events::types::RuntimeEvent::NativeToolInvocation {
                invocation_id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: crate::runtime::identity::ToolCallId::new("call"),
                },
                tool_id: crate::runtime::identity::ToolId::new("tool"),
                fact: crate::tools::invocation::NativeInvocationFact::Completed { status },
            };
            assert_eq!(
                event(&native)["fact"],
                json!({"type":"completed","status":expected})
            );
        }
        for (phase, encoded) in [
            (ToolCancellationPhase::BeforeStart, "before_start"),
            (ToolCancellationPhase::DuringExecution, "during_execution"),
        ] {
            assert_eq!(
                tool_execution_status(&Status::Cancelled {
                    reason: CancellationReason::ParentCancelled,
                    phase,
                }),
                json!({"type":"cancelled","reason":"parent_cancelled","phase":encoded})
            );
        }
    }

    #[test]
    fn archive_runtime_diagnostics_are_not_authored_history() {
        let private = "ARCHIVE_INTERNAL_DIAGNOSTIC_SECRET";
        let failure = crate::events::types::AttemptFailure::Runtime {
            error: crate::runtime::types::RuntimeError::Internal {
                message: private.into(),
            },
        };
        let encoded = attempt_failure(&failure);
        assert_eq!(encoded["error"]["type"], "internal");
        assert!(!encoded.to_string().contains(private));
        let reason = finish_reason(&crate::model::ModelFinishReason::Other {
            reason: private.into(),
        });
        assert_eq!(reason["type"], "other");
        assert!(!reason.to_string().contains(private));
        let authored = crate::runtime::types::RuntimeError::UnknownTool {
            name: "AUTHORED_NAME_SECRET".into(),
        };
        assert_eq!(runtime_error(&authored)["name"], "AUTHORED_NAME_SECRET");
    }
}
