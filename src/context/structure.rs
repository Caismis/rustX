//! The deterministic structural index of canonical history.
//!
//! Cut selection never uses `messages.len() - N` and never guesses: the
//! engine builds a structural index over canonical history that records the
//! tool-call/result edges of every turn. A whole-message cut is valid only
//! when no tool-call/result edge crosses it, and malformed history (a
//! `ToolMessageBlock` whose call resolves to no requesting agent message) is
//! rejected explicitly rather than guessed around.

use std::collections::BTreeMap;

use crate::context::error::{ContextError, ContextErrorKind};
use crate::message::types::{AgentContentBlock, MessageBlock};
use crate::runtime::identity::{MessageId, ToolCallId};

/// The structural facts of one canonical history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralIndex {
    /// The number of leading pinned messages: everything through the last
    /// `SystemMessageBlock`, inclusive, is literal and outside summary
    /// coverage.
    pub pinned_end: usize,
    /// The position of every agent message, in order.
    agent_positions: Vec<usize>,
    /// The content length of every agent message, keyed by its position.
    agent_content_len: BTreeMap<usize, usize>,
    /// The tool calls of every agent message, keyed by agent position, in
    /// content block order: (content block index, call id).
    agent_calls: BTreeMap<usize, Vec<(usize, ToolCallId)>>,
    /// `tool_call_id` → the position of the requesting agent message.
    call_owners: BTreeMap<ToolCallId, usize>,
    /// `tool_call_id` → the position of its result message, when one exists.
    results: BTreeMap<ToolCallId, usize>,
    /// agent message position → the last message position of its turn (its
    /// own position or the greatest position of its results).
    turn_ends: BTreeMap<usize, usize>,
    /// message position → the message id at that position.
    ids: Vec<MessageId>,
}

impl StructuralIndex {
    /// Builds the structural index of one canonical history.
    ///
    /// # Errors
    ///
    /// Returns [`ContextErrorKind::MalformedHistory`] when a
    /// `ToolMessageBlock` cannot be attributed to a requesting
    /// `AgentMessageBlock` (an orphan tool result), when a tool call is
    /// issued by more than one agent message, or when the history is
    /// otherwise structurally contradictory. Malformed history is never
    /// guessed around.
    pub fn build(history: &[MessageBlock]) -> Result<Self, ContextError> {
        let mut agent_positions = Vec::new();
        let mut agent_content_len = BTreeMap::new();
        let mut agent_calls = BTreeMap::new();
        let mut call_owners: BTreeMap<ToolCallId, usize> = BTreeMap::new();
        let mut results: BTreeMap<ToolCallId, usize> = BTreeMap::new();
        let mut ids = Vec::with_capacity(history.len());
        let mut last_system = None;
        for (position, message) in history.iter().enumerate() {
            ids.push(message_id(message));
            match message {
                MessageBlock::System(_) => last_system = Some(position),
                MessageBlock::Agent(agent) => {
                    agent_positions.push(position);
                    agent_content_len.insert(position, agent.content.len());
                    let calls: &mut Vec<(usize, ToolCallId)> =
                        agent_calls.entry(position).or_default();
                    for (block, block_content) in agent.content.iter().enumerate() {
                        if let AgentContentBlock::ToolCall(call) = block_content {
                            calls.push((block, call.id.clone()));
                            if call_owners.insert(call.id.clone(), position).is_some() {
                                return Err(malformed(&format!(
                                    "tool call {} issued by more than one agent message",
                                    call.id
                                )));
                            }
                        }
                    }
                }
                MessageBlock::Tool(tool) => {
                    if !call_owners.contains_key(&tool.tool_call_id) {
                        return Err(malformed(&format!(
                            "tool message {} references no requesting agent message",
                            tool.id
                        )));
                    }
                    if results
                        .insert(tool.tool_call_id.clone(), position)
                        .is_some()
                    {
                        return Err(malformed(&format!(
                            "tool message {} duplicates the result of call {}",
                            tool.id, tool.tool_call_id
                        )));
                    }
                }
                MessageBlock::User(_) => {}
            }
        }
        let mut turn_ends = BTreeMap::new();
        for &agent_position in &agent_positions {
            let MessageBlock::Agent(agent) = &history[agent_position] else {
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
            pinned_end: last_system.map_or(0, |position| position + 1),
            agent_positions,
            agent_content_len,
            agent_calls,
            call_owners,
            results,
            turn_ends,
            ids,
        })
    }

    /// The position of a message id in the indexed history.
    #[must_use]
    pub fn position_of(&self, message_id: &MessageId) -> Option<usize> {
        self.ids.iter().position(|id| id == message_id)
    }

    /// Whether a whole-message cut after position `cut` (retiring
    /// `history[..cut]`) separates no tool call from its result.
    ///
    /// A retired call with a retained result is the only way an edge can
    /// cross the cut: results always follow their calls, so a retained call
    /// always retains its result.
    #[must_use]
    pub fn whole_cut_is_valid(&self, cut: usize) -> bool {
        self.agent_positions
            .iter()
            .filter(|&&position| position < cut)
            .all(|&position| self.turn_ends[&position] < cut)
    }

    /// Every agent message position, in order.
    #[must_use]
    pub fn agent_positions(&self) -> &[usize] {
        &self.agent_positions
    }

