//! Versioned stream projections for native types with mixed historical/private ownership.
use crate::model::snapshot::RequestSnapshot;
use serde::Serialize;
use serde_json::{Value, json};

/// Explicit schema: new durable fields cannot silently become archive fields.
#[derive(Serialize)]
struct ArchiveRequestSnapshotV4<'a> {
    exact_reconstruction: bool,
    missing_input_components: [&'static str; 3],
    semantic_input_fingerprint: &'a str,
    request_id: &'a crate::runtime::identity::RequestId,
    identity: &'a crate::model::snapshot::RequestIdentity,
    provisional_message_id: &'a crate::runtime::identity::MessageId,
    canonical_frontier: &'a crate::context::resources::LedgerFrontier,
    effective_system_prompt: &'a String,
    system_sections: &'a Vec<crate::context::assembly::AcceptedSystemSection>,
    context_window_tokens: &'a u64,
    profile: &'a Option<crate::model::catalog::ModelProfileId>,
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
    json!(ArchiveRequestSnapshotV4 {
        exact_reconstruction: false,
        missing_input_components: [
            "complete_model_invocation",
            "frozen_typed_input_and_image_bytes",
            "provider_native_replay_state"
        ],
        semantic_input_fingerprint: &snapshot.semantic_input_fingerprint,
        request_id: &snapshot.request_id,
        identity: &snapshot.identity,
        provisional_message_id: &snapshot.provisional_message_id,
        canonical_frontier: &snapshot.canonical_frontier,
        effective_system_prompt: &snapshot.effective_system_prompt,
        system_sections: &snapshot.system_sections,
        context_window_tokens: &snapshot.context_window_tokens,
        profile: &snapshot.profile,
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
/// have explicit restricted payloads. The exhaustive match requires review of new facts.
pub(super) fn journal(envelope: &crate::events::types::RuntimeEventEnvelope) -> Value {
    json!({"schema_version": envelope.schema_version, "event_id": envelope.event_id,
        "sequence": envelope.sequence, "conversation_id": envelope.conversation_id,
        "attempt_id": envelope.attempt_id, "turn_id": envelope.turn_id,
        "timestamp": envelope.timestamp, "event": event(&envelope.event)})
}
/// Archive Journal status is outcome evidence, not executor diagnostic prose.
/// Exhaustive by design: a new status must receive an explicit projection decision.
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
/// Exhaustive by design so new continuation variants require a projection decision.
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
        "managed_output":result.managed_output.as_deref().map(managed_output_continuation),
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

fn auxiliary_finish_reason(reason: crate::model::auxiliary::AuxiliaryFinishReason) -> Value {
    json!({"type":reason})
}

fn auxiliary_input(input: &crate::model::auxiliary::AuxiliaryInputEvidence) -> Value {
    use crate::model::auxiliary::AuxiliaryInputEvidence;
    match input {
        AuxiliaryInputEvidence::Temporary => json!({"mode":"temporary"}),
        AuxiliaryInputEvidence::Historical { cut } => json!({"mode":"historical","cut":{
            "conversation_id":cut.conversation_id, "ledger":cut.ledger,
            "journal":cut.journal, "transcript":cut.transcript,
            "projection_activation":cut.projection_activation,
        }}),
    }
}

fn auxiliary_business_source(source: &crate::model::auxiliary::AuxiliaryBusinessSource) -> Value {
    use crate::model::auxiliary::AuxiliaryBusinessSource;
    match source {
        AuxiliaryBusinessSource::Independent => json!({"kind":"independent"}),
        AuxiliaryBusinessSource::Submission { identity } => {
            json!({"kind":"submission","identity":identity})
        }
        AuxiliaryBusinessSource::Business {
            namespace,
            reference,
        } => {
            json!({"kind":"business","namespace":namespace,"reference":reference})
        }
        AuxiliaryBusinessSource::Compaction { operation_id } => {
            json!({"kind":"compaction","operation_id":operation_id})
        }
        AuxiliaryBusinessSource::HumanMessage { message_id } => {
            json!({"kind":"human_message","message_id":message_id})
        }
    }
}

fn auxiliary_call(call: &crate::model::auxiliary::AuxiliaryCallAudit) -> Value {
    json!({
        "call_id":call.call_id, "purpose":call.purpose, "trigger":call.trigger,
        "business_source":auxiliary_business_source(&call.business_source),
        "input":auxiliary_input(&call.input),
        "semantic_input_fingerprint":call.semantic_input_fingerprint,
        "invocation_fingerprint":call.invocation_fingerprint,
        "selected_message_ids":call.selected_message_ids,
        "started_sequence":call.started_sequence, "settled_sequence":call.settled_sequence,
        "outcome":call.outcome,
    })
}

fn auxiliary_request(request: &crate::model::auxiliary::AuxiliaryRequestAudit) -> Value {
    json!({
        "call_id":request.call_id,"ordinal":request.ordinal,"request_id":request.request_id,
        "model":request.model,"started_sequence":request.started_sequence,
        "terminal_sequence":request.terminal_sequence,"local_terminal":request.local_terminal,
        "normalized_failure_kind":request.normalized_failure_kind,
        "generation":request.generation,"usage":request.usage,
        "request_evidence":request.request_evidence.as_ref().map(auxiliary_request_evidence),
    })
}

/// Inherited native audit is projected explicitly just like local Journal facts.
pub(super) fn inherited_auxiliary_record(mut value: Value) -> std::io::Result<Value> {
    use crate::model::auxiliary::AuxiliaryAuditFact;
    let fact = value
        .get_mut("fact")
        .ok_or_else(|| std::io::Error::other("inherited auxiliary audit has no fact"))?;
    let native: AuxiliaryAuditFact =
        serde_json::from_value(fact.take()).map_err(std::io::Error::other)?;
    *fact = match &native {
        AuxiliaryAuditFact::CallStarted(call) => json!({"CallStarted":auxiliary_call(call)}),
        AuxiliaryAuditFact::CallSettled(call) => json!({"CallSettled":auxiliary_call(call)}),
        AuxiliaryAuditFact::RequestStarted(request) => {
            json!({"RequestStarted":auxiliary_request(request)})
        }
        AuxiliaryAuditFact::RetryScheduled(request) => {
            json!({"RetryScheduled":auxiliary_request(request)})
        }
        AuxiliaryAuditFact::RequestSettled(request) => {
            json!({"RequestSettled":auxiliary_request(request)})
        }
    };
    Ok(value)
}

fn auxiliary_request_evidence(
    evidence: &crate::model::auxiliary::AuxiliaryRequestEvidence,
) -> Value {
    use crate::model::auxiliary::AuxiliaryObservedProviderTerminal;
    let observed = evidence
        .observed_provider_terminal
        .as_ref()
        .map(|terminal| match terminal {
            AuxiliaryObservedProviderTerminal::Completed {
                finish_reason: reason,
            } => json!({"type":"completed","finish_reason":auxiliary_finish_reason(*reason)}),
            AuxiliaryObservedProviderTerminal::Failed { kind } => {
                json!({"type":"failed","kind":kind})
            }
        });
    json!({
        "request_id":evidence.request_id,
        "observed_provider_terminal":observed,
        "usage":evidence.usage,
        "usage_completeness":evidence.usage_completeness,
        "remote_execution_uncertainty":evidence.remote_execution_uncertainty,
    })
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
#[allow(clippy::too_many_lines)] // Exhaustive authority audit; new event variants require a decision.
fn event(event: &crate::events::types::RuntimeEvent) -> Value {
    use crate::events::types::RuntimeEvent;
    match event {
        RuntimeEvent::AuxiliaryCallStarted {
            call_id,
            purpose,
            trigger,
            business_source,
            input,
            semantic_input_fingerprint,
            invocation_fingerprint,
            selected_message_ids,
        } => json!({
            "type":"auxiliary_call_started", "call_id":call_id,
            "purpose":purpose, "trigger":trigger, "business_source":auxiliary_business_source(business_source), "input":auxiliary_input(input),
            "semantic_input_fingerprint":semantic_input_fingerprint,
            "invocation_fingerprint":invocation_fingerprint, "selected_message_ids":selected_message_ids,
        }),
        RuntimeEvent::AuxiliaryRetryScheduled {
            call_id,
            request_id,
            next_ordinal,
            delay_ms,
            request_evidence,
        } => {
            json!({"type":"auxiliary_retry_scheduled","call_id":call_id,"request_id":request_id,"next_ordinal":next_ordinal,"delay_ms":delay_ms,"request_evidence":auxiliary_request_evidence(request_evidence)})
        }
        RuntimeEvent::AuxiliaryCallSettled {
            call_id,
            outcome,
            request_evidence,
        } => json!({"type":"auxiliary_call_settled","call_id":call_id,"outcome":outcome,
                "request_evidence":request_evidence.as_ref().map(auxiliary_request_evidence)}),
        RuntimeEvent::AttemptCompleted {
            attempt_id,
            finish_reason: reason,
        } => {
            json!({"type":"attempt_completed","attempt_id":attempt_id,"finish_reason":finish_reason(reason)})
        }
        RuntimeEvent::ModelRequestCompleted {
            owner,
            request_id,
            finish_reason: reason,
            usage,
            generation,
        } => {
            json!({"type":"model_request_completed","owner":owner,"request_id":request_id,"finish_reason":finish_reason(reason),"usage":usage,"generation":generation})
        }
        RuntimeEvent::ModelRequestFailed {
            owner,
            request_id,
            error,
            usage,
            generation,
        } => {
            json!({"type":"model_request_failed","owner":owner,"request_id":request_id,"error":model_error(error),"usage":usage,"generation":generation})
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
        RuntimeEvent::WorkflowFailed {
            workflow_id,
            run_id,
            status,
            ..
        } => {
            json!({"type":"workflow_failed","workflow_id":workflow_id,"run_id":run_id,"status":tool_execution_status(status),"diagnostic_unavailable":"runtime diagnostic excluded"})
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
        | RuntimeEvent::CompactionStarted { .. }
        | RuntimeEvent::CompactionCompleted { .. }
        | RuntimeEvent::BackgroundExecutionCommitted { .. }
        | RuntimeEvent::BackgroundTerminalPublished { .. }
        | RuntimeEvent::AgentActivationAdmission { .. }
        | RuntimeEvent::SubagentOwnershipCommitted { .. }
        | RuntimeEvent::SubagentTerminalPublished { .. }
        | RuntimeEvent::SubagentTerminalSettled { .. }
        | RuntimeEvent::SubagentPhysicalSettlementProven { .. }
        | RuntimeEvent::WorkflowStarted { .. }
        | RuntimeEvent::WorkflowBlockStarted { .. }
        | RuntimeEvent::WorkflowLoopIterationAdmitted { .. }
        | RuntimeEvent::WorkflowLoopIterationSettled { .. }
        | RuntimeEvent::WorkflowLoopExited { .. }
        | RuntimeEvent::WorkflowBlockSettled { .. }
        | RuntimeEvent::WorkflowNodeStarted { .. }
        | RuntimeEvent::WorkflowNodeWaiting { .. }
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

/// Export inspection bytes and scoped provenance without claiming exact native replay.
/// No destination execution or canonical identities are invented for Derived messages.
pub(super) fn checkpoint(checkpoint: &crate::durable::projection::ProjectionCheckpoint) -> Value {
    use crate::context::checkpoint::CheckpointMessage;
    use crate::message::types::AssistantContentBlock;
    use crate::model::projected::ProjectedMessage;
    let items=checkpoint.candidate.items.iter().map(|item| {
        let message=match &item.message {
            CheckpointMessage::Original(reference)=>json!({"type":"original","reference":reference}),
            CheckpointMessage::Derived(derived)=>{
                let body=match &derived.message {
                    ProjectedMessage::Assistant {content}=>json!({"role":"assistant","content":content.iter().map(|block|match block {
                        AssistantContentBlock::Reasoning(reasoning)=>json!({"type":"reasoning","text":reasoning.text,"native_replay_unavailable":true}),
                        AssistantContentBlock::Text(_) | AssistantContentBlock::ToolCall(_) | AssistantContentBlock::Refusal(_) | AssistantContentBlock::Image(_)=>json!(block),
                    }).collect::<Vec<_>>()}),
                    ProjectedMessage::User {..} | ProjectedMessage::Tool {..}=>json!(derived.message),
                };
                json!({"type":"derived","body":body,"sources":derived.sources,"checkpoint_sources":derived.checkpoint_sources,"transformations":derived.transformations})
            }
        };
        json!({"message":message,"call_sources":item.call_sources,"result_call":item.result_call})
    }).collect::<Vec<_>>();
    json!({"exact_reconstruction":false,"missing_input_components":["provider_native_replay_state"],
        "checkpoint_id":checkpoint.candidate.checkpoint_id,"candidate_fingerprint":checkpoint.fingerprint,
        "expected_head":checkpoint.candidate.expected_head,"read_cut":checkpoint.candidate.read_cut,
        "base_frontier":checkpoint.candidate.base_frontier,"items":items,
        "summary_metadata":checkpoint.candidate.summary_metadata,
        "summary_generation":checkpoint.candidate.summary_generation,
        "activation":checkpoint.activation,"occupancy":checkpoint.occupancy})
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
    #[test]
    fn inherited_auxiliary_provider_finish_diagnostic_is_projected_out() {
        use crate::model::auxiliary::*;
        let private = "PRIVATE_PROVIDER_FINISH";
        let provider_finish = crate::model::ModelFinishReason::Other {
            reason: private.into(),
        };
        let request_id = crate::runtime::identity::RequestId::new("request");
        let request = AuxiliaryRequestAudit {
            call_id: crate::runtime::identity::AuxiliaryCallId::generate(),
            ordinal: 0,
            request_id: request_id.clone(),
            model: "model".into(),
            started_sequence: 1,
            terminal_sequence: Some(2),
            local_terminal: Some(AuxiliaryRequestTerminal::Completed),
            normalized_failure_kind: None,
            generation: None,
            usage: None,
            request_evidence: Some(AuxiliaryRequestEvidence {
                request_id,
                observed_provider_terminal: Some(AuxiliaryObservedProviderTerminal::Completed {
                    finish_reason: AuxiliaryFinishReason::from(&provider_finish),
                }),
                usage: None,
                usage_completeness: AuxiliaryUsageCompleteness::Unreported,
                remote_execution_uncertainty: false,
            }),
        };
        let projected = inherited_auxiliary_record(json!({
            "origin":{"type":"inherited"}, "fact":AuxiliaryAuditFact::RequestSettled(request),
        }))
        .unwrap();
        assert!(!projected.to_string().contains(private));
        assert_eq!(
            projected["fact"]["RequestSettled"]["request_evidence"]["observed_provider_terminal"]["finish_reason"]
                ["type"],
            "other"
        );
    }
}
