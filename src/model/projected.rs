//! Identity-free typed model context. Projection content is never an execution fact.
use crate::message::types::{
    AssistantContentBlock, InboundKind, MessageBlock, UserContentBlock, UserSource,
};
use crate::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
use crate::tools::types::ToolExecutionResult;
use serde::{Deserialize, Serialize};

/// Immutable source scope, distinct from a projected occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanonicalMessageRef {
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    /// Integrity of the complete immutable body, independent of semantic request hashing.
    pub content_digest: String,
}

impl CanonicalMessageRef {
    /// Capture identity and immutable content integrity at the projection boundary.
    /// # Panics
    /// Panics only if the canonical body cannot serialize.
    #[must_use]
    pub fn of(conversation_id: &ConversationId, message: &MessageBlock) -> Self {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        serde_json::to_writer(DigestWriter(&mut digest), message)
            .expect("canonical body serializes");
        Self {
            conversation_id: conversation_id.clone(),
            message_id: message.id().clone(),
            content_digest: format!("{:x}", digest.finalize()),
        }
    }
    /// Verify the exact immutable dependency, including provenance metadata.
    #[must_use]
    pub fn matches(&self, message: &MessageBlock) -> bool {
        Self::of(&self.conversation_id, message) == *self
    }
}
pub(crate) struct DigestWriter<'a>(pub(crate) &'a mut sha2::Sha256);
impl std::io::Write for DigestWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        use sha2::Digest;
        self.0.update(bytes);
        #[cfg(test)]
        super::selected_input::record_hash_work(bytes.len(), 0);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// User classification in a projection. Summary context exists only here,
/// never in the canonical `InboundKind` or the inbound admission protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectedUserKind {
    Message,
    GoalContinuation(crate::goal::GoalRef),
    Context(crate::message::types::ContextKind),
    CompactionContext,
}
impl From<&InboundKind> for ProjectedUserKind {
    fn from(kind: &InboundKind) -> Self {
        match kind {
            InboundKind::Message => Self::Message,
            InboundKind::Context(context) => Self::Context(context.clone()),
            InboundKind::GoalContinuation(goal) => Self::GoalContinuation(goal.clone()),
        }
    }
}

/// Typed content without a canonical message identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectedMessage {
    User {
        content: Vec<UserContentBlock>,
        source: UserSource,
        kind: ProjectedUserKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timestamp: Option<chrono::DateTime<chrono::Utc>>,
    },
    Assistant {
        content: Vec<AssistantContentBlock>,
    },
    Tool {
        tool_call_id: ToolCallId,
        tool_id: ToolId,
        result: ToolExecutionResult,
    },
}
impl From<&MessageBlock> for ProjectedMessage {
    fn from(message: &MessageBlock) -> Self {
        match message {
            MessageBlock::User(m) => Self::User {
                content: m.content.clone(),
                source: m.source.clone(),
                kind: ProjectedUserKind::from(&m.kind),
                timestamp: m.timestamp,
            },
            MessageBlock::Assistant(m) => Self::Assistant {
                content: m.content.clone(),
            },
            MessageBlock::Tool(m) => Self::Tool {
                tool_call_id: m.tool_call_id.clone(),
                tool_id: m.tool_id.clone(),
                result: m.result.clone(),
            },
        }
    }
}

/// Semantic transformation provenance. Sources are evidence, never identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivedModelMessage {
    pub message: ProjectedMessage,
    pub sources: Vec<CanonicalMessageRef>,
    pub checkpoint_sources: Vec<CheckpointSourceRef>,
    pub transformations: Vec<SemanticTransformation>,
}
/// Scoped immutable projection provenance, used when a summary consumes an earlier summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CheckpointSourceRef {
    pub conversation_id: ConversationId,
    pub checkpoint_id: crate::runtime::identity::CheckpointId,
    pub content_digest: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SemanticTransformation {
    Summary,
    ImageDowngrade,
    UploadRendering,
}

/// Borrowed role/content shared by Original and Derived representations.
#[derive(serde::Serialize)]
#[serde(tag = "role", content = "content", rename_all = "snake_case")]
pub enum ModelMessageView<'a> {
    User(&'a [UserContentBlock]),
    Assistant(&'a [AssistantContentBlock]),
    Tool(&'a ToolCallId, &'a ToolExecutionResult),
    RequestOnly(&'a super::input::RequestOnlyModelContext),
}

/// Exact frozen request source: immutable Originals are keyed, Derived values are owned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "input_kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrozenModelInput {
    Original(CanonicalMessageRef),
    Derived(DerivedModelMessage),
    RequestOnly(super::input::RequestOnlyModelContext),
}

impl<'a> From<&'a MessageBlock> for ModelMessageView<'a> {
    fn from(message: &'a MessageBlock) -> Self {
        match message {
            MessageBlock::User(m) => Self::User(&m.content),
            MessageBlock::Assistant(m) => Self::Assistant(&m.content),
            MessageBlock::Tool(m) => Self::Tool(&m.tool_call_id, &m.result),
        }
    }
}

/// Model-visible role/content, with the authoritative bounded Tool projection.
/// Runtime execution metadata and projection provenance never enter this view.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum ModelVisibleMessage<'a> {
    Borrowed(ModelMessageView<'a>),
    Tool {
        role: &'static str,
        content: (&'a ToolCallId, crate::tools::types::ModelToolResultContent),
    },
}
impl<'a> ModelMessageView<'a> {
    pub(crate) fn model_visible(self) -> Result<ModelVisibleMessage<'a>, &'static str> {
        match self {
            Self::Tool(id, result) => Ok(ModelVisibleMessage::Tool {
                role: "tool",
                content: (id, result.model_visible_content()?),
            }),
            other => Ok(ModelVisibleMessage::Borrowed(other)),
        }
    }
}
