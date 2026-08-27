//! Normalized adapter-to-kernel model streaming protocol.
//!
//! `ModelEvent` is a streaming fact from a model adapter (M2): text deltas,
//! reasoning deltas, refusal deltas, tool-call assembly, usage updates,
//! continuation state, and final completion or failure. It is not the
//! durable `RuntimeEvent` journal and it is never placed into the canonical
//! conversation history. Only the completed generation becomes an
//! `AssistantMessageBlock`.
//!
//! Every content-targeted event carries a [`ContentBlockIndex`] identifying
//! the ordered output block it belongs to, so interleaved text, reasoning,
//! refusal, continuation-state, and tool-call content assemble
//! unambiguously without exposing any provider block id type.
//!
//! The stream distinguishes every content semantic the canonical message
//! model distinguishes: a refusal streams through [`ModelEvent::RefusalDelta`]
//! and assembles into
//! [`AssistantContentBlock::Refusal`](crate::message::types::AssistantContentBlock::Refusal),
//! never into plain text.

use serde::{Deserialize, Serialize};

use crate::message::types::ContentBlockIndex;
use crate::model::error::ModelError;
use crate::model::finish::ModelFinishReason;
use crate::model::types::ModelUsage;
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::ToolCallId;
use crate::tools::types::{ToolCall, ToolCallStart};

/// A normalized model streaming event.
///
/// M2 adapters convert provider streams into these events, and the agent
/// kernel assembles one final `AssistantMessageBlock` from them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    /// The generation/request started.
    Started,
    /// A text delta of one output block.
    TextDelta {
        /// The output block the delta belongs to.
        block_index: ContentBlockIndex,
        /// The incremental text.
        text: String,
    },
    /// A reasoning delta of one output block.
    ReasoningDelta {
        /// The reasoning block the delta belongs to.
        block_index: ContentBlockIndex,
        /// The incremental reasoning text.
        text: String,
    },
    /// A refusal delta of one output block.
    ///
    /// Refusal content streams as refusal, never as ordinary text, so the
    /// completed message can assemble an
    /// [`AssistantContentBlock::Refusal`](crate::message::types::AssistantContentBlock::Refusal)
    /// without provider-specific hidden state.
    RefusalDelta {
        /// The refusal block the delta belongs to.
        block_index: ContentBlockIndex,
        /// The incremental refusal text.
        text: String,
    },
    /// A tool call started; only data known at start is present.
    ToolCallStarted {
        /// The tool-call content block being assembled.
        block_index: ContentBlockIndex,
        /// The tool call identity, without streamed arguments yet.
        call: ToolCallStart,
    },
    /// An incremental argument fragment for an in-flight tool call.
    ToolCallArgumentsDelta {
        /// The tool-call content block being assembled.
        block_index: ContentBlockIndex,
        /// Identity of the tool call being assembled.
        call_id: ToolCallId,
        /// The incremental JSON argument fragment.
        arguments_delta: String,
    },
    /// A tool call finished assembling with its complete arguments.
    ToolCallCompleted {
        /// The tool-call content block that completed.
        block_index: ContentBlockIndex,
        /// The fully assembled tool call.
        call: ToolCall,
    },
    /// An incremental or final usage update.
    UsageUpdate {
        /// The updated usage.
        usage: ModelUsage,
    },
    /// Provider continuation state became available for one reasoning block.
    ContinuationState {
        /// The reasoning block the state belongs to. Providers may return
        /// opaque continuation/signature state even when no reasoning text
        /// is exposed.
        block_index: ContentBlockIndex,
        /// The provider continuation state to preserve.
        state: ProviderContinuationState,
    },
    /// The generation completed successfully.
    Completed {
        /// Why the generation finished.
        finish_reason: ModelFinishReason,
        /// Final usage, when reported.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<ModelUsage>,
    },
    /// The generation failed with a normalized error.
    Failed {
        /// The normalized model error.
        error: ModelError,
    },
}

