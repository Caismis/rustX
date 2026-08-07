//! Canonical model stream assembly and validation.
//!
//! The agent loop owns `AgentMessageBlock` assembly from the canonical
//! `ModelEvent` stream (M2 explicitly deferred this to the loop). This module
//! validates the stream contract and assembles one ordered message:
//!
//! - a stream starts with `Started` (or is a bare terminal `Failed` for a
//!   request rejected before provider execution) and has at most one
//!   terminal event;
//! - no event follows the terminal event;
//! - content blocks assemble in contiguous `ContentBlockIndex` order, with
//!   interleaved text, reasoning, refusal, tool-call, and continuation-state
//!   facts targeting their block;
//! - tool calls assemble by call identity: argument deltas and completion
//!   reference a started call, and a call completes exactly once;
//! - a refusal terminal rolls back provisional non-refusal content, so the
//!   committed message holds only the refusal blocks (the provider streams
//!   provisional output that a later refusal invalidates);
//! - provider continuation state attaches to its reasoning block, and the
//!   boundary (greatest-block-index) state is reported for the next request.
//!
//! Violations of the canonical contract are explicit
//! [`RuntimeError::ContractViolation`] failures; impossible streams are
//! never silently accepted.

use std::collections::BTreeMap;

use crate::message::content::TextBlock;
use crate::message::types::{
    AgentContentBlock, ContentBlockIndex, ReasoningBlock, RefusalBlock,
};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::types::ModelUsage;
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::ToolCallId;
use crate::runtime::types::RuntimeError;
use crate::tools::types::ToolCall;

/// The assembled result of one completed model turn.
#[derive(Debug, Clone, PartialEq)]
pub struct AssembledTurn {
    /// The assembled content blocks in index order, after refusal rollback.
    pub content: Vec<AgentContentBlock>,
    /// The fully assembled tool calls of this turn, in block order. These
    /// are the calls the loop must execute before the model continues.
    pub tool_calls: Vec<ToolCall>,
    /// The provider continuation state of the boundary reasoning block, when
    /// the stream reported one. Propagated losslessly into the next request.
    pub continuation: Option<ProviderContinuationState>,
    /// Final usage: the terminal event's reported usage, else the latest
    /// usage update.
    pub usage: Option<ModelUsage>,
}

/// The in-flight assembly state of one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingCall {
    block_index: ContentBlockIndex,
    completed: bool,
}

/// Assemblies one canonical `ModelEvent` stream into an ordered message.
///
/// The assembler owns no runtime event emission and no identities; the loop
/// maps stream facts to runtime events and assigns message identities.
#[derive(Debug, Default)]
pub struct ModelEventAssembler {
    blocks: Vec<Option<AgentContentBlock>>,
    tool_calls: BTreeMap<ToolCallId, PendingCall>,
    completed_calls: Vec<(ContentBlockIndex, ToolCall)>,
    continuation: Option<(ContentBlockIndex, ProviderContinuationState)>,
    usage: Option<ModelUsage>,
    started: bool,
    terminal: bool,
}

impl ModelEventAssembler {
    /// Creates an empty assembler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the stream started.
    #[must_use]
    pub fn started(&self) -> bool {
        self.started
    }

    /// Whether a terminal event was already accepted.
    #[must_use]
    pub fn terminal(&self) -> bool {
        self.terminal
    }

