//! Heavy inspection detail for one selected record.
//!
//! Detail is fetched on demand, so nothing here is bounded by paging cost —
//! only by the explicit detail bounds. It is still a pure read: resolving
//! detail loads immutable snapshots and canonical messages and changes no
//! native state whatsoever.

use super::bounds::identity_fits;
use super::content::{assistant_blocks, tool_result_block, user_blocks, user_source_label};
use super::record::generation_metrics;
use super::request::{RequestOutcome, request_detail};
use super::tool::{historical_definition, tool_detail};
use super::types::{
    TraceDetail, TraceKind, TraceMessageDetail, TraceMessageRole, TraceRequestFailure,
};
use super::{ADOPTED_MESSAGE_LIMIT, STEP_JOIN_LIMIT, TraceProjection};
use crate::durable::ConversationStoreError;
use crate::durable::presentation::FactScope;
use crate::events::types::{RuntimeEvent as E, RuntimeEventEnvelope};
use crate::message::types::{AssistantContentBlock, MessageBlock};
use crate::model::snapshot::RequestSnapshot;
use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
use crate::tools::types::ToolCall;

impl TraceProjection<'_> {
    /// Builds the detail of one resolved anchor.
    pub(super) fn build_detail(
        &self,
        anchor: &RuntimeEventEnvelope,
    ) -> Result<TraceDetail, ConversationStoreError> {
        let mut detail = TraceDetail {
            id: format!("trace:{}", anchor.sequence),
            kind: TraceKind::Step,
            request: None,
            tool: None,
            messages: Vec::new(),
            truncated: false,
        };
        match &anchor.event {
            E::ModelRequestStarted { request_id, .. } => {
                detail.kind = TraceKind::Request;
                let frozen = self.store.load_request_snapshot(request_id)?;
                let end = self.ending(
                    FactScope::Request(request_id.to_string()),
                    &["model_request_completed", "model_request_failed"],
                )?;
                detail.request =
                    request_detail(self.store, &frozen, request_outcome(end.as_ref()))?;
                detail.truncated = detail.request.is_none();
            }
            E::InboundTurnAdopted { message_ids } => {
                detail.kind = TraceKind::User;
                detail.truncated = message_ids.len() > ADOPTED_MESSAGE_LIMIT;
                // One adoption transaction can contain several canonical
                // messages. Preserve the batch order and each identity.
                for message in self
                    .store
                    .load_messages(&message_ids[..message_ids.len().min(ADOPTED_MESSAGE_LIMIT)])?
                {
                    if let Some(message) = message_detail(&message) {
                        detail.messages.push(message);
                    } else {
                        detail.truncated = true;
                    }
                }
            }
            E::AssistantMessageCommitted { message_id } => {
                detail.kind = TraceKind::Assistant;
                if let Some(message) = self
                    .store
                    .load_messages(std::slice::from_ref(message_id))?
                    .first()
                {
                    if let Some(message) = message_detail(message) {
                        detail.messages.push(message);
                    } else {
                        detail.truncated = true;
                    }
                }
            }
            E::CompactionStarted => {
                detail.kind = TraceKind::Compaction;
                if let Some(event) = self
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
                    && let E::CompactionCompleted {
                        summary_message_id, ..
                    } = &event.event
                    && let Some(message) = self
                        .store
                        .load_messages(std::slice::from_ref(summary_message_id))?
                        .first()
                {
                    if let Some(message) = message_detail(message) {
                        detail.messages.push(message);
                    } else {
                        detail.truncated = true;
                    }
                }
            }
            E::ToolExecutionStarted {
                tool_call_id,
                tool_id,
            } => {
                detail.kind = TraceKind::Tool;
                detail.tool = self.tool_call_detail(anchor, tool_call_id, tool_id, true)?;
                detail.truncated = detail.tool.is_none();
            }
            E::AttemptStarted { .. } => detail.kind = TraceKind::Attempt,
            E::TurnStarted => detail.kind = TraceKind::Step,
            E::BackgroundExecutionCommitted { .. } => detail.kind = TraceKind::Background,
            E::SubagentOwnershipCommitted { .. } => detail.kind = TraceKind::Subagent,
            E::WorkflowStarted { .. } => detail.kind = TraceKind::Workflow,
            E::InteractionRequested { .. } => detail.kind = TraceKind::Interaction,
            _ => unreachable!("allowlisted anchors only"),
        }
        Ok(detail)
    }

    /// Assembles Tool detail from its three separate native authorities.
    pub(super) fn tool_call_detail(
        &self,
        anchor: &RuntimeEventEnvelope,
        call_id: &ToolCallId,
        tool_id: &ToolId,
        started: bool,
    ) -> Result<Option<super::types::TraceToolDetail>, ConversationStoreError> {
        let proposal = self.step_tool_call(anchor, call_id, tool_id)?;
        // The historical schema belongs to the request that carried the
        // proposal, not to the current capability set. Resolving it through
        // the proposing Assistant message keeps that ownership exact.
        let definition = match proposal.as_ref() {
            Some((message_id, _)) => self
                .owning_request_snapshot(anchor, message_id)?
                .and_then(|snapshot| historical_definition(&snapshot, tool_id)),
            None => None,
        };
        let message = self.canonical_tool_message(anchor, call_id, tool_id)?;
        Ok(tool_detail(
            call_id,
            tool_id,
            proposal.as_ref().map(|(_, call)| call),
            definition,
            started,
            message.as_ref(),
        ))
    }

    /// The canonical `ToolCall` proposal of this exact call, if it is loadable.
    ///
    /// Assistant commits inside the same logical Step are the only candidates,
    /// and a candidate must match both the call identity and the Tool
    /// identity. A provider call id reused in another Step therefore cannot
    /// supply arguments for this one.
    pub(super) fn step_tool_call(
        &self,
        anchor: &RuntimeEventEnvelope,
        call_id: &ToolCallId,
        tool_id: &ToolId,
    ) -> Result<Option<(MessageId, ToolCall)>, ConversationStoreError> {
        let (Some(attempt), Some(turn)) = (&anchor.attempt_id, &anchor.turn_id) else {
            return Ok(None);
        };
        let commits = self.facts(
            FactScope::Step(attempt.clone(), turn.clone()),
            &["assistant_message_committed"],
            None,
            STEP_JOIN_LIMIT,
        )?;
        let ids: Vec<MessageId> = commits
            .iter()
            .filter_map(|event| match &event.event {
                E::AssistantMessageCommitted { message_id } => Some(message_id.clone()),
                _ => None,
            })
            .collect();
        for message in self.store.load_messages(&ids)? {
            let MessageBlock::Assistant(assistant) = &message else {
                continue;
            };
            for block in &assistant.content {
                if let AssistantContentBlock::ToolCall(call) = block
                    && call.id == *call_id
                    && call.tool_id == *tool_id
                {
                    return Ok(Some((assistant.id.clone(), call.clone())));
                }
            }
        }
        Ok(None)
    }

    /// The immutable snapshot of the request whose generation produced one
    /// Assistant message.
    ///
    /// Matched on the snapshot's own frozen provisional message identity, so
    /// the request found is exactly the one that generated that message
    /// rather than the newest request of the Step.
    fn owning_request_snapshot(
        &self,
        anchor: &RuntimeEventEnvelope,
        assistant_message_id: &MessageId,
    ) -> Result<Option<RequestSnapshot>, ConversationStoreError> {
        let (Some(attempt), Some(turn)) = (&anchor.attempt_id, &anchor.turn_id) else {
            return Ok(None);
        };
        let starts = self.facts(
            FactScope::Step(attempt.clone(), turn.clone()),
            &["model_request_started"],
            None,
            STEP_JOIN_LIMIT,
        )?;
        for start in starts {
            let E::ModelRequestStarted { request_id, .. } = &start.event else {
                continue;
            };
            let snapshot = self.store.load_request_snapshot(request_id)?;
            if snapshot.provisional_message_id == *assistant_message_id {
                return Ok(Some(snapshot));
            }
        }
        Ok(None)
    }

    /// The canonical `ToolMessage` of one call, from the Message Ledger.
    fn canonical_tool_message(
        &self,
        anchor: &RuntimeEventEnvelope,
        call_id: &ToolCallId,
        tool_id: &ToolId,
    ) -> Result<Option<crate::message::types::ToolMessageBlock>, ConversationStoreError> {
        let Some(commit) = self.ending(
            FactScope::ToolCall {
                call_id: call_id.to_string(),
                attempt: anchor.attempt_id.clone(),
                turn: anchor.turn_id.clone(),
            },
            &["tool_message_committed"],
        )?
        else {
            return Ok(None);
        };
        let E::ToolMessageCommitted { message_id, .. } = &commit.event else {
            return Ok(None);
        };
        Ok(self
            .store
            .load_messages(std::slice::from_ref(message_id))?
            .into_iter()
            .find_map(|message| match message {
                MessageBlock::Tool(tool)
                    if tool.tool_call_id == *call_id && tool.tool_id == *tool_id =>
                {
                    Some(tool)
                }
                _ => None,
            }))
    }
}

