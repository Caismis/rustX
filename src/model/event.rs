//! Normalized adapter-to-kernel model streaming protocol.
//!
//! `ModelEvent` is a streaming fact from a model adapter (M2): text deltas,
//! reasoning deltas, tool-call assembly, usage updates, continuation state,
//! and final completion or failure. It is not the durable `RuntimeEvent`
//! journal and it is never placed into the canonical conversation history.
//! Only the completed generation becomes an `AgentMessageBlock`.

use serde::{Deserialize, Serialize};

use crate::model::error::ModelError;
use crate::model::finish::ModelFinishReason;
use crate::model::types::ModelUsage;
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::ToolCallId;
use crate::tools::types::ToolCall;

/// A normalized model streaming event.
///
/// M2 adapters convert provider streams into these events, and the agent
/// kernel assembles one final `AgentMessageBlock` from them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    /// The generation/request started.
    Started,
    /// A text delta of the generation.
    TextDelta {
        /// The incremental text.
        text: String,
    },
    /// A reasoning delta of the generation.
    ReasoningDelta {
        /// The incremental reasoning text.
        text: String,
    },
    /// A tool call started; `arguments` grows with argument deltas.
    ToolCallStarted {
        /// The tool call being assembled.
        call: ToolCall,
    },
    /// An incremental argument fragment for an in-flight tool call.
    ToolCallArgumentsDelta {
        /// Identity of the tool call being assembled.
        call_id: ToolCallId,
        /// The incremental JSON argument fragment.
        arguments_delta: String,
    },
    /// A tool call finished assembling.
    ToolCallCompleted {
        /// The completed tool call.
        call: ToolCall,
    },
    /// An incremental or final usage update.
    UsageUpdate {
        /// The updated usage.
        usage: ModelUsage,
    },
    /// Provider continuation state became available.
    ContinuationState {
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
    use crate::message::types::MessageBlock;
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
                    text: "hi".to_owned(),
                },
                "text_delta",
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
                        retry_after_ms: None,
                        provider_code: None,
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

    /// Continuation state is carried as a typed event payload, not text.
    #[test]
    fn continuation_state_survives_as_typed_payload() {
        let event = ModelEvent::ContinuationState {
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
        let json = r#"{"type":"text_delta","text":"hi"}"#;
        let event: ModelEvent = serde_json::from_str(json).expect("deserialize delta");
        assert!(matches!(event, ModelEvent::TextDelta { .. }));
        let result = serde_json::from_str::<MessageBlock>(json);
        assert!(
            result.is_err(),
            "a ModelEvent delta must not be a MessageBlock"
        );
    }
}
