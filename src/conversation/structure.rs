//! The deterministic structural analysis of the **active** conversation.
//!
//! Structure is a property of canonical conversation messages, so it lives
//! in the conversation domain and is computed over the messages the
//! Conversation Surface currently makes active — never over the complete
//! Message Ledger.
//!
//! The contracts it enforces are frozen:
//!
//! ```text
//! Agent/Assistant owns ToolCall identity and arguments
//! Tool            owns the execution result and references ToolCallId
//! ```
//!
//! A `ToolMessageBlock` whose call resolves to no active owning agent
//! message is malformed and rejected explicitly, never guessed around. A
//! Surface span is replaceable only when no tool-call/result relationship
//! crosses either of its boundaries: a retained tool result can never lose
//! its active owning tool call, and vice versa. Raw message counts are never
//! a structural boundary heuristic.

use std::collections::BTreeMap;

use crate::message::types::{AgentContentBlock, MessageBlock};
use crate::runtime::identity::{MessageId, ToolCallId};

/// A structural contract violation of the active conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuralError {
    /// A tool result references a tool call no active agent message issued.
    OrphanToolResult {
        /// The offending tool message.
        message_id: MessageId,
        /// The unresolvable tool call identity.
        tool_call_id: ToolCallId,
    },
    /// The same tool call identity is issued by more than one agent message.
    DuplicateToolCall(ToolCallId),
    /// The same tool call has more than one result.
    DuplicateToolResult {
        /// The duplicated tool call identity.
        tool_call_id: ToolCallId,
        /// The offending duplicate tool message.
        message_id: MessageId,
    },
    /// A requested span would separate a tool call from its result.
    SplitToolPair {
        /// The tool call whose relationship the span would break.
        tool_call_id: ToolCallId,
    },
    /// A requested span contains a trusted `System` message.
    ///
    /// This is the narrow interim rule of Issue #54: trusted System content
    /// is never demoted into a runtime `User` compaction summary. The full
    /// Effective System Prompt architecture belongs to Issue #55.
    SystemMessageInSpan(MessageId),
}

impl core::fmt::Display for StructuralError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OrphanToolResult {
                message_id,
                tool_call_id,
            } => write!(
                f,
                "tool message {message_id} references tool call {tool_call_id}, \
                 which no active agent message issued"
            ),
            Self::DuplicateToolCall(id) => {
                write!(f, "tool call {id} is issued by more than one agent message")
            }
            Self::DuplicateToolResult {
                tool_call_id,
                message_id,
            } => write!(
                f,
                "tool message {message_id} duplicates the result of call {tool_call_id}"
            ),
            Self::SplitToolPair { tool_call_id } => write!(
                f,
                "the requested span would separate tool call {tool_call_id} from its result"
            ),
            Self::SystemMessageInSpan(id) => write!(
                f,
                "the requested span contains trusted system message {id}; \
                 system content is never replaced by a runtime compaction summary"
            ),
        }
    }
}

impl std::error::Error for StructuralError {}

/// The structural facts of one active conversation, by active position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralIndex {
    /// Every agent message position, in active order.
    agent_positions: Vec<usize>,
    /// `tool_call_id` → the active position of the requesting agent message.
    call_owners: BTreeMap<ToolCallId, usize>,
    /// `tool_call_id` → the active position of its result, when one exists.
    results: BTreeMap<ToolCallId, usize>,
    /// agent position → the last active position of its turn.
    turn_ends: BTreeMap<usize, usize>,
    /// active position → whether the message is a `System` message.
    system_positions: Vec<usize>,
    /// active position → the message identity at that position.
    ids: Vec<MessageId>,
}

