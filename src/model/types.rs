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

pub use rustx_types::model::types::{ModelProtocol, ModelUsage, UsageDetails};

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
        if super::adapter::cache::prefix_fingerprint(&self.messages, boundary)
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
        if super::adapter::cache::prefix_fingerprint(&self.messages, boundary)
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
        for index in super::adapter::cache::continuation_candidates(
            &self.messages,
            &source.assistant.message_id,
        ) {
            let message = &self.messages[index];
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
impl ModelRequest {
    /// SHA-256 of provider-visible semantic content, excluding execution identities.
    /// # Panics
    /// Panics only if the runtime-owned semantic fields cannot serialize.
    #[must_use]
    pub fn semantic_input_fingerprint(&self) -> String {
        self.semantic_input_fingerprint_with_messages(&self.messages.semantic_fingerprint())
    }

    pub(crate) fn semantic_input_fingerprint_with_messages(&self, messages: &str) -> String {
        fingerprint_metadata(&(
            messages,
            &self.images,
            &self.effective_system_prompt,
            &self.tools,
            &self.invocation,
            &self.continuation,
            &self.response_schema,
        ))
    }
}

// The tuple is already in durable fingerprint order. Serialize directly into
// the digest, including opaque values whose key order is part of the bytes.
pub(super) fn fingerprint_metadata(fields: &impl Serialize) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    serde_json::to_writer(&mut digest, fields).expect("semantic input serializes");
    format!("{:x}", digest.finalize())
}
#[cfg(test)]
pub(crate) use rustx_types::model::types::SemanticView;
pub(crate) use rustx_types::model::types::append_semantic_message;
pub use rustx_types::model::types::semantic_messages_fingerprint;

