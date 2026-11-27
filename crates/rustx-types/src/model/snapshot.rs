use crate::runtime::identity::{AttemptId, AuxiliaryCallId, MessageId, RequestId, TurnId};
use serde::{Deserialize, Serialize};

/// The identity of one actual primary request attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestIdentity {
    /// The attempt that owns this request.
    pub attempt_id: AttemptId,
    /// The logical turn/step within that attempt.
    pub turn: TurnId,
    /// The shared monotonically increasing actual-request ordinal within the
    /// logical model step. Zero is the initial request; transient and
    /// context-overflow recovery requests share the same sequence.
    pub retry_number: u32,
}

impl RequestIdentity {
    /// Derives the stable durable identity of this actual request.
    #[must_use]
    pub fn request_id(&self) -> RequestId {
        RequestId::new(format!(
            "request:{}:{}:{}:{}:{}",
            self.attempt_id.as_str().len(),
            self.attempt_id,
            self.turn.as_str().len(),
            self.turn,
            self.retry_number
        ))
    }

    /// Derives the provisional Assistant message identity frozen for this
    /// request generation.
    ///
    /// The Request Snapshot is the authority for this mapping. Publication
    /// streams and canonical acceptance must use the value frozen here rather
    /// than deriving a second identity from live Agent Loop state.
    #[must_use]
    pub fn provisional_message_id(&self) -> MessageId {
        if self.retry_number == 0 {
            MessageId::new(format!("{}-agent-{}", self.attempt_id, self.turn))
        } else {
            MessageId::new(format!(
                "{}-agent-{}-retry-{}",
                self.attempt_id, self.turn, self.retry_number
            ))
        }
    }
}

/// Ownership of one actual model request; identity grants no execution permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelRequestOwner {
    /// A primary request owned by an Agent attempt.
    Agent { identity: RequestIdentity },
    /// One physical request within a logical auxiliary call.
    Auxiliary {
        call_id: AuxiliaryCallId,
        /// Zero is the initial request. Physical retries have distinct request
        /// identities and advance this ordinal within the same auxiliary call.
        ordinal: u32,
    },
}

/// Test fixtures shared by the types crate's own suites and downstream crates
/// that enable `test-fixtures`.
#[cfg(any(test, feature = "test-fixtures"))]
pub mod tests {
    /// The canonical text-only invocation configuration of an adapter test.
    #[must_use]
    pub fn invocation_for(
        protocol: crate::model::types::ModelProtocol,
        model: &str,
    ) -> crate::model::invocation_config::ModelInvocationConfig {
        let chat_reasoning_replay = (protocol
            == crate::model::types::ModelProtocol::OpenAiChatCompletions)
            .then_some(crate::model::capabilities::ChatReasoningReplay::Omit);
        crate::model::invocation_config::ModelInvocationConfig {
            replay_binding: Some("test-binding".into()),
            model: model.to_owned(),
            protocol,
            max_output_tokens: 512,
            request_params: crate::model::request_params::RequestParams::new(),
            capabilities: crate::model::capabilities::ModelCapabilities::text_only(),
            compat: crate::model::capabilities::ModelCompat {
                chat_reasoning_replay,
                ..Default::default()
            },
        }
    }

    /// A canonical user-only request for the given protocol.
    #[must_use]
    pub fn simple_request(
        protocol: crate::model::types::ModelProtocol,
        model: &str,
        prompt: &str,
    ) -> crate::model::types::ModelRequest {
        use crate::message::content::TextBlock;
        use crate::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
        use crate::runtime::identity::MessageId;
        crate::model::types::ModelRequest {
            prompt_fallback_authorization:
                crate::model::types::PromptFallbackAuthorization::default(),
            response_schema: None,
            response_byte_limit: None,
            continuation_source: None,
            images: std::collections::BTreeMap::new(),
            invocation: invocation_for(protocol, model),
            messages: vec![crate::model::input::ModelInputMessage::Canonical(
                MessageBlock::User(UserMessageBlock {
                    id: MessageId::new("msg-user-1"),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: prompt.to_owned(),
                    })],
                    source: UserSource::Human,
                    kind: crate::message::types::InboundKind::Message,
                    timestamp: None,
                }),
            )]
            .into(),
            tools: Vec::new(),
            effective_system_prompt: String::new(),
            continuation: None,
        }
    }
}