impl StructuralIndex {
    /// Builds the structural index of the active conversation messages.
    ///
    /// # Errors
    ///
    /// Returns the [`StructuralError`] of the first violation: an orphan
    /// tool result, a tool call issued twice, or a duplicated tool result.
    pub fn build(active: &[MessageBlock]) -> Result<Self, StructuralError> {
        let mut agent_positions = Vec::new();
        let mut call_owners: BTreeMap<ToolCallId, usize> = BTreeMap::new();
        let mut results: BTreeMap<ToolCallId, usize> = BTreeMap::new();
        let mut system_positions = Vec::new();
        let mut ids = Vec::with_capacity(active.len());
        for (position, message) in active.iter().enumerate() {
            ids.push(super::ledger::message_id_of(message));
            match message {
                MessageBlock::System(_) => system_positions.push(position),
                MessageBlock::Agent(agent) => {
                    agent_positions.push(position);
                    for block in &agent.content {
                        if let AgentContentBlock::ToolCall(call) = block
                            && call_owners.insert(call.id.clone(), position).is_some()
                        {
                            return Err(StructuralError::DuplicateToolCall(call.id.clone()));
                        }
                    }
                }
                MessageBlock::Tool(tool) => {
                    if !call_owners.contains_key(&tool.tool_call_id) {
                        return Err(StructuralError::OrphanToolResult {
                            message_id: tool.id.clone(),
                            tool_call_id: tool.tool_call_id.clone(),
                        });
                    }
                    if results
                        .insert(tool.tool_call_id.clone(), position)
                        .is_some()
                    {
                        return Err(StructuralError::DuplicateToolResult {
                            tool_call_id: tool.tool_call_id.clone(),
                            message_id: tool.id.clone(),
                        });
                    }
                }
                MessageBlock::User(_) => {}
            }
        }
        let mut turn_ends = BTreeMap::new();
        for &agent_position in &agent_positions {
            let MessageBlock::Agent(agent) = &active[agent_position] else {
                unreachable!("agent_positions only holds agent messages");
            };
            let mut end = agent_position;
            for block in &agent.content {
                if let AgentContentBlock::ToolCall(call) = block
                    && let Some(&result_position) = results.get(&call.id)
                {
                    end = end.max(result_position);
                }
            }
            turn_ends.insert(agent_position, end);
        }
        Ok(Self {
            agent_positions,
            call_owners,
            results,
            turn_ends,
            system_positions,
            ids,
        })
    }

    /// The number of indexed active messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The active position of one message identity.
    #[must_use]
    pub fn position_of(&self, message_id: &MessageId) -> Option<usize> {
        self.ids.iter().position(|id| id == message_id)
    }

    /// The identity at one active position.
    #[must_use]
    pub fn id_at(&self, position: usize) -> Option<&MessageId> {
        self.ids.get(position)
    }

    /// Every agent message position, in active order.
    #[must_use]
    pub fn agent_positions(&self) -> &[usize] {
        &self.agent_positions
    }

    /// Every `System` message position, in active order.
    #[must_use]
    pub fn system_positions(&self) -> &[usize] {
        &self.system_positions
    }

    /// The last active position of the turn owned by the agent message at
    /// `agent_position`: its own position, or the greatest position of its
    /// committed results.
    ///
    /// # Panics
    ///
    /// Panics when `agent_position` is not an agent message position.
    #[must_use]
    pub fn turn_end_of(&self, agent_position: usize) -> usize {
        self.turn_ends[&agent_position]
    }

