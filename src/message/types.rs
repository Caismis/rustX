//! The canonical conversation model.
//!
//! The canonical conversation contains exactly four top-level message roles:
//! [`MessageBlock::System`], [`MessageBlock::User`], [`MessageBlock::Agent`],
//! and [`MessageBlock::Tool`]. These four semantics are frozen; provider
//! roles such as `OpenAI`'s `developer` are adapter concerns, not canonical
//! roles, and are never mapped to a fifth role.
//!
//! Role and provenance are separate: a `UserMessageBlock` means inbound
//! information supplied to the current agent, regardless of whether a human,
//! another agent, the fleet, an external system, or the runtime produced it.
//! Streaming deltas are `ModelEvent` facts and never become message blocks;
//! only completed generations are committed as `AgentMessageBlock` values.

use serde::{Deserialize, Serialize};

use crate::message::content::{FileReference, ImageReference, TextBlock};
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::{AgentId, MessageId, ToolCallId, ToolId};
use crate::tools::types::{ToolCall, ToolExecutionResult};

/// The canonical conversation message.
///
/// The `role` discriminator is stable: `system`, `user`, `agent`, `tool`.
/// No additional top-level role exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum MessageBlock {
    /// Trusted system/runtime instructions or context.
    System(SystemMessageBlock),
    /// Inbound information supplied to the current agent.
    User(UserMessageBlock),
    /// One completed model generation produced by the current agent.
    Agent(AgentMessageBlock),
    /// The result of one tool call produced by the current agent.
    Tool(ToolMessageBlock),
}

/// A stable index identifying one content block within the ordered content
/// list of the canonical message being assembled.
///
/// Streaming facts (text deltas, reasoning deltas, provider continuation
/// state, tool-call content) reference the block they belong to by this
/// index, so interleaved and multiple blocks remain unambiguous without
/// exposing any provider-specific block id type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentBlockIndex(u32);

impl ContentBlockIndex {
    /// Creates an index from a raw value.
    #[must_use]
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    /// Returns the raw index value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl core::fmt::Display for ContentBlockIndex {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Trusted system/runtime instructions or context.
///
/// The authority records who supplied the trusted content, as a typed
/// category rather than an arbitrary unvalidated string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemMessageBlock {
    /// Durable message identity.
    pub id: MessageId,
    /// Who supplied the trusted content.
    pub authority: SystemAuthority,
    /// The trusted instruction or context text.
    pub content: Vec<TextBlock>,
}

/// Who supplied a `SystemMessageBlock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemAuthority {
    /// The platform/product.
    Platform,
    /// The agent itself.
    Agent,
    /// The conversation runtime.
    Runtime,
    /// A bound skill.
    Skill,
    /// The fleet/control plane.
    Fleet,
}

/// Inbound information supplied to the current agent.
///
/// A `UserMessageBlock` does not necessarily mean a human spoke: it is the
/// canonical home for anything inbound, including messages from other agents
/// (with [`UserSource::Agent`] provenance) and, in the future, runtime
/// compaction summaries (with [`InboundKind::CompactionSummary`] kind). It
/// must never become `AgentMessageBlock` or `ToolMessageBlock`, which are
/// reserved for output and actions of the current agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessageBlock {
    /// Durable message identity.
    pub id: MessageId,
    /// The inbound content.
    pub content: Vec<UserContentBlock>,
    /// Provenance: who supplied the inbound information.
    pub source: UserSource,
    /// Typed kind of inbound information.
    #[serde(default)]
    pub kind: InboundKind,
}

/// Provenance of inbound information.
///
/// Provenance is metadata; it never changes the message role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserSource {
    /// A human user.
    Human,
    /// Another agent.
    Agent {
        /// Identity of the sending agent.
        agent_id: AgentId,
    },
    /// The fleet/control plane.
    Fleet,
    /// An external system.
    ExternalSystem,
    /// The runtime itself.
    Runtime,
}

/// Typed kind of inbound information.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundKind {
    /// An ordinary inbound message.
    #[default]
    Message,
    /// A future runtime compaction summary: inbound runtime-provided
    /// historical context. Compaction itself is not implemented in M1; the
    /// kind exists so no fifth message role is ever needed for it.
    CompactionSummary,
}

/// A content block inside a `UserMessageBlock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserContentBlock {
    /// Plain text.
    Text(TextBlock),
    /// An image reference.
    Image(ImageReference),
    /// A file reference.
    File(FileReference),
}

/// One completed model generation produced by the current agent.
///
/// One generation becomes one immutable `AgentMessageBlock` containing
/// multiple content blocks. Streaming deltas are never committed here; they
/// belong to `ModelEvent` until the generation completes. `send_message`
/// results and other inbound material from other agents never appear in this
/// role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMessageBlock {
    /// Durable message identity.
    pub id: MessageId,
    /// The completed generation content.
    pub content: Vec<AgentContentBlock>,
}

