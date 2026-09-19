//! Bounded summary-record construction, one anchor at a time.
//!
//! Each anchor resolves its own native grouping, its own terminal boundary,
//! and a one-line preview of its content. A record never borrows a fact from
//! a neighbouring anchor: a Tool's outcome comes from that Tool's own
//! terminal, a request's usage from that request's own terminal fact, and a
//! step's end from that step's own completion.

use super::bounds::{TRACE_RECORD_BYTES, TracePreview, encoded_len, identity_fits};
use super::content::{message_preview, tool_outcome, tool_status_detail};
use super::types::{
    TraceArtifact, TraceCursor, TraceGeneration, TraceKind, TraceLocation, TraceRecord,
    TraceRequestSummary, TraceState, TraceTiming, TraceToolCall, TraceToolSummary,
};
use super::{ADOPTED_MESSAGE_LIMIT, TraceProjection};
use crate::durable::ConversationStoreError;
use crate::durable::presentation::FactScope;
use crate::events::types::{RuntimeEvent as E, RuntimeEventEnvelope};
use crate::message::types::{AssistantContentBlock, MessageBlock};
use crate::model::generation_evidence::GenerationEvidence;
use crate::model::types::ModelUsage;
use crate::tools::types::ToolExecutionStatus;

impl TraceProjection<'_> {
    /// Projects one anchor into its bounded summary record.
    #[allow(clippy::too_many_lines)] // One closed anchor vocabulary, one place.
    pub(super) fn record(
        &self,
        anchor: &RuntimeEventEnvelope,
    ) -> Result<TraceRecord, ConversationStoreError> {
        let mut record = TraceRecord {
            id: format!("trace:{}", anchor.sequence),
            position: TraceCursor::at(anchor.sequence),
            location: TraceLocation {
                attempt_id: anchor.attempt_id.clone(),
                step_id: anchor.turn_id.clone(),
            },
            kind: TraceKind::Step,
            state: TraceState::Incomplete,
            timing: TraceTiming {
                started_at: anchor.timestamp,
                ended_at: None,
                duration_ms: None,
            },
            preview: None,
            request: None,
            tool: None,
            calls: vec![],
            native_id: None,
            message_id: None,
            attachments: vec![],
            has_detail: false,
            truncated: false,
        };
        let ending = match &anchor.event {
            E::AttemptStarted { attempt_id } => {
                record.kind = TraceKind::Attempt;
                self.ending(
                    FactScope::Attempt(attempt_id.clone()),
                    &[
                        "attempt_completed",
                        "attempt_failed",
                        "attempt_cancelled",
                        "attempt_timed_out",
                        "attempt_limit_exceeded",
                    ],
                )?
            }
            E::TurnStarted => {
                if let (Some(attempt), Some(turn)) = (&anchor.attempt_id, &anchor.turn_id) {
                    self.ending(
                        FactScope::Step(attempt.clone(), turn.clone()),
                        &["turn_completed"],
                    )?
                } else {
                    None
                }
            }
            E::InboundTurnAdopted { message_ids } => {
                // Adoption is the canonical linearization point at which
                // inbound became a turn this conversation owes an answer
                // for. It is instantaneous: it has no terminal boundary and
                // therefore no duration.
                record.kind = TraceKind::User;
                record.state = TraceState::Completed;
                record.has_detail = !message_ids.is_empty();
                record.message_id = message_ids
                    .first()
                    .filter(|id| identity_fits(id.as_str()))
                    .cloned();
                for message in self
                    .store
                    .load_messages(&message_ids[..message_ids.len().min(ADOPTED_MESSAGE_LIMIT)])?
                {
                    if record.preview.is_none() {
                        record.preview = message_preview(&message);
                    }
                }
                record.truncated = message_ids.len() > ADOPTED_MESSAGE_LIMIT;
                None
            }
            E::ModelRequestStarted { request_id, .. } => {
                record.kind = TraceKind::Request;
                record.has_detail = true;
                let frozen = self.store.load_request_snapshot(request_id)?;
                // The frozen snapshot owns this request's grouping. The
                // anchor's own correlation columns agree, but the snapshot is
                // the authority the ordinal is read from.
                record.location = TraceLocation {
                    attempt_id: Some(frozen.identity.attempt_id.clone()),
                    step_id: Some(frozen.identity.turn.clone()),
                };
                let end = self.ending(
                    FactScope::Request(request_id.to_string()),
                    &["model_request_completed", "model_request_failed"],
                )?;
                let (failure_kind, usage, generation) = request_terminal(end.as_ref());
                record.preview = Some(TracePreview::of(&frozen.invocation.model));
                record.request = Some(TraceRequestSummary {
                    previous_failure_kind: self.previous_request_failure(&frozen)?,
                    assistant_message_id: frozen.provisional_message_id.clone(),
                    request_id: request_id.clone(),
                    retry_number: frozen.identity.retry_number,
                    model: frozen.invocation.model.clone(),
                    failure_kind,
                    usage: usage.clone(),
                    generation: generation
                        .map(|evidence| generation_metrics(evidence, usage.as_ref())),
                });
                end
            }
            E::AssistantMessageCommitted { message_id } => {
                record.kind = TraceKind::Assistant;
                // Acceptance is the canonical commit itself; the record is
                // complete at the instant it exists.
                record.state = TraceState::Completed;
                record.has_detail = true;
                record.message_id = Some(message_id.clone());
                for message in self.store.load_messages(std::slice::from_ref(message_id))? {
                    record.preview = message_preview(&message);
                    if let MessageBlock::Assistant(assistant) = message {
                        for block in &assistant.content {
                            match block {
                                AssistantContentBlock::ToolCall(call)
                                    if identity_fits(call.id.as_str())
                                        && identity_fits(call.tool_id.as_str()) =>
                                {
                                    record.calls.push(TraceToolCall {
                                        call_id: call.id.clone(),
                                        tool_id: call.tool_id.clone(),
                                        name: call.name.clone(),
                                    });
                                }
                                AssistantContentBlock::Image(image)
                                    if identity_fits(image.artifact_id.as_str()) =>
                                {
                                    record.attachments.push(TraceArtifact {
                                        artifact_id: image.artifact_id.clone(),
                                        image: true,
                                        name: None,
                                        mime_type: None,
                                    });
                                }
                                _ => {}
                            }
                        }
                    }
                }
                None
            }
            E::ToolExecutionStarted {
                tool_call_id,
                tool_id,
            } => {
                record.kind = TraceKind::Tool;
                record.has_detail = true;
                let scope = || FactScope::ToolCall {
                    call_id: tool_call_id.to_string(),
                    attempt: anchor.attempt_id.clone(),
                    turn: anchor.turn_id.clone(),
                };
                let end = self
                    .ending(
                        scope(),
                        &["tool_execution_completed", "tool_execution_failed"],
                    )?
                    // Both the call and the Tool identity must match. Names
                    // and positions never pair an outcome to a call.
                    .filter(|event| match &event.event {
                        E::ToolExecutionCompleted { tool_id: id, .. }
                        | E::ToolExecutionFailed { tool_id: id, .. } => id == tool_id,
                        _ => false,
                    });
                let mut summary = TraceToolSummary {
                    call_id: tool_call_id.clone(),
                    tool_id: tool_id.clone(),
                    name: None,
                    // This anchor *is* the durable start fact, so execution
                    // is proven for this record by construction.
                    started: true,
                    outcome: None,
                    detail: None,
                };
                if let Some(commit) = self.ending(scope(), &["tool_message_committed"])?
                    && let E::ToolMessageCommitted { message_id, .. } = commit.event
                {
                    for message in self
                        .store
                        .load_messages(std::slice::from_ref(&message_id))?
                    {
                        if let MessageBlock::Tool(tool) = &message
                            && tool.tool_call_id == *tool_call_id
                            && tool.tool_id == *tool_id
                        {
                            record.message_id = Some(message_id.clone());
                            summary.outcome = Some(tool_outcome(&tool.result.status));
                            summary.detail = tool_status_detail(&tool.result.status)
                                .as_deref()
                                .map(TracePreview::of);
                            record.preview = message_preview(&message);
                            let (attachments, truncated) =
                                super::content::tool_result_artifacts(&tool.result);
                            record.attachments = attachments;
                            record.truncated |= truncated;
                        }
                    }
                }
                summary
                    .name
                    .clone_from(&self.tool_call_name(anchor, tool_call_id, tool_id)?);
                if record.preview.is_none()
                    && let Some(name) = summary.name.as_deref()
                {
                    record.preview = Some(TracePreview::of(name));
                }
                record.tool = Some(summary);
                end
            }
            E::CompactionStarted => {
                record.kind = TraceKind::Compaction;
                // Compaction has no native operation identity. Its boundary
                // is the next compaction start or terminal in durable order.
                let end = self
                    .store
                    .read_presentation_events(&crate::durable::presentation::FactQuery {
                        scope: FactScope::All,
                        kinds: vec![
                            "compaction_started",
                            "compaction_completed",
                            "compaction_failed",
                        ],
                        before: None,
                        after: anchor.sequence,
                        ascending: true,
                        through: self.through,
                        limit: 1,
                    })?
                    .pop()
                    .filter(|event| !matches!(event.event, E::CompactionStarted));
                if let Some(event) = end.as_ref()
                    && let E::CompactionCompleted {
                        summary_message_id, ..
                    } = &event.event
                {
                    record.has_detail = true;
                    record.message_id = Some(summary_message_id.clone());
                    for message in self
                        .store
                        .load_messages(std::slice::from_ref(summary_message_id))?
                    {
                        record.preview = message_preview(&message);
                    }
                }
                end
            }
            E::BackgroundExecutionCommitted { execution_id, .. } => {
                record.kind = TraceKind::Background;
                record.native_id = Some(execution_id.to_string());
                self.ending(
                    FactScope::Execution(execution_id.to_string()),
                    &["background_terminal_published"],
                )?
            }
            E::SubagentOwnershipCommitted {
                subagent_id, agent, ..
            } => {
                record.kind = TraceKind::Subagent;
                record.native_id = Some(subagent_id.to_string());
                record.preview = Some(TracePreview::of(agent.as_str()));
                self.ending(
                    FactScope::Subagent(subagent_id.to_string()),
                    &["subagent_terminal_published", "subagent_terminal_settled"],
                )?
            }
            E::WorkflowStarted { run_id, .. } => {
                record.kind = TraceKind::Workflow;
                let id = serde_json::to_string(run_id).map_err(|_| {
                    ConversationStoreError::InvalidReference("invalid Workflow identity".into())
                })?;
                record.native_id = Some(id.clone());
                self.ending(
                    FactScope::Workflow(id),
                    &[
                        "workflow_completed",
                        "workflow_failed",
                        "workflow_cancelled",
                    ],
                )?
            }
            E::InteractionRequested { interaction_id, .. } => {
                record.kind = TraceKind::Interaction;
                record.native_id = Some(interaction_id.to_string());
                self.ending(
                    FactScope::Interaction(interaction_id.to_string()),
                    &["interaction_settled"],
                )?
            }
            _ => unreachable!("allowlisted anchors only"),
        };
        if let Some(end) = ending.filter(|end| end.sequence > anchor.sequence) {
            record.state = terminal(&end.event);
            record.timing.ended_at = Some(end.timestamp);
            // Both endpoints exist, so the duration is measured rather than
            // assumed. A record with one endpoint keeps none.
            record.timing.duration_ms =
                u64::try_from((end.timestamp - anchor.timestamp).num_milliseconds()).ok();
        }
        Ok(record)
    }

    /// The exact preceding actual request's failure class, when recorded.
    ///
    /// The previous request is named by the native ordinal, not by scanning
    /// timestamps: retry evidence is the frozen `retry_number`, and nothing
    /// else is treated as proof that one request followed another.
    fn previous_request_failure(
        &self,
        frozen: &crate::model::snapshot::RequestSnapshot,
    ) -> Result<Option<crate::model::error::ModelErrorKind>, ConversationStoreError> {
        if frozen.identity.retry_number == 0 {
            return Ok(None);
        }
        let mut identity = frozen.identity.clone();
        identity.retry_number -= 1;
        Ok(self
            .ending(
                FactScope::Request(identity.request_id().to_string()),
                &["model_request_failed"],
            )?
            .and_then(|event| match event.event {
                E::ModelRequestFailed { error, .. } => Some(error.kind),
                _ => None,
            }))
    }

    /// The recorded model-facing name of one started Tool call.
    ///
    /// Read from the canonical Assistant proposal in the same logical Step,
    /// matched on both the call identity and the Tool identity, so a reused
    /// provider call id in another step cannot supply a name here.
    pub(super) fn tool_call_name(
        &self,
        anchor: &RuntimeEventEnvelope,
        call_id: &crate::runtime::identity::ToolCallId,
        tool_id: &crate::runtime::identity::ToolId,
    ) -> Result<Option<String>, ConversationStoreError> {
        Ok(self
            .step_tool_call(anchor, call_id, tool_id)?
            .map(|(_, call)| call.name))
    }
}

