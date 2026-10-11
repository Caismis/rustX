//! Exact Tool-occurrence navigation across the complete native Journal.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{TRACE_PAGE_LIMIT, TraceCursor, TracePage, TraceProjection};
use crate::durable::{ConversationStoreError, presentation::FactScope};
use crate::events::types::RuntimeEvent;
use crate::message::types::{AssistantContentBlock, MessageBlock, ToolCallOccurrenceRef};
use crate::runtime::identity::{ToolCallId, ToolId};

/// The occurrence, rather than a globally ambiguous provider call ID, owns navigation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolLocator {
    pub occurrence: ToolCallOccurrenceRef,
    pub call_id: ToolCallId,
    pub tool_id: ToolId,
}

/// A contiguous native history window ending at the selected execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolLocation {
    pub record_id: String,
    pub page: TracePage,
}

impl TraceProjection<'_> {
    /// Resolves a canonical occurrence at this finite cut, independently of loaded pages.
    /// A proposal without a native start (or inherited history without local
    /// execution facts) returns None; neighbours and repeated IDs never stand in.
    /// # Errors
    /// Invalid identities and durable read failures are explicit errors.
    pub fn locate_tool(
        &self,
        locator: &TraceToolLocator,
    ) -> Result<Option<TraceToolLocation>, ConversationStoreError> {
        if !super::bounds::identity_fits(locator.occurrence.assistant_message_id.as_str())
            || !super::bounds::identity_fits(locator.call_id.as_str())
            || !super::bounds::identity_fits(locator.tool_id.as_str())
        {
            return Err(ConversationStoreError::InvalidReference(
                "Tool occurrence identity is too long".into(),
            ));
        }
        let messages = self.store.load_messages(std::slice::from_ref(
            &locator.occurrence.assistant_message_id,
        ))?;
        let Some(MessageBlock::Assistant(assistant)) = messages.first() else {
            return Ok(None);
        };
        let Some(AssistantContentBlock::ToolCall(call)) = assistant
            .content
            .get(locator.occurrence.block_index.get() as usize)
        else {
            return Ok(None);
        };
        if call.id != locator.call_id || call.tool_id != locator.tool_id {
            return Ok(None);
        }
        // Find this canonical commit across the finite Journal, then use its
        // exact native Step to query the execution index. Never enumerate all
        // executions merely because a provider reused the same call ID.
        let Some(commit) = self
            .facts(
                FactScope::Message(assistant.id.to_string()),
                &["assistant_message_committed"],
                None,
                1,
            )?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let candidates = self.facts(
            FactScope::ToolCall {
                call_id: locator.call_id.to_string(),
                attempt: commit.attempt_id.clone(),
                turn: commit.turn_id.clone(),
            },
            &["tool_execution_started"],
            None,
            1,
        )?;
        for anchor in candidates {
            if anchor.sequence > commit.sequence
                && let RuntimeEvent::ToolExecutionStarted {
                    tool_call_id,
                    tool_id,
                } = &anchor.event
                && tool_call_id == &locator.call_id
                && tool_id == &locator.tool_id
                && let Some((message_id, _)) =
                    self.step_tool_call(&anchor, tool_call_id, tool_id)?
                && message_id == assistant.id
            {
                let cursor = TraceCursor::at(anchor.sequence);
                let end = TraceCursor::at(anchor.sequence.saturating_add(1));
                return Ok(Some(TraceToolLocation {
                    record_id: cursor.0,
                    page: self.page(Some(&end), TRACE_PAGE_LIMIT)?,
                }));
            }
        }
        Ok(None)
    }
}