    /// Accepts one model event, validating the canonical stream contract.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::ContractViolation`] when the event violates
    /// the stream contract: an event before `Started` other than a terminal
    /// `Failed`, any event after the terminal event, a second terminal
    /// event, a content delta targeting a foreign or skipped block, or a
    /// tool-call delta that cannot be attributed to an open call.
    pub fn push(&mut self, event: &ModelEvent) -> Result<(), RuntimeError> {
        if self.terminal {
            return Err(violation("model event after the terminal event"));
        }
        match event {
            ModelEvent::Started => {
                if self.started {
                    return Err(violation("duplicate Started event"));
                }
                self.started = true;
                Ok(())
            }
            ModelEvent::Failed { .. } => {
                self.terminal = true;
                Ok(())
            }
            ModelEvent::Completed { .. } => {
                if !self.started {
                    return Err(violation("terminal Completed before Started"));
                }
                self.terminal = true;
                Ok(())
            }
            _ => {
                if !self.started {
                    return Err(violation("model content before Started"));
                }
                match event {
                    ModelEvent::TextDelta { block_index, text } => {
                        self.push_text(*block_index, text)
                    }
                    ModelEvent::ReasoningDelta { block_index, text } => {
                        self.push_reasoning(*block_index, text)
                    }
                    ModelEvent::RefusalDelta { block_index, text } => {
                        self.push_refusal(*block_index, text)
                    }
                    ModelEvent::ToolCallStarted { block_index, call } => {
                        self.register_tool_call(*block_index, call)
                    }
                    ModelEvent::ToolCallArgumentsDelta {
                        block_index,
                        call_id,
                        arguments_delta: _,
                    } => self.validate_arguments_delta(*block_index, call_id),
                    ModelEvent::ToolCallCompleted { block_index, call } => {
                        self.complete_tool_call(*block_index, call)
                    }
                    ModelEvent::UsageUpdate { usage } => {
                        self.usage = Some(usage.clone());
                        Ok(())
                    }
                    ModelEvent::ContinuationState { block_index, state } => {
                        self.attach_continuation(*block_index, state)
                    }
                    ModelEvent::Started | ModelEvent::Failed { .. }
                    | ModelEvent::Completed { .. } => unreachable!(),
                }
            }
        }
    }

    /// Appends a text delta to its block.
    fn push_text(
        &mut self,
        block_index: ContentBlockIndex,
        text: &str,
    ) -> Result<(), RuntimeError> {
        let block = self.block_at(block_index, BlockKind::Text)?;
        match block {
            AgentContentBlock::Text(text_block) => {
                text_block.text.push_str(text);
                Ok(())
            }
            _ => unreachable!("block_at validated the text kind"),
        }
    }

    /// Appends a reasoning delta to its block.
    fn push_reasoning(
        &mut self,
        block_index: ContentBlockIndex,
        text: &str,
    ) -> Result<(), RuntimeError> {
        let block = self.block_at(block_index, BlockKind::Reasoning)?;
        match block {
            AgentContentBlock::Reasoning(reasoning) => {
                reasoning.text.get_or_insert_with(String::new).push_str(text);
                Ok(())
            }
            _ => unreachable!("block_at validated the reasoning kind"),
        }
    }

    /// Appends a refusal delta to its block.
    fn push_refusal(
        &mut self,
        block_index: ContentBlockIndex,
        text: &str,
    ) -> Result<(), RuntimeError> {
        let block = self.block_at(block_index, BlockKind::Refusal)?;
        match block {
            AgentContentBlock::Refusal(refusal) => {
                refusal.text.push_str(text);
                Ok(())
            }
            _ => unreachable!("block_at validated the refusal kind"),
        }
    }

    /// Validates that an argument delta belongs to an open tool call.
    fn validate_arguments_delta(
        &self,
        block_index: ContentBlockIndex,
        call_id: &ToolCallId,
    ) -> Result<(), RuntimeError> {
        let Some(pending) = self.tool_calls.get(call_id) else {
            return Err(violation(&format!(
                "tool-call arguments delta for unknown call {call_id}"
            )));
        };
        if pending.completed {
            return Err(violation(&format!(
                "tool-call arguments delta after completion of {call_id}"
            )));
        }
        if pending.block_index != block_index {
            return Err(violation(&format!(
                "tool-call arguments delta block mismatch for {call_id}"
            )));
        }
        Ok(())
    }

    /// Finalizes the turn: rolls back provisional content on a refusal and
    /// rejects streams with tool calls that never completed.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::ContractViolation`] when a started tool call
    /// never completed before the terminal event.
    pub fn finish(
        self,
        finish_reason: &ModelFinishReason,
        reported_usage: Option<ModelUsage>,
    ) -> Result<AssembledTurn, RuntimeError> {
        for (call_id, pending) in &self.tool_calls {
            if !pending.completed {
                return Err(violation(&format!(
                    "tool call {call_id} never completed before the terminal event"
                )));
            }
        }
        let refusal = *finish_reason == ModelFinishReason::Refusal;
        let mut content = Vec::with_capacity(self.blocks.len());
        for block in self.blocks {
            let Some(block) = block else {
                return Err(violation("assembly contains a missing block"));
            };
            if !refusal || matches!(block, AgentContentBlock::Refusal(_)) {
                content.push(block);
            }
        }
        let mut completed_calls = self.completed_calls;
        completed_calls.sort_by_key(|(block_index, _)| *block_index);
        let tool_calls: Vec<ToolCall> =
            completed_calls.into_iter().map(|(_, call)| call).collect();
        Ok(AssembledTurn {
            content,
            tool_calls,
            continuation: self.continuation.map(|(_, state)| state),
            usage: reported_usage.or(self.usage),
        })
    }