/// The terminal outcome facts of one request, from its own terminal event.
fn request_terminal(
    end: Option<&RuntimeEventEnvelope>,
) -> (
    Option<crate::model::error::ModelErrorKind>,
    Option<ModelUsage>,
    Option<GenerationEvidence>,
) {
    match end.map(|event| &event.event) {
        Some(E::ModelRequestCompleted {
            usage, generation, ..
        }) => (None, usage.clone(), *generation),
        Some(E::ModelRequestFailed {
            error,
            usage,
            generation,
            ..
        }) => (Some(error.kind.clone()), usage.clone(), *generation),
        _ => (None, None, None),
    }
}

/// Derives display metrics from settled evidence and this request's usage.
pub(super) fn generation_metrics(
    evidence: GenerationEvidence,
    usage: Option<&ModelUsage>,
) -> TraceGeneration {
    TraceGeneration {
        ttft_ms: evidence.time_to_first_output_ms(),
        generation_ms: evidence.generation_ms(),
        terminal_ms: evidence.terminal_ms,
        // Throughput needs both a measured decode span and reported output
        // tokens. Missing usage leaves it unavailable rather than zero.
        output_tokens_per_second: usage
            .and_then(|usage| evidence.throughput_tokens_per_second(usage.output_tokens)),
    }
}

