//! Provider-neutral model request and usage contracts.
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

use crate::model::input::ModelInputMessage;
use crate::model::invocation::ModelInvocationConfig;
use crate::runtime::continuation::ProviderContinuationState;
use crate::tools::types::ModelToolDefinition;

/// The model interaction protocol an adapter must speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
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

/// A provider-neutral model request.
///
/// A request has final provider-neutral values plus an explicit effective
/// system prompt. The separation is deliberate:
///
/// - **ordered model input** — the complete projected canonical messages plus
///   any explicitly request-only context, and compiled model-facing tool
///   definitions;
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
    /// Frozen per-call output contract. Ordinary text requests leave this absent.
    /// Prompt preparation retains the schema for the same local validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<super::structured::PreparedResponseSchema>,
    /// Native per-response transport allocation budget, before provider parsing.
    /// It is neither provider input nor durable semantic evidence.
    #[serde(skip)]
    pub response_byte_limit: Option<usize>,
    /// Resolved managed images for transport only; never serialized into durable records.
    #[serde(skip)]
    pub images: crate::model::images::ResolvedImages,
    /// The immutable resolved invocation configuration of this request:
    /// model identity, protocol, output budget, opaque provider request
    /// parameters, effective capabilities, and structural compat metadata.
    pub invocation: ModelInvocationConfig,
    /// The already ordered provider-neutral input items to send. Canonical
    /// items retain their Ledger identity; request-only items are explicitly
    /// noncanonical and have no `MessageId`.
    pub messages: super::SelectedModelInput,
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
    /// Actual accepted output and covered input that authorize native continuation.
    pub continuation_source: Option<crate::runtime::continuation::AcceptedOutputContinuationSource>,
}

impl ModelRequest {
    /// Map the actual accepted output into this input and prove remote coverage.
    /// # Errors
    /// Missing/ambiguous output or reordered/omitted covered input fails explicitly.
    pub fn continuation_boundary(&self) -> Result<Option<usize>, String> {
        let Some((boundary, source)) = self.accepted_output_boundary()? else {
            return Ok(None);
        };
        if semantic_messages_fingerprint(&self.messages.range(0..boundary))
            != source.covered_input_fingerprint
        {
            return Err("continuation does not cover the compiled historical input".into());
        }
        Ok(Some(boundary))
    }

    /// Reconcile coverage after request-local semantic materialization. The adapter
    /// must independently validate complete native replay before snapshot freeze.
    pub(crate) fn reconcile_continuation(&mut self) -> Result<bool, String> {
        let Some((boundary, source)) = self.accepted_output_boundary()? else {
            return Ok(false);
        };
        if semantic_messages_fingerprint(&self.messages.range(0..boundary))
            == source.covered_input_fingerprint
        {
            return Ok(false);
        }
        self.continuation = None;
        self.continuation_source = None;
        Ok(true)
    }

    fn accepted_output_boundary(
        &self,
    ) -> Result<
        Option<(
            usize,
            &crate::runtime::continuation::AcceptedOutputContinuationSource,
        )>,
        String,
    > {
        let (Some(state), Some(source)) = (&self.continuation, &self.continuation_source) else {
            return if self.continuation.is_none() && self.continuation_source.is_none() {
                Ok(None)
            } else {
                Err("native continuation has no actual accepted output source".into())
            };
        };
        if state != &source.native_state {
            return Err("continuation state differs from the actual accepted native output".into());
        }
        let mut boundary = None;
        for (index, message) in self.messages.iter().enumerate() {
            let scoped = match message {
                ModelInputMessage::Canonical(message) => source.assistant.matches(message),
                ModelInputMessage::Derived(message) => message.sources.contains(&source.assistant),
                ModelInputMessage::RequestOnly(_) => false,
            };
            if scoped
                && semantic_messages_fingerprint(std::slice::from_ref(message))
                    == source.accepted_output_fingerprint
                && boundary.replace(index).is_some()
            {
                return Err("ambiguous accepted-output continuation mapping".into());
            }
        }
        let boundary =
            boundary.ok_or("continuation accepted output is absent or semantically changed")?;
        Ok(Some((boundary, source)))
    }
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
/// Missing provider metrics remain absent; a reported zero is `Some(0)`.
/// These are native u64 quantities, not byte offsets. Observation transports
/// use the existing exact decimal codec without narrowing provider evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModelUsage {
    /// Input tokens consumed by the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Output tokens produced by the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Total tokens, where the provider reports or can derive them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// Optional normalized usage details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<UsageDetails>,
}