    /// Returns the block at `block_index`, creating it with `kind` when it
    /// is the next contiguous block, and rejects foreign or skipped indices.
    fn block_at(
        &mut self,
        block_index: ContentBlockIndex,
        kind: BlockKind,
    ) -> Result<&mut AgentContentBlock, RuntimeError> {
        let index = block_index.get() as usize;
        if index > self.blocks.len() {
            return Err(violation(&format!(
                "content delta skipped block index {block_index}"
            )));
        }
        if index == self.blocks.len() {
            self.blocks.push(Some(kind.placeholder()));
        }
        let existing = self
            .blocks
            .get(index)
            .and_then(Option::as_ref)
            .expect("block exists after creation");
        if !kind.matches(existing) {
            return Err(violation(&format!(
                "{} delta targets a foreign block {block_index}",
                kind.describe()
            )));
        }
        let block = self
            .blocks
            .get_mut(index)
            .and_then(Option::as_mut)
            .expect("block exists after creation");
        Ok(block)
    }

    fn register_tool_call(
        &mut self,
        block_index: ContentBlockIndex,
        call: &crate::tools::types::ToolCallStart,
    ) -> Result<(), RuntimeError> {
        if self.tool_calls.contains_key(&call.id) {
            return Err(violation(&format!(
                "duplicate start of tool call {}",
                call.id
            )));
        }
        let index = block_index.get() as usize;
        if index != self.blocks.len() {
            return Err(violation(&format!(
                "tool-call start targets a foreign or skipped block {block_index}"
            )));
        }
        self.blocks
            .push(Some(AgentContentBlock::ToolCall(ToolCall {
                id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: call.name.clone(),
                arguments: serde_json::Value::Object(serde_json::Map::new()),
            })));
        self.tool_calls.insert(
            call.id.clone(),
            PendingCall {
                block_index,
                completed: false,
            },
        );
        Ok(())
    }

    fn complete_tool_call(
        &mut self,
        block_index: ContentBlockIndex,
        call: &ToolCall,
    ) -> Result<(), RuntimeError> {
        let Some(pending) = self.tool_calls.get_mut(&call.id) else {
            return Err(violation(&format!(
                "tool-call completion for unknown call {}",
                call.id
            )));
        };
        if pending.completed {
            return Err(violation(&format!(
                "duplicate completion of tool call {}",
                call.id
            )));
        }
        if pending.block_index != block_index {
            return Err(violation(&format!(
                "tool-call completion block mismatch for {}",
                call.id
            )));
        }
        pending.completed = true;
        let index = block_index.get() as usize;
        self.blocks[index] = Some(AgentContentBlock::ToolCall(call.clone()));
        self.completed_calls.push((block_index, call.clone()));
        Ok(())
    }

    fn attach_continuation(
        &mut self,
        block_index: ContentBlockIndex,
        state: &ProviderContinuationState,
    ) -> Result<(), RuntimeError> {
        let index = block_index.get() as usize;
        if index > self.blocks.len() {
            return Err(violation(&format!(
                "continuation state skipped block index {block_index}"
            )));
        }
        if index == self.blocks.len() {
            self.blocks.push(Some(AgentContentBlock::Reasoning(
                ReasoningBlock {
                    text: None,
                    provider_state: None,
                },
            )));
        }
        let block = self
            .blocks
            .get_mut(index)
            .and_then(Option::as_mut)
            .expect("block exists after creation");
        let AgentContentBlock::Reasoning(reasoning) = block else {
            return Err(violation(&format!(
                "continuation state targets a non-reasoning block {block_index}"
            )));
        };
        reasoning.provider_state = Some(state.clone());
        if self
            .continuation
            .as_ref()
            .is_none_or(|(current, _)| *current <= block_index)
        {
            self.continuation = Some((block_index, state.clone()));
        }
        Ok(())
    }
}

