//! The native and durable facts one anchor owns, before any presentation.
//!
//! Trace has two read responsibilities, and they are deliberately different:
//!
//! ```text
//! summary    immutable historical presentation, resolved once per page
//! lifecycle  mutable lifecycle repair of records a client already holds
//! ```
//!
//! Both start from the same anchor, so the facts an anchor owns *by itself*
//! — its native identity, its own grouping, its own durable terminal, and
//! the canonical content it already had to read to know its own outcome —
//! are resolved here, once, and shared.
//!
//! Nothing resolved here is a relationship between records. The System
//! Prompt predecessor, the canonical Context a request introduced and the
//! recorded model-facing Tool name are historical presentation joins: they
//! belong to [`super::record`], they are resolved only when a summary page
//! is built, and a lifecycle refresh never performs them. That separation is
//! the point of this module. A lifecycle refresh may cover up to
//! [`super::TRACE_RECORD_LIMIT`] records, and the availability of ordinary
//! lifecycle repair must not depend on immutable joins whose result a
//! [`super::TraceLifecycle`] does not even carry.

use super::bounds::TracePreview;
use super::content::{message_preview, tool_outcome, tool_status_detail};
use super::types::{
    TraceArtifact, TraceCursor, TraceGeneration, TraceKind, TraceLocation, TraceState, TraceTiming,
    TraceToolCall, TraceToolSummary,
};
use super::{ADOPTED_MESSAGE_LIMIT, TraceProjection};
use crate::durable::ConversationStoreError;
use crate::durable::presentation::FactScope;
use crate::events::types::{RuntimeEvent as E, RuntimeEventEnvelope};
use crate::message::types::{AssistantContentBlock, MessageBlock};
use crate::model::error::ModelErrorKind;
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{MessageId, ToolCallId};

use super::bounds::identity_fits;
use super::record::{generation_metrics, terminal};

/// One anchor's native identity, own grouping and own durable lifecycle.
///
/// `preview`, `calls` and `has_detail` are summary vocabulary, but they are
/// derived here when — and only when — the anchor already had to load the
/// canonical message for a lifecycle fact of its own, so keeping them costs
/// a lifecycle refresh nothing. Every preview that would need a read of its
/// own is left to the summary projection.
pub(super) struct AnchorFacts {
    pub id: String,
    pub position: TraceCursor,
    pub location: TraceLocation,
    pub kind: TraceKind,
    pub state: TraceState,
    pub timing: TraceTiming,
    pub preview: Option<TracePreview>,
    pub calls: Vec<TraceToolCall>,
    pub native_id: Option<String>,
    pub agent_id: Option<crate::runtime::identity::AgentId>,
    pub activation_id: Option<crate::runtime::identity::SubagentId>,
    pub originating_tool_call_id: Option<ToolCallId>,
    pub message_id: Option<MessageId>,
    pub attachments: Vec<TraceArtifact>,
    pub tool: Option<TraceToolSummary>,
    pub request: Option<AnchorRequest>,
    pub has_detail: bool,
    pub truncated: bool,
}

/// One request anchor's own immutable snapshot and own terminal outcome.
///
/// The snapshot is loaded once. Lifecycle repair needs the frozen
/// provisional Assistant identity and the frozen grouping from it; the
/// summary projection additionally reads the historical model and ordinal.
/// Neither reads any *other* request's snapshot — that join belongs to the
/// System Prompt presentation and happens only for a summary.
pub(super) struct AnchorRequest {
    pub frozen: RequestSnapshot,
    pub failure_kind: Option<ModelErrorKind>,
    pub usage: Option<ModelUsage>,
    pub generation: Option<TraceGeneration>,
}