impl ModelUsage {
    /// Whether all normalized core metrics were observed or exactly derived.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.input_tokens.is_some() && self.output_tokens.is_some() && self.total_tokens.is_some()
    }
}

/// Optional normalized token details where providers expose them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UsageDetails {
    /// Tokens consumed by reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// Input tokens served from cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
}

impl ModelRequest {
    /// SHA-256 of provider-visible semantic content, excluding execution identities.
    /// # Panics
    /// Panics only if the runtime-owned semantic fields cannot serialize.
    #[must_use]
    pub fn semantic_input_fingerprint(&self) -> String {
        self.semantic_input_fingerprint_with_messages(&self.messages.semantic_fingerprint())
    }

    pub(crate) fn semantic_input_fingerprint_with_messages(&self, messages: &str) -> String {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&(
            messages,
            &self.images,
            &self.effective_system_prompt,
            &self.tools,
            &self.invocation,
            &self.continuation,
            &self.response_schema,
        ))
        .expect("semantic input serializes");
        format!("{:x}", Sha256::digest(bytes))
    }
}

/// Ordered semantic content identity, independent of canonical/projected/request occurrence IDs.
/// # Panics
/// Runtime-owned semantic content must serialize.
#[must_use]
pub fn semantic_messages_fingerprint<'a>(
    messages: impl IntoIterator<Item = &'a ModelInputMessage>,
) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for message in messages {
        append_semantic_message(&mut digest, message);
    }
    format!("{:x}", digest.finalize())
}

pub(crate) fn append_semantic_message(digest: &mut sha2::Sha256, message: &ModelInputMessage) {
    use sha2::Digest;
    struct SemanticWriter<'a>(&'a mut sha2::Sha256);
    impl std::io::Write for SemanticWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            #[cfg(test)]
            super::selected_input::record_hash_work(0, bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let value = match message.view() {
        crate::model::projected::ModelMessageView::User(content) => {
            serde_json::json!({"role":"user", "content":content})
        }
        crate::model::projected::ModelMessageView::Assistant(content) => {
            serde_json::json!({"role":"assistant", "content":content})
        }
        crate::model::projected::ModelMessageView::Tool(call_id, result) => {
            match result.model_visible_content() {
                Ok(crate::tools::types::ModelToolResultContent::Parts(parts)) => {
                    serde_json::json!({"role":"tool", "call_id":call_id, "parts":parts})
                }
                Ok(crate::tools::types::ModelToolResultContent::Text(text)) => {
                    // Text-only identities remain unchanged; both consumers
                    // use the same normalized bounded Tool semantic content.
                    serde_json::json!({"role":"tool", "call_id":call_id, "text":text, "media":[]})
                }
                Err(_) => {
                    serde_json::json!({"role":"tool", "call_id":call_id, "unsupported_content":true})
                }
            }
        }
        crate::model::projected::ModelMessageView::RequestOnly(context) => {
            serde_json::json!({"role":"user", "text":context.render()})
        }
    };
    serde_json::to_writer(SemanticWriter(&mut *digest), &value).expect("semantic input serializes");
    digest.update([0]);
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
            input_tokens: Some(120),
            output_tokens: Some(42),
            total_tokens: Some(162),
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