fn violation(message: &str) -> RuntimeError {
    RuntimeError::ContractViolation {
        message: message.to_owned(),
    }
}

/// The content semantic a delta targets; new blocks are created as their
/// kind's placeholder and deltas of another kind are rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Reasoning,
    Refusal,
}

impl BlockKind {
    fn placeholder(self) -> AgentContentBlock {
        match self {
            Self::Text => AgentContentBlock::Text(TextBlock {
                text: String::new(),
            }),
            Self::Reasoning => AgentContentBlock::Reasoning(ReasoningBlock {
                text: None,
                provider_state: None,
            }),
            Self::Refusal => AgentContentBlock::Refusal(RefusalBlock {
                text: String::new(),
            }),
        }
    }

    fn matches(self, block: &AgentContentBlock) -> bool {
        matches!(
            (self, block),
            (Self::Text, AgentContentBlock::Text(_))
                | (Self::Reasoning, AgentContentBlock::Reasoning(_))
                | (Self::Refusal, AgentContentBlock::Refusal(_))
        )
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Reasoning => "reasoning",
            Self::Refusal => "refusal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AssembledTurn, ModelEventAssembler};
    use crate::message::types::{AgentContentBlock, ContentBlockIndex};
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::ModelUsage;
    use crate::runtime::continuation::{
        AnthropicContinuation, ProviderContinuationState,
    };
    use crate::runtime::identity::{ToolCallId, ToolId};
    use crate::runtime::types::RuntimeError;
    use crate::tools::types::{ToolCall, ToolCallStart};

    fn text(block: u32, text: &str) -> ModelEvent {
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(block),
            text: text.to_owned(),
        }
    }

