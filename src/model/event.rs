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
    /// The generation completed successfully. The canonical assembler retains
    /// this event's finish reason and usage as the sole successful terminal
    /// authority for the assembled turn.
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
    use crate::message::types::{ContentBlockIndex, MessageBlock};
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::ModelUsage;
    use crate::runtime::continuation::{AnthropicContinuation, ProviderContinuationState};

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
                        input_tokens: Some(1),
                        output_tokens: Some(1),
                        total_tokens: Some(2),
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
                        malformed_tool_proposal: None,
                        timeout_phase: None,
                        generation: None,
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
                opaque: serde_json::json!({"signature": "sig-1"}).into(),
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
}