impl TraceProjection<'_> {
    /// Resolves the facts one anchor owns from its own native authorities.
    ///
    /// # Errors
    ///
    /// Propagates the failed durable read; an unreadable authority is never
    /// interpreted as an absent fact.
    #[allow(clippy::too_many_lines)] // One closed anchor vocabulary, one place.
    pub(super) fn anchor_facts(
        &self,
        anchor: &RuntimeEventEnvelope,
    ) -> Result<AnchorFacts, ConversationStoreError> {
        let mut facts = AnchorFacts {
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
            calls: vec![],
            native_id: None,
            agent_id: None,
            activation_id: None,
            originating_tool_call_id: None,
            message_id: None,
            attachments: vec![],
            tool: None,
            request: None,
            has_detail: false,
            truncated: false,
        };
        let ending = match &anchor.event {
            E::AttemptStarted { attempt_id } => {
                facts.kind = TraceKind::Attempt;
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
                // therefore no duration. Its identities and its overflow are
                // both in the anchor itself, so no message is loaded for a
                // lifecycle fact — the preview that needs one belongs to the
                // summary projection.
                facts.kind = TraceKind::User;
                facts.state = TraceState::Completed;
                facts.has_detail = !message_ids.is_empty();
                facts.message_id = message_ids.first().cloned();
                facts.truncated = message_ids.len() > ADOPTED_MESSAGE_LIMIT;
                None
            }
            E::ModelRequestStarted { request_id, .. } => {
                facts.kind = TraceKind::Request;
                facts.has_detail = true;
                let frozen = self.store.load_request_snapshot(request_id)?;
                // The frozen snapshot owns this request's grouping. The
                // anchor's own correlation columns agree, but the snapshot is
                // the authority the ordinal is read from.
                facts.location = TraceLocation {
                    attempt_id: Some(frozen.identity.attempt_id.clone()),
                    step_id: Some(frozen.identity.turn.clone()),
                };
                let end = self.ending(
                    FactScope::Request(request_id.to_string()),
                    &["model_request_completed", "model_request_failed"],
                )?;
                let (failure_kind, usage, generation) = request_terminal(end.as_ref());
                facts.preview = Some(TracePreview::of(&frozen.invocation.model));
                facts.request = Some(AnchorRequest {
                    generation: generation
                        .map(|evidence| generation_metrics(evidence, usage.as_ref())),
                    frozen,
                    failure_kind,
                    usage,
                });
                end
            }
            E::AssistantMessageCommitted { message_id } => {
                facts.kind = TraceKind::Assistant;
                // Acceptance is the canonical commit itself; the record is
                // complete at the instant it exists.
                facts.state = TraceState::Completed;
                facts.has_detail = true;
                facts.message_id = Some(message_id.clone());
                for message in self.store.load_messages(std::slice::from_ref(message_id))? {
                    facts.preview = message_preview(&message);
                    if let MessageBlock::Assistant(assistant) = message {
                        for block in &assistant.content {
                            match block {
                                AssistantContentBlock::ToolCall(call)
                                    if identity_fits(call.id.as_str())
                                        && identity_fits(call.tool_id.as_str()) =>
                                {
                                    facts.calls.push(TraceToolCall {
                                        call_id: call.id.clone(),
                                        tool_id: call.tool_id.clone(),
                                        name: call.name.clone(),
                                    });
                                }
                                AssistantContentBlock::Image(image)
                                    if identity_fits(image.artifact_id.as_str()) =>
                                {
                                    facts.attachments.push(TraceArtifact {
                                        artifact_id: image.artifact_id.clone(),
                                        image: true,
                                        name: None,
                                        mime_type: None,
                                    });
                                }
                                AssistantContentBlock::ToolCall(_)
                                | AssistantContentBlock::Image(_) => {
                                    facts.truncated = true;
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
                facts.kind = TraceKind::Tool;
                facts.has_detail = true;
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
                    // The recorded model-facing name is a historical
                    // presentation join over the Step's Assistant proposals.
                    // A lifecycle update does not carry it, so it is resolved
                    // only when a summary row is built.
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
                            facts.message_id = Some(message_id.clone());
                            summary.outcome = Some(tool_outcome(&tool.result.status));
                            summary.detail = tool_status_detail(&tool.result.status)
                                .as_deref()
                                .map(TracePreview::of);
                            facts.preview = message_preview(&message);
                            let (attachments, truncated) =
                                super::content::tool_result_artifacts(&tool.result);
                            facts.attachments = attachments;
                            facts.truncated |= truncated;
                        }
                    }
                }
                facts.tool = Some(summary);
                end
            }
            E::CompactionStarted => {
                facts.kind = TraceKind::Compaction;
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
                    // The summary's identity is in the terminal fact itself.
                    // Its preview needs a Ledger read, so it is left to the
                    // summary projection.
                    facts.has_detail = true;
                    facts.message_id = Some(summary_message_id.clone());
                }
                end
            }
            E::BackgroundExecutionCommitted {
                execution_id,
                tool_call_id,
                ..
            } => {
                facts.kind = TraceKind::Background;
                facts.native_id = Some(execution_id.to_string());
                facts.originating_tool_call_id = Some(tool_call_id.clone());
                self.ending(
                    FactScope::Execution(execution_id.to_string()),
                    &["background_terminal_published"],
                )?
            }
            E::SubagentOwnershipCommitted {
                subagent_id,
                child_agent_id,
                agent,
                tool_call_id,
                ..
            } => {
                facts.kind = TraceKind::Subagent;
                facts.native_id = Some(subagent_id.to_string());
                facts.agent_id = Some(child_agent_id.clone());
                facts.activation_id = Some(subagent_id.clone());
                facts.originating_tool_call_id = Some(tool_call_id.clone());
                facts.preview = Some(TracePreview::of(agent.as_str()));
                self.ending(
                    FactScope::Subagent(subagent_id.to_string()),
                    &["subagent_terminal_published", "subagent_terminal_settled"],
                )?
            }
            E::WorkflowStarted {
                run_id,
                tool_call_id,
                ..
            } => {
                facts.kind = TraceKind::Workflow;
                facts.originating_tool_call_id = Some(tool_call_id.clone());
                let id = serde_json::to_string(run_id).map_err(|_| {
                    ConversationStoreError::InvalidReference("invalid Workflow identity".into())
                })?;
                facts.native_id = Some(id.clone());
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
                facts.kind = TraceKind::Interaction;
                facts.native_id = Some(interaction_id.to_string());
                self.ending(
                    FactScope::Interaction(interaction_id.to_string()),
                    &["interaction_settled"],
                )?
            }
            _ => unreachable!("allowlisted anchors only"),
        };
        if let Some(end) = ending.filter(|end| end.sequence > anchor.sequence) {
            facts.state = terminal(&end.event);
            facts.timing.ended_at = Some(end.timestamp);
            // Both endpoints exist, so the duration is measured rather than
            // assumed. A record with one endpoint keeps none.
            facts.timing.duration_ms =
                u64::try_from((end.timestamp - anchor.timestamp).num_milliseconds()).ok();
        }
        Ok(facts)
    }
}

/// The terminal outcome facts of one request, from its own terminal event.
fn request_terminal(
    end: Option<&RuntimeEventEnvelope>,
) -> (
    Option<ModelErrorKind>,
    Option<ModelUsage>,
    Option<crate::model::generation_evidence::GenerationEvidence>,
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