/// The typed state proven by one terminal fact.
pub(super) fn terminal(event: &E) -> TraceState {
    match event {
        E::AttemptCompleted { .. }
        | E::TurnCompleted
        | E::ModelRequestCompleted { .. }
        | E::CompactionCompleted { .. }
        | E::WorkflowCompleted { .. } => TraceState::Completed,
        E::InteractionSettled { settlement, .. } => {
            use crate::events::interaction::InteractionSettlement as S;
            match settlement {
                S::Approved | S::Reviewed { .. } | S::QuestionnaireSubmitted { .. } => {
                    TraceState::Completed
                }
                S::Denied { .. } | S::QuestionnaireDeclined => TraceState::Denied,
                S::Cancelled { .. } => TraceState::Cancelled,
                S::DeadlineExpired { .. } => TraceState::TimedOut,
                S::ReviewInvalidated => TraceState::Interrupted,
            }
        }
        E::AttemptCancelled { .. } | E::WorkflowCancelled { .. } => TraceState::Cancelled,
        E::AttemptTimedOut { .. } => TraceState::TimedOut,
        E::AttemptLimitExceeded { .. } => TraceState::Limited,
        E::ModelRequestFailed { error, .. } => match error.kind {
            crate::model::error::ModelErrorKind::Cancelled => TraceState::Cancelled,
            crate::model::error::ModelErrorKind::Timeout => TraceState::TimedOut,
            _ => TraceState::Failed,
        },
        E::AttemptFailed { .. } | E::CompactionFailed { .. } | E::ToolExecutionFailed { .. } => {
            TraceState::Failed
        }
        E::ToolExecutionCompleted { result, .. } => tool_state(&result.status),
        E::WorkflowFailed { status, .. } => tool_state(status),
        E::SubagentTerminalPublished { state, .. } | E::SubagentTerminalSettled { state, .. } => {
            use crate::events::types::SubagentTerminalState as S;
            match state {
                S::Succeeded => TraceState::Completed,
                S::Failed => TraceState::Failed,
                S::Cancelled => TraceState::Cancelled,
                S::Interrupted => TraceState::Interrupted,
            }
        }
        E::BackgroundTerminalPublished { state, .. } => {
            use crate::events::types::BackgroundTerminalState as S;
            match state {
                S::Succeeded => TraceState::Completed,
                S::Failed => TraceState::Failed,
                S::Cancelled => TraceState::Cancelled,
                S::Denied => TraceState::Denied,
                S::TimedOut => TraceState::TimedOut,
                S::OutcomeUnknown => TraceState::OutcomeUnknown,
            }
        }
        _ => TraceState::Incomplete,
    }
}