    /// The last message position of the turn that owns `message_id`, when
    /// `message_id` is an agent message with a complete turn.
    ///
    /// Used to enforce the continuation constraint: the boundary must cover
    /// the continuation-owning agent message and the complete tool-result
    /// portion of its turn.
    #[must_use]
    pub fn turn_end_of(&self, agent_position: usize) -> usize {
        self.turn_ends[&agent_position]
    }

    /// Whether a split of the agent message at `agent_position` with
    /// `first_retained_block` is well-formed: it must retire at least one
    /// content block and retain at least one.
    #[must_use]
    pub fn split_is_valid(&self, agent_position: usize, first_retained_block: usize) -> bool {
        let Some(&content_len) = self.agent_content_len.get(&agent_position) else {
            return false;
        };
        first_retained_block > 0 && first_retained_block < content_len
    }

    /// The content length of the agent message at `agent_position`.
    #[must_use]
    pub fn content_len_of(&self, agent_position: usize) -> Option<usize> {
        self.agent_content_len.get(&agent_position).copied()
    }

    /// The tool calls of the agent message at `agent_position`, in content
    /// block order: `(content block index, call id)`.
    #[must_use]
    pub fn calls_of(&self, agent_position: usize) -> &[(usize, ToolCallId)] {
        self.agent_calls
            .get(&agent_position)
            .map_or(&[], Vec::as_slice)
    }

    /// The result position of one tool call, when its result was committed.
    #[must_use]
    pub fn result_position(&self, call_id: &ToolCallId) -> Option<usize> {
        self.results.get(call_id).copied()
    }

    /// The number of indexed messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// The message id of one canonical block.
#[must_use]
pub fn message_id(message: &MessageBlock) -> MessageId {
    match message {
        MessageBlock::System(system) => system.id.clone(),
        MessageBlock::User(user) => user.id.clone(),
        MessageBlock::Agent(agent) => agent.id.clone(),
        MessageBlock::Tool(tool) => tool.id.clone(),
    }
}

fn malformed(message: &str) -> ContextError {
    ContextError::new(ContextErrorKind::MalformedHistory, message)
}

#[cfg(test)]
mod tests {
    use super::StructuralIndex;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AgentContentBlock, AgentMessageBlock, MessageBlock, ToolMessageBlock, UserContentBlock,
        UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
    use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};

    fn user(id: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "hi".to_owned(),
            })],
            source: UserSource::Human,
            kind: crate::message::types::InboundKind::Message,
            timestamp: None,
        })
    }

    fn agent(id: &str, calls: &[&str]) -> MessageBlock {
        MessageBlock::Agent(AgentMessageBlock {
            id: MessageId::new(id),
            content: calls
                .iter()
                .map(|call| {
                    AgentContentBlock::ToolCall(crate::tools::types::ToolCall {
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

    /// An orphan tool message is malformed history, never guessed around.
    #[test]
    fn orphan_tool_message_is_malformed() {
        let history = vec![user("u1"), tool("ghost")];
        let error = StructuralIndex::build(&history).expect_err("must be rejected");
        assert_eq!(
            error.kind,
            crate::context::error::ContextErrorKind::MalformedHistory
        );
        assert!(error.message.contains("no requesting agent message"));
    }

    /// A whole cut never separates a tool call from its result.
    #[test]
    fn whole_cuts_respect_call_result_edges() {
        let history = vec![user("u1"), agent("a1", &["c1"]), tool("c1"), user("u2")];
        let index = StructuralIndex::build(&history).expect("well-formed");
        assert!(index.whole_cut_is_valid(1));
        assert!(!index.whole_cut_is_valid(2));
        assert!(index.whole_cut_is_valid(3));
        assert!(index.whole_cut_is_valid(4));
    }

    /// A pending call (no committed result) imposes no edge constraint.
    #[test]
    fn pending_calls_impose_no_edge() {
        let history = vec![user("u1"), agent("a1", &["c1"])];
        let index = StructuralIndex::build(&history).expect("well-formed");
        assert!(index.whole_cut_is_valid(1));
        assert!(index.whole_cut_is_valid(2));
    }

    /// The turn end covers the agent message and all of its results.
    #[test]
    fn turn_end_covers_agent_and_results() {
        let history = vec![
            user("u1"),
            agent("a1", &["c1", "c2"]),
            tool("c1"),
            tool("c2"),
        ];
        let index = StructuralIndex::build(&history).expect("well-formed");
        assert_eq!(index.turn_end_of(1), 3);
        assert_eq!(index.pinned_end, 0);
    }

    /// A split must retire and retain at least one content block.
    #[test]
    fn split_must_be_interior() {
        let history = vec![agent("a1", &["c1"])];
        let index = StructuralIndex::build(&history).expect("well-formed");
        assert!(!index.split_is_valid(0, 0));
        assert!(!index.split_is_valid(0, 1));
        let two_blocks = vec![
            user("u1"),
            agent("a2", &["c1", "c2"]),
            tool("c1"),
            tool("c2"),
        ];
        let index = StructuralIndex::build(&two_blocks).expect("well-formed");
        assert!(!index.split_is_valid(1, 0));
        assert!(index.split_is_valid(1, 1));
        assert!(!index.split_is_valid(1, 2));
    }
}
