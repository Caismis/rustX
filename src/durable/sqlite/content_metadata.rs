//! Borrowed native metadata; payload trees are never copied for display indexes.
use crate::message::types::{AssistantContentBlock, ContentBlockIndex, MessageBlock};
use crate::runtime::identity::{ToolCallId, ToolId};
use serde::{Serialize, Serializer, ser::SerializeMap as _};
pub(super) struct MessageMetadata<'a>(pub &'a MessageBlock);
impl Serialize for MessageMetadata<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self.0 {
            MessageBlock::User(user) => {
                map.serialize_entry("role", "user")?;
                map.serialize_entry("id", &user.id)?;
                map.serialize_entry("source", &user.source)?;
                map.serialize_entry("kind", &user.kind)?;
                if let Some(timestamp) = user.timestamp {
                    map.serialize_entry("timestamp", &timestamp)?;
                }
            }
            MessageBlock::Assistant(assistant) => {
                map.serialize_entry("role", "assistant")?;
                map.serialize_entry("id", &assistant.id)?;
            }
            MessageBlock::Tool(tool) => {
                map.serialize_entry("role", "tool")?;
                map.serialize_entry("id", &tool.id)?;
                map.serialize_entry("occurrence", &tool.occurrence)?;
                map.serialize_entry("tool_call_id", &tool.tool_call_id)?;
                map.serialize_entry("tool_id", &tool.tool_id)?;
                map.serialize_entry("result", &ResultMetadata(&tool.result))?;
            }
        }
        map.end()
    }
}
struct ResultMetadata<'a>(&'a crate::tools::types::ToolExecutionResult);
impl Serialize for ResultMetadata<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let result = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("status", &result.status)?;
        map.serialize_entry("duration_ms", &result.duration_ms)?;
        map.serialize_entry("artifacts", &result.artifacts)?;
        if let Some(value) = &result.workflow {
            map.serialize_entry("workflow", value)?;
        }
        if let Some(value) = &result.exit_code {
            map.serialize_entry("exit_code", value)?;
        }
        if let Some(value) = &result.truncation {
            map.serialize_entry("truncation", value)?;
        }
        if let Some(value) = &result.managed_output {
            map.serialize_entry("managed_output", value)?;
        }
        if !result.deliveries.is_empty() {
            map.serialize_entry("deliveries", &result.deliveries)?;
        }
        map.end()
    }
}
pub(super) struct AssistantMetadata<'a>(pub &'a AssistantContentBlock);
impl Serialize for AssistantMetadata<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        match self.0 {
            AssistantContentBlock::Text(_) => map.serialize_entry("type", "text")?,
            AssistantContentBlock::Refusal(_) => map.serialize_entry("type", "refusal")?,
            AssistantContentBlock::Image(_) => map.serialize_entry("type", "image")?,
            AssistantContentBlock::Reasoning(reasoning) => {
                map.serialize_entry("type", "reasoning")?;
                if let Some(state) = &reasoning.provider_state {
                    map.serialize_entry("provider_state", state)?;
                }
            }
            AssistantContentBlock::ToolCall(call) => {
                map.serialize_entry("type", "tool_call")?;
                map.serialize_entry("id", &call.id)?;
                map.serialize_entry("tool_id", &call.tool_id)?;
                map.serialize_entry("name", &call.name)?;
            }
        }
        map.end()
    }
}
#[derive(Serialize)]
pub(super) struct PublicationMetadata<'a> {
    pub id: &'a ToolCallId,
    pub tool_id: &'a ToolId,
    pub name: &'a str,
}
impl PublicationMetadata<'_> {
    pub(super) fn tagged(&self) -> TaggedPublication<'_> {
        TaggedPublication {
            kind: "tool_call",
            metadata: self,
        }
    }
}
#[derive(Serialize)]
pub(super) struct TaggedPublication<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    metadata: &'a PublicationMetadata<'a>,
}
#[derive(Serialize)]
pub(super) struct AuditMetadata<'a> {
    pub block_index: &'a ContentBlockIndex,
    pub call_id: &'a ToolCallId,
    pub tool_id: &'a ToolId,
    pub name: &'a str,
    pub complete: &'a bool,
}
impl AuditMetadata<'_> {
    pub(super) fn tagged(&self) -> TaggedAudit<'_> {
        TaggedAudit {
            kind: "proposed_tool_call",
            metadata: self,
        }
    }
}
#[derive(Serialize)]
pub(super) struct TaggedAudit<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    metadata: &'a AuditMetadata<'a>,
}