/// The terminal outcome facts of one request, read from its own terminal.
fn request_outcome(end: Option<&RuntimeEventEnvelope>) -> RequestOutcome {
    match end.map(|event| &event.event) {
        Some(E::ModelRequestCompleted {
            usage, generation, ..
        }) => RequestOutcome {
            usage: usage.clone(),
            failure: None,
            generation: generation.map(|evidence| generation_metrics(evidence, usage.as_ref())),
        },
        Some(E::ModelRequestFailed {
            error,
            usage,
            generation,
            ..
        }) => RequestOutcome {
            usage: usage.clone(),
            failure: Some(TraceRequestFailure {
                kind: error.kind.clone(),
                message: super::bounds::TraceText::detail(&error.message),
            }),
            generation: generation.map(|evidence| generation_metrics(evidence, usage.as_ref())),
        },
        // No terminal fact yet. An in-flight request has no outcome, and
        // absence is not a success.
        _ => RequestOutcome {
            usage: None,
            failure: None,
            generation: None,
        },
    }
}

/// Projects one canonical message for inspection.
fn message_detail(message: &MessageBlock) -> Option<TraceMessageDetail> {
    if !identity_fits(message.id().as_str()) {
        return None;
    }
    let (role, source, blocks, truncated) = match message {
        MessageBlock::User(user) => {
            let (blocks, truncated) = user_blocks(&user.content);
            (
                TraceMessageRole::User,
                Some(user_source_label(&user.source).to_owned()),
                blocks,
                truncated,
            )
        }
        MessageBlock::Assistant(assistant) => {
            let (blocks, truncated) = assistant_blocks(&assistant.content);
            (TraceMessageRole::Assistant, None, blocks, truncated)
        }
        MessageBlock::Tool(tool) => {
            let block = tool_result_block(tool);
            let truncated = block.is_none();
            (
                TraceMessageRole::Tool,
                None,
                block.into_iter().collect(),
                truncated,
            )
        }
    };
    Some(TraceMessageDetail {
        message_id: message.id().clone(),
        role,
        source,
        blocks,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::types::{ContentBlockIndex, ToolCallOccurrenceRef, ToolMessageBlock};
    use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};

    #[test]
    fn canonical_tool_message_omits_a_result_with_either_oversized_correlation_identity() {
        let sentinel = "opaque-identity-".repeat(super::super::bounds::TRACE_IDENTITY_BYTES);
        for oversized in [None, Some("call"), Some("tool")] {
            let message = MessageBlock::Tool(ToolMessageBlock {
                id: MessageId::new("result"),
                occurrence: ToolCallOccurrenceRef::new(
                    MessageId::new("owner"),
                    ContentBlockIndex::new(0),
                ),
                tool_call_id: ToolCallId::new(if oversized == Some("call") {
                    &sentinel
                } else {
                    "call"
                }),
                tool_id: ToolId::new(if oversized == Some("tool") {
                    &sentinel
                } else {
                    "tool"
                }),
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Success,
                    content: vec![],
                    duration_ms: 1,
                    exit_code: None,
                    artifacts: vec![],
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            });
            let detail = message_detail(&message).unwrap();
            assert_eq!(detail.truncated, oversized.is_some());
            assert_eq!(detail.blocks.len(), usize::from(oversized.is_none()));
            assert!(!serde_json::to_string(&detail).unwrap().contains(&sentinel));
        }
    }
}