pub(super) fn tool_state(state: &ToolExecutionStatus) -> TraceState {
    match state {
        ToolExecutionStatus::Success => TraceState::Completed,
        ToolExecutionStatus::Failed { .. } => TraceState::Failed,
        ToolExecutionStatus::Denied { .. } => TraceState::Denied,
        ToolExecutionStatus::Cancelled { .. } => TraceState::Cancelled,
        ToolExecutionStatus::TimedOut => TraceState::TimedOut,
        ToolExecutionStatus::OutcomeUnknown { .. } => TraceState::OutcomeUnknown,
    }
}

/// Enforces the per-record identity and byte bounds.
///
/// An oversized native identity is omitted whole rather than shortened,
/// because a shortened identity is a different identity that refers to
/// nothing. The record keeps its own stable Trace id either way.
pub(super) fn bound_record(record: &mut TraceRecord) {
    if record
        .location
        .attempt_id
        .as_ref()
        .is_some_and(|id| !identity_fits(id.as_str()))
    {
        record.location.attempt_id = None;
        record.truncated = true;
    }
    if record
        .location
        .step_id
        .as_ref()
        .is_some_and(|id| !identity_fits(id.as_str()))
    {
        record.location.step_id = None;
        record.truncated = true;
    }
    if record
        .native_id
        .as_ref()
        .is_some_and(|id| !identity_fits(id))
    {
        record.native_id = None;
        record.truncated = true;
    }
    if record
        .message_id
        .as_ref()
        .is_some_and(|id| !identity_fits(id.as_str()))
    {
        record.message_id = None;
        record.truncated = true;
    }
    if record.request.as_ref().is_some_and(|request| {
        !identity_fits(request.request_id.as_str())
            || !identity_fits(request.assistant_message_id.as_str())
    }) {
        record.request = None;
        record.truncated = true;
    }
    if record.tool.as_ref().is_some_and(|tool| {
        !identity_fits(tool.call_id.as_str()) || !identity_fits(tool.tool_id.as_str())
    }) {
        record.tool = None;
        record.truncated = true;
    }
    let identities = record.attachments.len() + record.calls.len();
    record
        .attachments
        .retain(|reference| identity_fits(reference.artifact_id.as_str()));
    record.calls.retain(|call| {
        identity_fits(call.call_id.as_str()) && identity_fits(call.tool_id.as_str())
    });
    record.truncated |= identities != record.attachments.len() + record.calls.len();
    if encoded_len(record) > TRACE_RECORD_BYTES {
        record.preview = None;
        record.calls.clear();
        record.truncated = true;
    }
    // JSON escaping can expand even bounded identity and model strings
    // several times over, so the bound is re-checked after each release.
    if encoded_len(record) > TRACE_RECORD_BYTES {
        record.request = None;
        record.tool = None;
        record.attachments.clear();
    }
}