#[cfg(test)]
mod tests {
    use super::ModelEvent;
    use crate::message::types::{AssistantContentBlock, ContentBlockIndex, MessageBlock};
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::ModelUsage;
    use crate::runtime::continuation::{
        AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
    };
    use crate::runtime::identity::{ToolCallId, ToolId};
    use crate::tools::types::{ToolCall, ToolCallStart};

    /// `ModelEvent` discriminators are stable strings.
    #[test]
    fn model_event_discriminators_are_stable() {
        let cases = [
            (ModelEvent::Started, "started"),
            (
                ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "hi".to_owned(),
                },
                "text_delta",
            ),
            (
                ModelEvent::RefusalDelta {
                    block_index: ContentBlockIndex::new(1),
                    text: "I cannot do that.".to_owned(),
                },
                "refusal_delta",
            ),
            (
                ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: Some(ModelUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                        total_tokens: 2,
                        details: None,
                    }),
                },
                "completed",
            ),
            (
                ModelEvent::Failed {
                    error: ModelError {
                        kind: ModelErrorKind::Timeout,
                        message: "timed out".to_owned(),
                        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                        retry_after_ms: None,
                        provider_code: None,
                        context_overflow: None,
                    },
                },
                "failed",
            ),
        ];
        for (event, expected) in cases {
            let value = serde_json::to_value(&event).expect("serialize event");
            assert_eq!(value["type"], expected);
            let decoded: ModelEvent = serde_json::from_value(value).expect("deserialize event");
            assert_eq!(decoded, event);
        }
    }

    /// Continuation state is carried as a typed, block-targeted payload.
    #[test]
    fn continuation_state_survives_as_typed_payload() {
        let event = ModelEvent::ContinuationState {
            block_index: ContentBlockIndex::new(0),
            state: ProviderContinuationState::Anthropic(AnthropicContinuation {
                opaque: serde_json::json!({"signature": "sig-1"}),
            }),
        };
        let json = serde_json::to_string(&event).expect("serialize event");
        let decoded: ModelEvent = serde_json::from_str(&json).expect("deserialize event");
        assert_eq!(decoded, event);
    }

    /// Streaming deltas are not conversation messages: a text delta cannot
    /// deserialize as a `MessageBlock`, which requires a `role` discriminator.
    #[test]
    fn model_event_deltas_are_not_message_blocks() {
        for json in [
            r#"{"type":"text_delta","block_index":0,"text":"hi"}"#,
            r#"{"type":"refusal_delta","block_index":1,"text":"I cannot do that."}"#,
        ] {
            let event: ModelEvent = serde_json::from_str(json).expect("deserialize delta");
            assert!(matches!(
                event,
                ModelEvent::TextDelta { .. } | ModelEvent::RefusalDelta { .. }
            ));
            let result = serde_json::from_str::<MessageBlock>(json);
            assert!(
                result.is_err(),
                "a ModelEvent delta must not be a MessageBlock"
            );
        }
    }

    /// Assembles ordered content blocks from a streamed event sequence.
    ///
    /// This is a minimal, test-local projection of what M2 assembly must
    /// produce; M1 defines the stream contract only.
    fn assemble(events: &[ModelEvent]) -> Vec<AssistantContentBlock> {
        let mut blocks: Vec<AssistantContentBlock> = Vec::new();
        let mut tool_calls: std::collections::BTreeMap<ToolCallId, (ToolCallStart, String)> =
            std::collections::BTreeMap::new();
        for event in events {
            match event {
                ModelEvent::TextDelta { block_index, text } => {
                    let idx = block_index.get() as usize;
                    if let Some(AssistantContentBlock::Text(block)) = blocks.get_mut(idx) {
                        block.text.push_str(text);
                    } else {
                        blocks.push(AssistantContentBlock::Text(
                            crate::message::content::TextBlock { text: text.clone() },
                        ));
                    }
                }
                ModelEvent::ReasoningDelta { block_index, text } => {
                    let idx = block_index.get() as usize;
                    if let Some(AssistantContentBlock::Reasoning(block)) = blocks.get_mut(idx) {
                        block.text.get_or_insert_with(String::new).push_str(text);
                    } else {
                        blocks.push(AssistantContentBlock::Reasoning(
                            crate::message::types::ReasoningBlock {
                                text: Some(text.clone()),
                                provider_state: None,
                            },
                        ));
                    }
                }
                ModelEvent::RefusalDelta { block_index, text } => {
                    let idx = block_index.get() as usize;
                    if let Some(AssistantContentBlock::Refusal(block)) = blocks.get_mut(idx) {
                        block.text.push_str(text);
                    } else {
                        blocks.push(AssistantContentBlock::Refusal(
                            crate::message::types::RefusalBlock { text: text.clone() },
                        ));
                    }
                }
                ModelEvent::ContinuationState { block_index, state } => {
                    let idx = block_index.get() as usize;
                    if idx == blocks.len() {
                        // Providers may expose continuation state before any
                        // reasoning delta; the reasoning block is created
                        // implicitly at the declared index.
                        blocks.push(AssistantContentBlock::Reasoning(
                            crate::message::types::ReasoningBlock {
                                text: None,
                                provider_state: Some(state.clone()),
                            },
                        ));
                    } else if let Some(AssistantContentBlock::Reasoning(block)) =
                        blocks.get_mut(idx)
                    {
                        block.provider_state = Some(state.clone());
                    }
                }
                ModelEvent::ToolCallStarted {
                    block_index: _,
                    call,
                } => {
                    tool_calls.insert(call.id.clone(), (call.clone(), String::new()));
                    blocks.push(AssistantContentBlock::ToolCall(ToolCall {
                        id: call.id.clone(),
                        tool_id: call.tool_id.clone(),
                        name: call.name.clone(),
                        arguments: serde_json::Value::Object(serde_json::Map::new()),
                    }));
                }
                ModelEvent::ToolCallArgumentsDelta {
                    block_index: _,
                    call_id,
                    arguments_delta,
                } => {
                    if let Some((_, buffer)) = tool_calls.get_mut(call_id) {
                        buffer.push_str(arguments_delta);
                    }
                }
                ModelEvent::ToolCallCompleted { block_index, call } => {
                    let idx = block_index.get() as usize;
                    blocks[idx] = AssistantContentBlock::ToolCall(call.clone());
                }
                _ => {}
            }
        }
        blocks
    }

    /// Interleaved reasoning, tool-call, reasoning, and text blocks assemble
    /// in the order they appeared, with state attached to the right block.
    #[test]
    fn interleaved_blocks_assemble_in_order() {
        let call_start = ToolCallStart {
            id: ToolCallId::new("call_01"),
            tool_id: ToolId::new("tool-list"),
            name: "list_directory".to_owned(),
        };
        let events = [
            ModelEvent::ReasoningDelta {
                block_index: ContentBlockIndex::new(0),
                text: "First reasoning.".to_owned(),
            },
            ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(1),
                call: call_start.clone(),
            },
            ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(1),
                call_id: call_start.id.clone(),
                arguments_delta: r#"{"path":"."}"#.to_owned(),
            },
            ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(1),
                call: ToolCall {
                    id: call_start.id.clone(),
                    tool_id: call_start.tool_id.clone(),
                    name: call_start.name.clone(),
                    arguments: serde_json::json!({"path": "."}),
                },
            },
            ModelEvent::ReasoningDelta {
                block_index: ContentBlockIndex::new(2),
                text: "Second reasoning after the tool call.".to_owned(),
            },
            ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(3),
                text: "Final answer.".to_owned(),
            },
        ];
        let blocks = assemble(&events);
        assert_eq!(blocks.len(), 4, "exactly four blocks in order");
        assert!(matches!(
            &blocks[0],
            AssistantContentBlock::Reasoning(r)
                if r.text.as_deref() == Some("First reasoning.") && r.provider_state.is_none()
        ));
        assert!(matches!(
            &blocks[1],
            AssistantContentBlock::ToolCall(c) if c.id.as_str() == "call_01" && c.arguments == serde_json::json!({"path": "."})
        ));
        assert!(matches!(
            &blocks[2],
            AssistantContentBlock::Reasoning(r)
                if r.text.as_deref() == Some("Second reasoning after the tool call.")
        ));
        assert!(matches!(
            &blocks[3],
            AssistantContentBlock::Text(t) if t.text == "Final answer."
        ));
    }

    /// Opaque reasoning continuation state survives even when no reasoning
    /// text is exposed by the provider.
    #[test]
    fn reasoning_state_without_visible_text_assembles() {
        let events = [
            ModelEvent::ContinuationState {
                block_index: ContentBlockIndex::new(0),
                state: ProviderContinuationState::OpenAiResponses(
                    OpenAiResponsesContinuation::Stateless {
                        items: vec![serde_json::json!({
                            "type": "reasoning",
                            "id": "rs_1",
                            "summary": [],
                            "encrypted_content": "opaque"
                        })],
                    },
                ),
            },
            ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(1),
                text: "Visible answer.".to_owned(),
            },
        ];
        let blocks = assemble(&events);
        assert_eq!(blocks.len(), 2);
        let AssistantContentBlock::Reasoning(reasoning) = &blocks[0] else {
            panic!("block 0 must be a reasoning block");
        };
        assert_eq!(reasoning.text, None, "no visible reasoning text");
        assert!(matches!(
            reasoning.provider_state,
            Some(ProviderContinuationState::OpenAiResponses(
                OpenAiResponsesContinuation::Stateless { .. }
            ))
        ));
        assert!(
            matches!(&blocks[1], AssistantContentBlock::Text(t) if t.text == "Visible answer.")
        );
    }

    /// Reasoning followed by refusal assembles in order, and the refusal
    /// becomes `AssistantContentBlock::Refusal`, never `Text`.
    #[test]
    fn reasoning_then_refusal_assembles_in_order() {
        let events = [
            ModelEvent::ReasoningDelta {
                block_index: ContentBlockIndex::new(0),
                text: "The request cannot be satisfied.".to_owned(),
            },
            ModelEvent::RefusalDelta {
                block_index: ContentBlockIndex::new(1),
                text: "I cannot comply with that request.".to_owned(),
            },
        ];
        let blocks = assemble(&events);
        assert_eq!(blocks.len(), 2, "reasoning and refusal remain two blocks");
        assert!(matches!(
            &blocks[0],
            AssistantContentBlock::Reasoning(r)
                if r.text.as_deref() == Some("The request cannot be satisfied.")
        ));
        assert!(matches!(
            &blocks[1],
            AssistantContentBlock::Refusal(r) if r.text == "I cannot comply with that request."
        ));
        assert!(
            !matches!(&blocks[1], AssistantContentBlock::Text(_)),
            "refusal must never assemble as plain text"
        );
    }

    /// Multiple refusal deltas targeting the same block concatenate
    /// deterministically into one `RefusalBlock`.
    #[test]
    fn multiple_refusal_deltas_concatenate() {
        let events = [
            ModelEvent::RefusalDelta {
                block_index: ContentBlockIndex::new(0),
                text: "I cannot".to_owned(),
            },
            ModelEvent::RefusalDelta {
                block_index: ContentBlockIndex::new(0),
                text: " comply with".to_owned(),
            },
            ModelEvent::RefusalDelta {
                block_index: ContentBlockIndex::new(0),
                text: " that request.".to_owned(),
            },
        ];
        let blocks = assemble(&events);
        assert_eq!(blocks.len(), 1);
        let AssistantContentBlock::Refusal(refusal) = &blocks[0] else {
            panic!("the block must be a refusal block");
        };
        assert_eq!(
            refusal.text, "I cannot comply with that request.",
            "refusal deltas must concatenate in stream order"
        );
    }
}