    /// Validates the inclusive active span `[start ..= end]` as a
    /// replaceable unit.
    ///
    /// The span must contain complete canonical messages only (which it does
    /// by construction: it is a range of whole active messages), it must not
    /// contain a trusted `System` message, and it must never separate a tool
    /// call from its result in either direction.
    ///
    /// # Errors
    ///
    /// Returns [`StructuralError::SystemMessageInSpan`] or
    /// [`StructuralError::SplitToolPair`].
    pub fn validate_span(&self, start: usize, end: usize) -> Result<(), StructuralError> {
        if let Some(&position) = self
            .system_positions
            .iter()
            .find(|&&position| position >= start && position <= end)
        {
            return Err(StructuralError::SystemMessageInSpan(
                self.ids[position].clone(),
            ));
        }
        for (call_id, &owner) in &self.call_owners {
            let Some(&result) = self.results.get(call_id) else {
                // A pending call with no committed result imposes no edge:
                // the Agent Loop contract allows it to remain representable.
                continue;
            };
            let owner_inside = owner >= start && owner <= end;
            let result_inside = result >= start && result <= end;
            if owner_inside != result_inside {
                return Err(StructuralError::SplitToolPair {
                    tool_call_id: call_id.clone(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{StructuralError, StructuralIndex};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AgentContentBlock, AgentMessageBlock, InboundKind, MessageBlock, SystemAuthority,
        SystemMessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
    use crate::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};

    fn user(id: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "hi".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }

    fn system(id: &str) -> MessageBlock {
        MessageBlock::System(SystemMessageBlock {
            id: MessageId::new(id),
            authority: SystemAuthority::Platform,
            content: vec![TextBlock {
                text: "be concise".to_owned(),
            }],
        })
    }

    fn agent(id: &str, calls: &[&str]) -> MessageBlock {
        MessageBlock::Agent(AgentMessageBlock {
            id: MessageId::new(id),
            content: calls
                .iter()
                .map(|call| {
                    AgentContentBlock::ToolCall(ToolCall {
                        id: ToolCallId::new(*call),
                        tool_id: ToolId::new("tool-a"),
                        name: "alpha".to_owned(),
                        arguments: serde_json::json!({}),
                    })
                })
                .collect(),
        })
    }

    fn tool(call: &str) -> MessageBlock {
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new(format!("tool-{call}")),
            tool_call_id: ToolCallId::new(call),
            tool_id: ToolId::new("tool-a"),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 1,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
            },
        })
    }

    /// An orphan tool result is malformed, never guessed around.
    #[test]
    fn orphan_tool_result_is_rejected() {
        let error = StructuralIndex::build(&[user("u1"), tool("ghost")]).expect_err("rejected");
        assert_eq!(
            error,
            StructuralError::OrphanToolResult {
                message_id: MessageId::new("tool-ghost"),
                tool_call_id: ToolCallId::new("ghost"),
            }
        );
    }

    /// A span never separates a tool call from its result, in either
    /// direction.
    #[test]
    fn spans_never_split_a_tool_pair() {
        let active = vec![user("u1"), agent("a1", &["c1"]), tool("c1"), user("u2")];
        let index = StructuralIndex::build(&active).expect("well-formed");
        assert!(index.validate_span(0, 0).is_ok());
        assert!(index.validate_span(0, 2).is_ok());
        assert!(index.validate_span(0, 3).is_ok());
        assert_eq!(
            index
                .validate_span(0, 1)
                .expect_err("retires the call only"),
            StructuralError::SplitToolPair {
                tool_call_id: ToolCallId::new("c1"),
            }
        );
        assert_eq!(
            index
                .validate_span(2, 3)
                .expect_err("retires the result only"),
            StructuralError::SplitToolPair {
                tool_call_id: ToolCallId::new("c1"),
            }
        );
    }

    /// A pending call with no committed result imposes no edge.
    #[test]
    fn pending_calls_impose_no_edge() {
        let active = vec![user("u1"), agent("a1", &["c1"])];
        let index = StructuralIndex::build(&active).expect("well-formed");
        assert!(index.validate_span(0, 1).is_ok());
        assert!(index.validate_span(0, 0).is_ok());
    }

    /// Trusted system content is never inside a replaceable span.
    #[test]
    fn system_messages_are_never_inside_a_span() {
        let active = vec![system("s1"), user("u1"), system("s2"), user("u2")];
        let index = StructuralIndex::build(&active).expect("well-formed");
        assert_eq!(
            index.validate_span(0, 1).expect_err("leading system"),
            StructuralError::SystemMessageInSpan(MessageId::new("s1"))
        );
        assert!(index.validate_span(1, 1).is_ok());
        assert_eq!(
            index.validate_span(1, 3).expect_err("later system"),
            StructuralError::SystemMessageInSpan(MessageId::new("s2"))
        );
        assert!(index.validate_span(3, 3).is_ok());
        assert_eq!(index.system_positions(), &[0, 2]);
    }

    /// The turn end covers the agent message and all of its results.
    #[test]
    fn turn_end_covers_agent_and_results() {
        let active = vec![
            user("u1"),
            agent("a1", &["c1", "c2"]),
            tool("c1"),
            tool("c2"),
        ];
        let index = StructuralIndex::build(&active).expect("well-formed");
        assert_eq!(index.turn_end_of(1), 3);
        assert_eq!(index.agent_positions(), &[1]);
        assert_eq!(index.len(), 4);
    }
}