/// A content block inside an `AgentMessageBlock`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentContentBlock {
    /// Generated text.
    Text(TextBlock),
    /// Model reasoning, with optional provider continuation state.
    Reasoning(ReasoningBlock),
    /// A tool call emitted by the generation.
    ToolCall(ToolCall),
    /// A refusal to comply with the request.
    Refusal(RefusalBlock),
    /// An image reference produced by the generation.
    Image(ImageReference),
}

/// Model reasoning content.
///
/// Reasoning text is preserved for diagnostics, but reasoning/continuation
/// state is never flattened into plain text: provider-specific opaque state
/// survives on the [`ProviderContinuationState`] boundary for later
/// continuation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReasoningBlock {
    /// The reasoning text, when the provider exposed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Provider continuation state required to continue the generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_state: Option<ProviderContinuationState>,
}

/// A refusal generated by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusalBlock {
    /// The refusal explanation text.
    pub text: String,
}

/// The result of one tool call produced by the current agent.
///
/// This block is the canonical conversation record of an execution outcome
/// and composes [`ToolExecutionResult`] as its single source of truth. For
/// `send_message`-style platform tools, the result is only the delivery
/// acceptance/rejection acknowledgment; a later reply from the recipient
/// arrives as a `UserMessageBlock` with agent provenance and is never nested
/// here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolMessageBlock {
    /// Durable message identity.
    pub id: MessageId,
    /// Identity of the tool call this block answers.
    pub tool_call_id: ToolCallId,
    /// Identity of the executed tool.
    pub tool_id: ToolId,
    /// The normalized execution result.
    pub result: ToolExecutionResult,
}

#[cfg(test)]
mod tests {
    use super::{
        AgentContentBlock, AgentMessageBlock, InboundKind, MessageBlock, SystemAuthority,
        SystemMessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::message::content::TextBlock;
    use crate::runtime::identity::{AgentId, MessageId, ToolCallId, ToolId};
    use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};

    /// All four canonical roles serialize with their stable discriminators.
    #[test]
    fn four_roles_have_stable_discriminators() {
        let system = MessageBlock::System(SystemMessageBlock {
            id: MessageId::new("msg-sys-1"),
            authority: SystemAuthority::Runtime,
            content: vec![TextBlock {
                text: "Be concise.".to_owned(),
            }],
        });
        let user = MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-user-1"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "hi".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
        });
        let agent = MessageBlock::Agent(AgentMessageBlock {
            id: MessageId::new("msg-agent-1"),
            content: vec![AgentContentBlock::Text(TextBlock {
                text: "ok".to_owned(),
            })],
        });
        let tool = MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("msg-tool-1"),
            tool_call_id: ToolCallId::new("call_01"),
            tool_id: ToolId::new("tool-bash"),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
            },
        });
        for (block, role) in [
            (system, "system"),
            (user, "user"),
            (agent, "agent"),
            (tool, "tool"),
        ] {
            let value = serde_json::to_value(&block).expect("serialize block");
            assert_eq!(value["role"], role, "unexpected discriminator");
        }
    }

    /// An inbound message from another agent stays a `UserMessageBlock`.
    #[test]
    fn agent_to_agent_inbound_remains_user_role() {
        let block = MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-user-2"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Task done.".to_owned(),
            })],
            source: UserSource::Agent {
                agent_id: AgentId::new("agent-b"),
            },
            kind: InboundKind::Message,
        });
        let value = serde_json::to_value(&block).expect("serialize block");
        assert_eq!(value["role"], "user");
        assert_eq!(value["source"]["agent"]["agent_id"], "agent-b");
        assert!(matches!(
            block,
            MessageBlock::User(UserMessageBlock {
                source: UserSource::Agent { .. },
                ..
            })
        ));
        assert!(!matches!(block, MessageBlock::Agent(_)));
        assert!(!matches!(block, MessageBlock::Tool(_)));
    }

    /// A future compaction summary remains a `UserMessageBlock`; no fifth
    /// role is required.
    #[test]
    fn compaction_summary_is_user_role() {
        let block = MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-summary-1"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Earlier in the conversation the agent listed files.".to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::CompactionSummary,
        });
        let json = serde_json::to_string(&block).expect("serialize block");
        let decoded: MessageBlock = serde_json::from_str(&json).expect("deserialize block");
        assert_eq!(decoded, block);
        assert!(matches!(
            decoded,
            MessageBlock::User(UserMessageBlock {
                kind: InboundKind::CompactionSummary,
                source: UserSource::Runtime,
                ..
            })
        ));
    }
}