#[cfg(test)]
mod tests {
    use super::{ModelProtocol, ModelUsage, UsageDetails};
    #[test]
    fn borrowed_fingerprint_bytes_match_the_original_value_representation() {
        use crate::message::content::{FileReference, ImageReference, TextBlock, UploadedFileRef};
        use crate::message::types::{AssistantContentBlock, ReasoningBlock, UserContentBlock};
        use crate::model::input::RequestOnlyModelContext;
        use crate::model::projected::ModelMessageView;
        use crate::runtime::continuation::{
            OpenAiResponsesContinuation, ProviderContinuationState,
        };
        use crate::runtime::identity::{ArtifactId, ToolCallId, ToolId};
        use crate::tools::types::{
            ModelToolResultContent, ToolCall, ToolExecutionResult, ToolResultContent,
        };
        use serde_json::json;

        let image = ImageReference {
            artifact_id: ArtifactId::new("art-image"),
            alt: Some("水\n\"".into()),
        };
        let file = FileReference {
            artifact_id: ArtifactId::new("art-file"),
            name: Some("doc".into()),
            mime_type: Some("text/plain".into()),
            description: Some("details".into()),
        };
        let user = [
            UserContentBlock::UploadedFile(UploadedFileRef {
                batch_id: "batch".into(),
                name: "file.txt".into(),
            }),
            UserContentBlock::Text(TextBlock {
                text: "hi\n\u{1b}水".into(),
            }),
            UserContentBlock::Image(image.clone()),
            UserContentBlock::File(file.clone()),
        ];
        let assistant = [
            AssistantContentBlock::Reasoning(ReasoningBlock {
                origin: None,
                text: Some("reason".into()),
                provider_state: Some(ProviderContinuationState::OpenAiResponses(
                    OpenAiResponsesContinuation::Stateless {
                        items: std::sync::Arc::new(vec![json!({"z": [-0.0, 1.25], "a": "\t"})]),
                    },
                )),
            }),
            AssistantContentBlock::ToolCall(ToolCall {
                id: ToolCallId::new("call-id"),
                tool_id: ToolId::new("tool-id"),
                name: "tool".into(),
                arguments: json!({"z": 9_007_199_254_740_993_u64, "a": [true, 1e-12]}),
            }),
            AssistantContentBlock::Image(image.clone()),
        ];
        let temporary = RequestOnlyModelContext::AuxiliaryAnswer {
            text: "reply\n水".into(),
        };
        for (view, expected) in [
            (
                ModelMessageView::User(&user),
                json!({"role": "user", "content": user}),
            ),
            (
                ModelMessageView::Assistant(&assistant),
                json!({"role": "assistant", "content": assistant}),
            ),
            (
                ModelMessageView::RequestOnly(&temporary),
                json!({"role": "assistant", "text": temporary.render()}),
            ),
        ] {
            assert_eq!(
                serde_json::to_vec(&super::SemanticView(&view)).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
        }

        let id = ToolCallId::new("call-id");
        for content in [
            vec![ToolResultContent::Json {
                value: json!({"z": 1, "a": 2}),
            }],
            vec![ToolResultContent::Image(image)],
            vec![ToolResultContent::File(file)],
        ] {
            let result: ToolExecutionResult = serde_json::from_value(json!({
                "status": {"type": "success"}, "content": content, "duration_ms": 0
            }))
            .unwrap();
            let expected = match result.model_visible_content() {
                Ok(ModelToolResultContent::Parts(parts)) => {
                    json!({"role": "tool", "call_id": id, "parts": parts})
                }
                Ok(ModelToolResultContent::Text(text)) => {
                    json!({"role": "tool", "call_id": id, "text": text, "media": []})
                }
                Err(_) => json!({"role": "tool", "call_id": id, "unsupported_content": true}),
            };
            let view = ModelMessageView::Tool(&id, &result);
            assert_eq!(
                serde_json::to_vec(&super::SemanticView(&view)).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
        }
    }

    #[test]
    fn streaming_metadata_digest_matches_the_original_buffered_digest() {
        use serde_json::json;
        use sha2::Digest;

        let metadata = (
            "messages",
            json!({"z": "水\n", "a": [1.25, 9_007_199_254_740_993_u64]}),
        );
        let before = sha2::Sha256::digest(serde_json::to_vec(&metadata).unwrap());
        assert_eq!(
            super::fingerprint_metadata(&metadata),
            format!("{before:x}")
        );
    }

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

#[cfg(test)]
mod continuation_work_tests {
    use super::*;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, MessageBlock, ReasoningBlock,
    };
    use crate::model::projected::CanonicalMessageRef;
    use crate::model::selected_input::take_hash_work;
    use crate::runtime::continuation::{
        AcceptedOutputContinuationSource, AnthropicContinuation, ProviderContinuationState,
    };
    use crate::runtime::identity::{ConversationId, MessageId, RequestId};

    #[test]
    fn continuation_hashes_only_identity_candidates_and_checks_the_prefix() {
        let mut request = crate::scripted_suites::common::simple_request(
            ModelProtocol::AnthropicMessages,
            "fixture",
            "prefix",
        );
        let state = ProviderContinuationState::Anthropic(AnthropicContinuation {
            opaque: serde_json::json!({"type":"redacted_thinking","data":"a".repeat(8192)}).into(),
        });
        let output = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("accepted"),
            content: vec![AssistantContentBlock::Reasoning(ReasoningBlock {
                origin: None,
                text: None,
                provider_state: Some(state.clone()),
            })],
        });
        let scope = ConversationId::generate();
        let source = AcceptedOutputContinuationSource::from_accepted_output(
            RequestId::new("previous"),
            &scope,
            request.messages.semantic_fingerprint(),
            &output,
        )
        .unwrap();
        request.continuation = Some(state);
        request.continuation_source = Some(source);
        request
            .messages
            .push(ModelInputMessage::Canonical(output.clone()));
        for index in 0..64 {
            let MessageBlock::Assistant(mut unrelated) = output.clone() else {
                unreachable!()
            };
            unrelated.id = MessageId::new(format!("unrelated-{index}"));
            request
                .messages
                .push(ModelInputMessage::Canonical(MessageBlock::Assistant(
                    unrelated,
                )));
        }
        take_hash_work();
        assert_eq!(request.continuation_boundary().unwrap(), Some(1));
        let (original, semantic) = take_hash_work();
        assert_eq!(original, serde_json::to_vec(&output).unwrap().len());
        assert!(semantic > 8192);
        let mut changed = request.clone();
        changed.messages[0] = crate::scripted_suites::common::simple_request(
            ModelProtocol::AnthropicMessages,
            "fixture",
            "changed prefix",
        )
        .messages[0]
            .clone();
        assert!(changed.continuation_boundary().is_err());
        request
            .messages
            .push(ModelInputMessage::Canonical(output.clone()));
        assert!(
            request
                .continuation_boundary()
                .unwrap_err()
                .contains("ambiguous")
        );
        let MessageBlock::Assistant(mut wrong) = output.clone() else {
            unreachable!()
        };
        wrong.content.clear();
        assert!(!CanonicalMessageRef::of(&scope, &output).matches(&MessageBlock::Assistant(wrong)));
    }
}
