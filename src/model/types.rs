//! Canonical model request and usage contracts.
//!
//! These types are provider-independent: they express the normalized
//! information the runtime owns, and future adapters translate them to and
//! from `OpenAI` Chat Completions, `OpenAI` Responses, and `Anthropic`
//! Messages. Provider SDK types never appear here.

use serde::{Deserialize, Serialize};

use crate::message::types::MessageBlock;
use crate::runtime::continuation::ProviderContinuationState;
use crate::tools::types::ToolDefinition;

/// The model interaction protocol an adapter must speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Reasoning effort configuration that belongs in canonical runtime
/// semantics rather than in a provider-specific option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// Minimal reasoning.
    Minimal,
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort (the default).
    #[default]
    Medium,
    /// High reasoning effort.
    High,
}

/// A canonical, provider-independent model request.
///
/// The runtime passes exactly the normalized information it owns: model
/// identity, canonical context, available tool definitions, reasoning
/// configuration, and optional provider continuation state. Provider
/// request schemas are adapter concerns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    /// Provider model identifier.
    pub model: String,
    /// Protocol the adapter must use.
    pub protocol: ModelProtocol,
    /// Canonical context/messages to send.
    pub messages: Vec<MessageBlock>,
    /// Tool definitions the model may call.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Reasoning effort for the generation.
    pub reasoning: ReasoningEffort,
    /// Maximum output tokens, when the runtime enforces one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Provider continuation state, when continuing an earlier generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ProviderContinuationState>,
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
    use super::{ModelProtocol, ModelUsage, ReasoningEffort, UsageDetails};

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

    /// Reasoning effort serializes with stable casing.
    #[test]
    fn reasoning_effort_round_trip() {
        let value = ReasoningEffort::High;
        let json = serde_json::to_string(&value).expect("serialize effort");
        assert_eq!(json, "\"high\"");
        let decoded: ReasoningEffort = serde_json::from_str(&json).expect("deserialize effort");
        assert_eq!(decoded, value);
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