    fn reasoning(block: u32, text: &str) -> ModelEvent {
        ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(block),
            text: text.to_owned(),
        }
    }

    fn refusal(block: u32, text: &str) -> ModelEvent {
        ModelEvent::RefusalDelta {
            block_index: ContentBlockIndex::new(block),
            text: text.to_owned(),
        }
    }

    fn completed(finish: ModelFinishReason) -> ModelEvent {
        ModelEvent::Completed {
            finish_reason: finish,
            usage: None,
        }
    }

    fn call_start(block: u32, id: &str) -> ModelEvent {
        ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(block),
            call: ToolCallStart {
                id: ToolCallId::new(id),
                tool_id: ToolId::new("tool-a"),
                name: "alpha".to_owned(),
            },
        }
    }

    fn call_args(block: u32, id: &str, fragment: &str) -> ModelEvent {
        ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(block),
            call_id: ToolCallId::new(id),
            arguments_delta: fragment.to_owned(),
        }
    }

    fn call_done(block: u32, id: &str, arguments: serde_json::Value) -> ModelEvent {
        ModelEvent::ToolCallCompleted {
            block_index: ContentBlockIndex::new(block),
            call: ToolCall {
                id: ToolCallId::new(id),
                tool_id: ToolId::new("tool-a"),
                name: "alpha".to_owned(),
                arguments,
            },
        }
    }

    fn assemble(events: &[ModelEvent], finish: &ModelFinishReason) -> AssembledTurn {
        let mut assembler = ModelEventAssembler::new();
        assembler.push(&ModelEvent::Started).expect("started");
        for event in events {
            assembler.push(event).expect("valid stream");
        }
        assembler.finish(finish, None).expect("finish turn")
    }

    /// A plain text turn assembles deltas in order into one text block.
    #[test]
    fn text_deltas_concatenate_in_order() {
        let turn = assemble(
            &[text(0, "hello"), text(0, " world"), completed(ModelFinishReason::Stop)],
            &ModelFinishReason::Stop,
        );
        assert_eq!(turn.content.len(), 1);
        let AgentContentBlock::Text(block) = &turn.content[0] else {
            panic!("expected a text block");
        };
        assert_eq!(block.text, "hello world");
        assert!(turn.tool_calls.is_empty());
        assert!(turn.continuation.is_none());
    }

    /// Interleaved reasoning, tool call, and text assemble in index order.
    #[test]
    fn interleaved_blocks_assemble_in_order() {
        let turn = assemble(
            &[
                reasoning(0, "Think."),
                call_start(1, "call-1"),
                call_args(1, "call-1", r#"{"x":1}"#),
                call_done(1, "call-1", serde_json::json!({"x": 1})),
                text(2, "Answer."),
                completed(ModelFinishReason::ToolCalls),
            ],
            &ModelFinishReason::ToolCalls,
        );
        assert_eq!(turn.content.len(), 3);
        assert!(matches!(&turn.content[0], AgentContentBlock::Reasoning(_)));
        assert!(matches!(&turn.content[1], AgentContentBlock::ToolCall(call) if call.id.as_str() == "call-1"));
        assert!(matches!(&turn.content[2], AgentContentBlock::Text(t) if t.text == "Answer."));
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].arguments, serde_json::json!({"x": 1}));
    }

    /// A refusal terminal rolls back provisional content: only the refusal
    /// blocks remain in the committed message.
    #[test]
    fn refusal_rolls_back_provisional_content() {
        let turn = assemble(
            &[
                text(0, "I would normally answer, but"),
                refusal(1, "I cannot comply."),
                completed(ModelFinishReason::Refusal),
            ],
            &ModelFinishReason::Refusal,
        );
        assert_eq!(turn.content.len(), 1);
        assert!(matches!(&turn.content[0], AgentContentBlock::Refusal(r) if r.text == "I cannot comply."));
        assert!(turn.tool_calls.is_empty());
    }

    /// Multiple refusal deltas concatenate into one refusal block.
    #[test]
    fn refusal_deltas_concatenate() {
        let turn = assemble(
            &[
                refusal(0, "I cannot "),
                refusal(0, "comply."),
                completed(ModelFinishReason::Refusal),
            ],
            &ModelFinishReason::Refusal,
        );
        let AgentContentBlock::Refusal(block) = &turn.content[0] else {
            panic!("expected a refusal block");
        };
        assert_eq!(block.text, "I cannot comply.");
    }

    /// Continuation state attaches to its reasoning block and becomes the
    /// boundary continuation, even when no reasoning text was exposed.
    #[test]
    fn continuation_state_attaches_losslessly() {
        let state = ProviderContinuationState::Anthropic(AnthropicContinuation {
            opaque: serde_json::json!({"signature": "sig-1"}),
        });
        let mut assembler = ModelEventAssembler::new();
        assembler
            .push(&ModelEvent::Started)
            .expect("started");
        assembler
            .push(&ModelEvent::ContinuationState {
                block_index: ContentBlockIndex::new(0),
                state: state.clone(),
            })
            .expect("state");
        assembler
            .push(&text(1, "Visible."))
            .expect("text");
        assembler
            .push(&completed(ModelFinishReason::Stop))
            .expect("completed");
        let turn = assembler.finish(&ModelFinishReason::Stop, None).expect("finish");
        assert_eq!(turn.continuation, Some(state.clone()));
        let AgentContentBlock::Reasoning(reasoning) = &turn.content[0] else {
            panic!("block 0 must be a reasoning block");
        };
        assert_eq!(reasoning.text, None);
        assert_eq!(reasoning.provider_state, Some(state));
    }

    /// The boundary continuation is the greatest block index carrying state.
    #[test]
    fn boundary_continuation_is_greatest_block_index() {
        let first = ProviderContinuationState::Anthropic(AnthropicContinuation {
            opaque: serde_json::json!({"signature": "sig-1"}),
        });
        let second = ProviderContinuationState::Anthropic(AnthropicContinuation {
            opaque: serde_json::json!({"signature": "sig-2"}),
        });
        let mut assembler = ModelEventAssembler::new();
        assembler.push(&ModelEvent::Started).expect("started");
        assembler
            .push(&ModelEvent::ContinuationState {
                block_index: ContentBlockIndex::new(0),
                state: first,
            })
            .expect("state 1");
        assembler
            .push(&ModelEvent::ContinuationState {
                block_index: ContentBlockIndex::new(1),
                state: second.clone(),
            })
            .expect("state 2");
        assembler
            .push(&completed(ModelFinishReason::Stop))
            .expect("completed");
        let turn = assembler.finish(&ModelFinishReason::Stop, None).expect("finish");
        assert_eq!(turn.continuation, Some(second));
    }

    /// A stream ending with a terminal event rejects everything after it.
    #[test]
    fn events_after_terminal_are_rejected() {
        let mut assembler = ModelEventAssembler::new();
        assembler.push(&ModelEvent::Started).expect("started");
        assembler
            .push(&completed(ModelFinishReason::Stop))
            .expect("completed");
        let error = assembler.push(&text(0, "late")).expect_err("rejected");
        assert!(matches!(error, RuntimeError::ContractViolation { .. }));
    }

    /// Content before `Started` and a second terminal are rejected.
    #[test]
    fn malformed_stream_openings_are_rejected() {
        let mut assembler = ModelEventAssembler::new();
        assert!(matches!(
            assembler.push(&text(0, "early")).expect_err("rejected"),
            RuntimeError::ContractViolation { .. }
        ));
        assembler.push(&ModelEvent::Started).expect("started");
        assembler
            .push(&completed(ModelFinishReason::Stop))
            .expect("completed");
        let mut second = ModelEventAssembler::new();
        assert!(matches!(
            second.push(&completed(ModelFinishReason::Stop)).expect_err("rejected"),
            RuntimeError::ContractViolation { .. }
        ));
    }

    /// A bare terminal `Failed` is a legal stream (request rejected before
    /// provider execution).
    #[test]
    fn bare_terminal_failed_is_legal() {
        let mut assembler = ModelEventAssembler::new();
        assembler
            .push(&ModelEvent::Failed {
                error: ModelError {
                    kind: ModelErrorKind::Timeout,
                    message: "boom".to_owned(),
                    retry_after_ms: None,
                    provider_code: None,
                },
            })
            .expect("legal rejected request");
        assert!(assembler.terminal());
        assert!(!assembler.started());
    }

    /// Tool-call deltas for unknown calls and duplicate completions are
    /// rejected.
    #[test]
    fn orphan_tool_call_deltas_are_rejected() {
        let mut assembler = ModelEventAssembler::new();
        assembler.push(&ModelEvent::Started).expect("started");
        assert!(matches!(
            assembler
                .push(&call_args(0, "ghost", "{}"))
                .expect_err("rejected"),
            RuntimeError::ContractViolation { .. }
        ));
        assembler.push(&call_start(0, "call-1")).expect("start");
        assembler
            .push(&call_done(0, "call-1", serde_json::json!({})))
            .expect("done");
        assert!(matches!(
            assembler
                .push(&call_done(0, "call-1", serde_json::json!({})))
                .expect_err("rejected"),
            RuntimeError::ContractViolation { .. }
        ));
    }

    /// A stream that ends with an unfinished tool call is rejected.
    #[test]
    fn unfinished_tool_call_is_rejected_at_finish() {
        let mut assembler = ModelEventAssembler::new();
        assembler.push(&ModelEvent::Started).expect("started");
        assembler.push(&call_start(0, "call-1")).expect("start");
        assembler
            .push(&completed(ModelFinishReason::ToolCalls))
            .expect("completed");
        let error = assembler
            .finish(&ModelFinishReason::ToolCalls, None)
            .expect_err("rejected");
        assert!(matches!(error, RuntimeError::ContractViolation { .. }));
    }

    /// A skipped block index is a contract violation, never a silent gap.
    #[test]
    fn skipped_block_indices_are_rejected() {
        let mut assembler = ModelEventAssembler::new();
        assembler.push(&ModelEvent::Started).expect("started");
        assert!(matches!(
            assembler.push(&text(2, "gap")).expect_err("rejected"),
            RuntimeError::ContractViolation { .. }
        ));
    }

    /// Usage updates are tracked and the terminal reported usage wins.
    #[test]
    fn usage_tracks_updates_with_terminal_winning() {
        let usage = ModelUsage {
            input_tokens: 1,
            output_tokens: 2,
            total_tokens: 3,
            details: None,
        };
        let mut assembler = ModelEventAssembler::new();
        assembler.push(&ModelEvent::Started).expect("started");
        assembler.push(&text(0, "hi")).expect("text");
        assembler
            .push(&ModelEvent::UsageUpdate {
                usage: usage.clone(),
            })
            .expect("usage");
        assembler.push(&text(0, "!")).expect("text");
        assembler
            .push(&completed(ModelFinishReason::Stop))
            .expect("completed");
        let turn = assembler
            .finish(&ModelFinishReason::Stop, Some(usage.clone()))
            .expect("finish");
        assert_eq!(turn.usage, Some(usage));
    }
}
