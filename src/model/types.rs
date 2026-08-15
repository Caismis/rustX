//! Canonical model request and usage contracts.
//!
//! These types are provider-independent: they express the normalized
//! information the runtime owns, and future adapters translate them to and
//! from `OpenAI` Chat Completions, `OpenAI` Responses, and `Anthropic`
//! Messages. Provider SDK types never appear here.
//!
//! Request-time context semantics are frozen by
//! [`crate::model::snapshot::RequestSnapshot`]. `ModelRequest` contains only
//! final provider-neutral values; it has no special Agent Status or Skill
//! semantic channels.

use serde::{Deserialize, Serialize};

use crate::message::types::MessageBlock;
use crate::model::invocation::ModelInvocationConfig;
use crate::runtime::continuation::ProviderContinuationState;
use crate::tools::types::ModelToolDefinition;

/// The model interaction protocol an adapter must speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProtocol {
    /// `OpenAI` Chat Completions API.
    #[serde(rename = "openai_chat_completions")]
    OpenAiChatCompletions,
    /// `OpenAI` Responses API.
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
    /// `Anthropic` Messages API.
    AnthropicMessages,
}

/// A canonical, provider-independent model request.
///
/// A request has final provider-neutral values plus an explicit effective
/// system prompt. The separation is deliberate:
///
/// - **canonical content** — the complete projected messages and compiled
///   model-facing tool definitions;
/// - **request-time system content** — the exact Effective System Prompt;
/// - **immutable resolved invocation configuration** —
///   [`ModelInvocationConfig`], the one channel through which provider wire
///   configuration reaches an adapter.
///
/// Provider request schemas remain adapter concerns, provider wire
/// parameters never enter canonical history or message types, and semantic
/// context is settled before an adapter is called.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    /// The immutable resolved invocation configuration of this request:
    /// model identity, protocol, output budget, opaque provider request
    /// parameters, effective capabilities, and structural compat metadata.
    pub invocation: ModelInvocationConfig,
    /// Canonical context/messages to send.
    pub messages: Vec<MessageBlock>,
    /// Compiled model-facing tool definitions the model may call. Runtime
    /// execution, replay, and origin policy never reach provider adapters.
    #[serde(default)]
    pub tools: Vec<ModelToolDefinition>,
    /// The exact request-time Effective System Prompt assembled by rustX.
    #[serde(default)]
    pub effective_system_prompt: String,
    /// Provider continuation state, when continuing an earlier generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ProviderContinuationState>,
}

impl ModelRequest {
    /// The provider-facing model identifier of this request.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.invocation.model
    }

    /// The protocol the adapter must speak.
    #[must_use]
    pub const fn protocol(&self) -> ModelProtocol {
        self.invocation.protocol
    }

    /// The runtime-resolved effective maximum output tokens.
    ///
    /// Real provider integration proved that an adapter cannot faithfully
    /// represent "no runtime output limit" when the provider requires an
    /// explicit generation maximum (Anthropic requires `max_tokens`). The
    /// runtime resolves the effective limit before the adapter boundary; no
    /// adapter-local default exists.
    #[must_use]
    pub const fn max_output_tokens(&self) -> u32 {
        self.invocation.max_output_tokens
    }

    /// The effective opaque provider request parameters.
    #[must_use]
    pub const fn request_params(&self) -> &crate::model::invocation::RequestParams {
        &self.invocation.request_params
    }
}

/// Normalized token accounting for one generation.
///
/// Providers do not expose identical token metrics; this is the stable
/// common core. Provider SDK usage objects never appear here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsage {
    /// Input tokens consumed by the request.
    pub input_tokens: u64,
    /// Output tokens produced by the response.
    pub output_tokens: u64,
    /// Total tokens, where the provider reports or can derive them.
    pub total_tokens: u64,
    /// Optional normalized usage details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<UsageDetails>,
}

/// Optional normalized token details where providers expose them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageDetails {
    /// Tokens consumed by reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// Input tokens served from cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::{ModelProtocol, ModelUsage, UsageDetails};
    /// Protocol discriminators are stable strings, never Rust debug output.
    #[test]
    fn protocol_discriminators_are_stable() {
        let cases = [
            (
                ModelProtocol::OpenAiChatCompletions,
                "openai_chat_completions",
            ),
            (ModelProtocol::OpenAiResponses, "openai_responses"),
            (ModelProtocol::AnthropicMessages, "anthropic_messages"),
        ];
        for (protocol, expected) in cases {
            let value = serde_json::to_value(protocol).expect("serialize protocol");
            assert_eq!(value, expected);
            let decoded: ModelProtocol =
                serde_json::from_value(value).expect("deserialize protocol");
            assert_eq!(decoded, protocol);
        }
    }

    /// Usage round-trips including optional details.
    #[test]
    fn usage_round_trip() {
        let usage = ModelUsage {
            input_tokens: 120,
            output_tokens: 42,
            total_tokens: 162,
            details: Some(UsageDetails {
                reasoning_tokens: Some(10),
                cached_input_tokens: Some(40),
            }),
        };
        let json = serde_json::to_string(&usage).expect("serialize usage");
        let decoded: ModelUsage = serde_json::from_str(&json).expect("deserialize usage");
        assert_eq!(decoded, usage);
    }
}
