//! Deterministic local capability validation for model requests.
//!
//! This is the runtime/model invocation boundary: everything rustX can know
//! locally is decided here, **before** a provider request is opened, so the
//! provider is never the first validator of a known rustX mismatch. It
//! validates protocol agreement, effective-capability agreement with the
//! canonical content of the request, continuation variant agreement, and
//! tool identity integrity. Provider-specific restrictions that cannot be
//! known locally still surface later as normalized `InvalidRequest` or
//! `Unsupported` provider errors.

use std::collections::BTreeMap;

use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::invocation::validate_content_modalities;
use crate::model::types::{ModelProtocol, ModelRequest};
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::ToolId;

/// Tools validated for one request, resolved by model-facing name.
#[derive(Debug, Clone, Default)]
pub struct ValidatedTools {
    by_name: BTreeMap<String, ToolId>,
    pub(super) continuation_boundary: Option<usize>,
}

impl ValidatedTools {
    /// The tool names in deterministic (sorted) order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    /// Whether the request exposed no tool to the model at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Resolves a model-facing tool name to its canonical [`ToolId`].
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<&ToolId> {
        self.by_name.get(name)
    }
}

/// Validates everything rustX can know locally about a request for the given
/// protocol.
///
/// Returns the resolved tool table on success, or a normalized
/// [`ModelError`]. Validation failures happen before any provider request.
///
/// # Errors
///
/// Returns `InvalidRequest` when the request protocol does not match the
/// adapter protocol, when the continuation variant does not match the
/// protocol, or when tool identities are empty or ambiguous.
pub fn validate_request(
    request: &ModelRequest,
    protocol: ModelProtocol,
) -> Result<ValidatedTools, ModelError> {
    #[cfg(feature = "issue-387-profile")]
    let _profile = super::profile::phase(super::profile::Phase::Validate);
    if request.protocol() != protocol {
        return Err(invalid_request(format!(
            "request protocol {} does not match adapter protocol {}",
            serde_name(request.protocol()),
            serde_name(protocol)
        )));
    }

    super::structured::validate_request(request)?;

    // Effective-capability agreement is checked before anything else that
    // could reach the network: content the invocation cannot represent is
    // rejected here, not by the provider.
    {
        #[cfg(feature = "issue-387-profile")]
        let _profile = super::profile::phase(super::profile::Phase::Modalities);
        #[cfg(feature = "issue-387-profile")]
        let messages = request.messages.iter().enumerate().map(|(index, message)| {
            super::profile::item(index);
            message
        });
        #[cfg(not(feature = "issue-387-profile"))]
        let messages = &request.messages;
        validate_content_modalities(messages, &request.invocation.capabilities)?;
    }
    if request.images.len() > 16 || {
        #[cfg(feature = "issue-387-profile")]
        let _profile = super::profile::phase(super::profile::Phase::ImageCount);
        #[cfg(feature = "issue-387-profile")]
        let messages = request.messages.iter().enumerate().map(|(index, message)| {
            super::profile::item(index);
            message
        });
        #[cfg(not(feature = "issue-387-profile"))]
        let messages = &request.messages;
        crate::model::images::image_count(messages) > 16
    } {
        return Err(invalid_request(
            "request exceeds the 16-image limit".to_owned(),
        ));
    }
    if !request.images.is_empty()
        && !request
            .invocation
            .capabilities
            .input_modalities
            .contains(&crate::model::catalog::Modality::Image)
    {
        return Err(invalid_request(
            "resolved images require an image-capable supported invocation".to_owned(),
        ));
    }

    {
        #[cfg(feature = "issue-387-profile")]
        let _profile = super::profile::phase(super::profile::Phase::Continuation);
        validate_continuation(request, protocol)?;
    }
    // Wire coverage/correlation is part of deterministic adapter admission,
    // including accepted stored calls absent from this request's explicit input.
    let boundary = request.continuation_boundary().map_err(invalid_request)?;
    super::correlation::request_labels_at(request, boundary).map_err(invalid_request)?;
    let mut tools = validate_tools(request)?;
    tools.continuation_boundary = boundary;
    Ok(tools)
}

#[cfg(test)]
thread_local! { static PREFLIGHTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn take_preflights() -> usize {
    PREFLIGHTS.with(|v| v.replace(0))
}
/// One immutable request and its transport bytes. Only the adapter boundary
/// constructs this value; cloning a logical retry shares both allocations.
#[derive(Clone)]
pub struct PreparedModelRequest {
    pub(super) input: std::sync::Arc<ModelRequest>,
    pub(super) body: bytes::Bytes,
    pub(super) tools: std::sync::Arc<ValidatedTools>,
}

impl PreparedModelRequest {
    pub(crate) fn input(&self) -> &ModelRequest {
        &self.input
    }
}

#[cfg(test)]
thread_local! {
    static PREPARATION_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
pub(crate) fn take_preparation_work() -> (usize, usize) {
    PREPARATION_WORK.with(|work| work.replace((0, 0)))
}

pub(crate) fn prepare_request(
    request: ModelRequest,
    protocol: ModelProtocol,
) -> Result<PreparedModelRequest, ModelError> {
    #[cfg(feature = "issue-387-profile")]
    let _request = super::profile::RequestScope::new(protocol, "prepare");
    #[cfg(test)]
    PREFLIGHTS.with(|v| v.set(v.get() + 1));
    let tools = validate_request(&request, protocol)?;
    let wire = translate_prepared(&request, &tools)?;
    #[cfg(feature = "issue-387-profile")]
    let _encoding = super::profile::phase(super::profile::Phase::HttpEncode);
    #[cfg(test)]
    PREPARATION_WORK.with(|work| work.update(|(translated, encoded)| (translated, encoded + 1)));
    let body = serde_json::to_vec(&wire)
        .map_err(|error| invalid_request(format!("failed to encode provider request: {error}")))?;
    Ok(PreparedModelRequest {
        input: std::sync::Arc::new(request),
        body: body.into(),
        tools: std::sync::Arc::new(tools),
    })
}

/// Adapter-owned preflight, including opaque native replay, without dispatch.
/// No SDK or wire values escape the adapter boundary.
#[cfg(test)]
pub(crate) fn validate_prepared_request(request: &ModelRequest) -> Result<(), ModelError> {
    prepared_wire_request(request).map(|_| ())
}

/// Count the actual final translated shape, including nested JSON escaping and
/// base64 images, before durable admission. The frozen retry input translates
/// identically; no SDK/wire value escapes this adapter-owned boundary.
pub(crate) fn validate_bounded_prepared_request(
    request: &ModelRequest,
    limit: usize,
) -> Result<usize, ModelError> {
    let translated = prepared_wire_request(request)?;
    crate::context::resources::bounded_encoded_len(&translated, limit).map_err(|_| {
        let mut error = invalid_request("translated provider request exceeds byte budget".into());
        error.kind = ModelErrorKind::ContextWindowExceeded;
        error
    })
}

#[derive(serde::Serialize)]
#[serde(untagged)]
enum PreparedWireRequest<'a> {
    Chat(super::openai::chat_completions::WireRequest<'a>),
    Responses(super::openai::responses::WireRequest<'a>),
    Anthropic(super::anthropic::mapping::WireRequest<'a>),
}

fn prepared_wire_request(request: &ModelRequest) -> Result<PreparedWireRequest<'_>, ModelError> {
    #[cfg(test)]
    PREFLIGHTS.with(|v| v.set(v.get() + 1));
    #[cfg(feature = "issue-387-profile")]
    let _request = super::profile::RequestScope::new(request.protocol(), "preflight");
    let tools = validate_request(request, request.protocol())?;
    translate_prepared(request, &tools)
}

fn translate_prepared<'a>(
    request: &'a ModelRequest,
    tools: &ValidatedTools,
) -> Result<PreparedWireRequest<'a>, ModelError> {
    #[cfg(test)]
    PREPARATION_WORK.with(|work| work.update(|(translated, encoded)| (translated + 1, encoded)));
    match request.protocol() {
        ModelProtocol::OpenAiChatCompletions => {
            super::openai::chat_completions::translate_request(request)
                .map(PreparedWireRequest::Chat)
        }
        ModelProtocol::OpenAiResponses => super::openai::responses::translate_request(
            request,
            tools,
            request.invocation.compat.responses_storage,
        )
        .map(PreparedWireRequest::Responses),
        ModelProtocol::AnthropicMessages => {
            super::anthropic::mapping::translate_request(request, tools)
                .map(PreparedWireRequest::Anthropic)
        }
    }
}

fn validate_continuation(
    request: &ModelRequest,
    protocol: ModelProtocol,
) -> Result<(), ModelError> {
    match (&request.continuation, protocol) {
        (None, _) => Ok(()),
        (Some(_state), ModelProtocol::OpenAiChatCompletions) => Err(invalid_request(
            "OpenAI Chat Completions has no Responses-style continuation; \
             continuation state must be None"
                .to_owned(),
        )),
        (Some(state), ModelProtocol::OpenAiResponses) => {
            if matches!(state, ProviderContinuationState::OpenAiResponses(_)) {
                Ok(())
            } else {
                Err(invalid_request(
                    "continuation state must be OpenAiResponses for the \
                     OpenAiResponses protocol"
                        .to_owned(),
                ))
            }
        }
        (Some(state), ModelProtocol::AnthropicMessages) => {
            if matches!(state, ProviderContinuationState::Anthropic(_)) {
                Ok(())
            } else {
                Err(invalid_request(
                    "continuation state must be Anthropic for the \
                     AnthropicMessages protocol"
                        .to_owned(),
                ))
            }
        }
    }
}

fn validate_tools(request: &ModelRequest) -> Result<ValidatedTools, ModelError> {
    #[cfg(feature = "issue-387-profile")]
    let _profile = super::profile::phase(super::profile::Phase::Tools);
    let mut by_name: BTreeMap<String, ToolId> = BTreeMap::new();
    for tool in &request.tools {
        if tool.name.is_empty() {
            return Err(invalid_request(
                "tool definitions must carry a non-empty model-facing name".to_owned(),
            ));
        }
        if tool.id.as_str().is_empty() {
            return Err(invalid_request(format!(
                "tool {:?} must carry a non-empty ToolId",
                tool.name
            )));
        }
        if by_name.insert(tool.name.clone(), tool.id.clone()).is_some() {
            return Err(invalid_request(format!(
                "duplicate model-facing tool name {:?}; tool names must be \
                 unambiguous before a request is sent",
                tool.name
            )));
        }
    }
    Ok(ValidatedTools {
        by_name,
        continuation_boundary: None,
    })
}

fn invalid_request(message: String) -> ModelError {
    ModelError {
        kind: ModelErrorKind::InvalidRequest,
        message,
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
        malformed_tool_proposal: None,
        timeout_phase: None,
        generation: None,
    }
}

fn serde_name(protocol: ModelProtocol) -> String {
    serde_json::to_string(&protocol).unwrap_or_else(|_| format!("{protocol:?}"))
}

#[cfg(test)]
mod tests {
    use super::validate_request;
    use crate::model::error::ModelErrorKind;
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::continuation::{
        AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
    };
    use crate::runtime::identity::ToolId;
    use crate::tools::types::ModelToolDefinition;

    #[test]
    fn classification_history_uses_data_in_all_actual_provider_requests() {
        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
            ModelProtocol::AnthropicMessages,
        ] {
            for (index, source) in
                crate::scripted_suites::support::fake::classification_history_messages()
                    .iter()
                    .enumerate()
            {
                let input = crate::runtime::auxiliary::classification::prepare_history(source);
                if index == 2 {
                    assert!(
                        input.is_err(),
                        "ToolCall must be refused before adapter entry"
                    );
                    continue;
                }
                let text = input.unwrap();
                let mut request =
                    crate::scripted_suites::common::simple_request(protocol, "test", "unused");
                request.messages = [crate::model::ModelInputMessage::RequestOnly(
                    crate::model::RequestOnlyModelContext::AuxiliaryQuestion { text: text.clone() },
                )]
                .into_iter()
                .collect();
                let mut wire =
                    serde_json::to_value(super::prepared_wire_request(&request).unwrap()).unwrap();
                assert!(
                    crate::model::adapter::correlation::observed_correlations(&mut wire, protocol)
                        .is_empty()
                );
                let encoded = serde_json::to_string(&wire).unwrap();
                assert!(encoded.contains(source.id().as_str()));
                assert!(encoded.contains("data"));
                match protocol {
                    ModelProtocol::OpenAiChatCompletions | ModelProtocol::AnthropicMessages => {
                        assert!(
                            wire["messages"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .all(|m| m["role"] != "tool")
                        );
                    }
                    ModelProtocol::OpenAiResponses => assert!(
                        wire["input"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|m| m["type"] != "function_call_output")
                    ),
                }
            }
        }
    }

    #[test]
    fn prepared_retry_shares_input_and_one_encoded_body() {
        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
            ModelProtocol::AnthropicMessages,
        ] {
            super::take_preparation_work();
            let request = crate::scripted_suites::common::simple_request(protocol, "test", "input");
            let prepared = super::prepare_request(request, protocol).unwrap();
            let retry = prepared.clone();
            assert!(std::sync::Arc::ptr_eq(&prepared.input, &retry.input));
            assert_eq!(prepared.body.as_ptr(), retry.body.as_ptr());
            let input = std::sync::Arc::downgrade(&prepared.input);
            drop(prepared);
            assert!(input.upgrade().is_some());
            drop(retry);
            assert!(input.upgrade().is_none());
            assert_eq!(super::take_preparation_work(), (1, 1));
        }
    }

    #[test]
    fn openai_borrowed_wire_matches_legacy_request_shape_for_large_frozen_input() {
        use crate::model::catalog::{ChatMaxTokensField, ChatStreamUsage, ResponsesStorageMode};
        use serde_json::json;

        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
        ] {
            let text = "Unicode 🦀 text with JSON punctuation \n".repeat(256);
            let mut request =
                crate::scripted_suites::common::simple_request(protocol, "fixture", &text);
            let message = request.messages[0].clone();
            request.messages.extend(std::iter::repeat_n(message, 127));
            request.effective_system_prompt = "system".into();
            request.invocation.compat.chat_max_tokens_field = ChatMaxTokensField::MaxTokens;
            request.invocation.compat.chat_stream_usage = ChatStreamUsage::Supported;
            request.invocation.compat.responses_storage = ResponsesStorageMode::Stateless;
            request.invocation.request_params.insert(
                "metadata".into(),
                json!({"native": {"escaped": text, "nullable":null}}),
            );
            let mut definition = tool("lookup", "tool-lookup");
            definition.input_schema = json!({"type":"object", "$comment":text}).into();
            request.tools.push(definition);
            // The legacy Chat SDK message array and the legacy Responses
            // item tree used these different structural shapes.
            let legacy = if protocol == ModelProtocol::OpenAiChatCompletions {
                let mut messages = vec![json!({"role":"system", "content":"system"})];
                messages.extend(std::iter::repeat_n(
                    json!({"role":"user", "content":[{"type":"text","text":text}]}),
                    128,
                ));
                json!({"model":"fixture", "messages":messages, "stream":true,
                    "stream_options":{"include_usage":true}, "max_tokens":request.max_output_tokens(),
                    "tools":[{"type":"function", "function":{"name":"lookup",
                        "description":"d", "parameters":request.tools[0].input_schema}}]})
            } else {
                let item = json!({"type":"message","role":"user","content":[{"type":"input_text","text":text}]});
                json!({"model":"fixture", "input":vec![item;128], "stream":true,
                    "instructions":"system", "store":false, "include":["reasoning.encrypted_content"],
                    "max_output_tokens":request.max_output_tokens(), "tools":[{"type":"function",
                        "name":"lookup","description":"d","parameters":request.tools[0].input_schema}]})
            };
            let expected = crate::model::invocation::finalize_provider_request(
                legacy,
                request.request_params(),
                protocol,
            )
            .unwrap();
            let prepared = super::prepare_request(request, protocol).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&prepared.body).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn bounded_preflight_counts_nested_tool_json_and_escaping_on_every_protocol() {
        use crate::message::types::{
            AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
            ToolCallOccurrenceRef, ToolMessageBlock,
        };
        use crate::runtime::identity::{MessageId, ToolCallId};
        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
            ModelProtocol::AnthropicMessages,
        ] {
            let hostile = "\0\n\"\\🙂".repeat(16_384);
            let mut input =
                crate::scripted_suites::common::simple_request(protocol, "test", &hostile);
            input.messages.push(
                MessageBlock::Assistant(AssistantMessageBlock {
                    id: MessageId::new("assistant"),
                    content: vec![AssistantContentBlock::ToolCall(
                        crate::tools::types::ToolCall {
                            id: ToolCallId::new("call"),
                            tool_id: ToolId::new("tool"),
                            name: "tool".into(),
                            arguments: serde_json::json!({"nested": {"text": hostile}}),
                        },
                    )],
                })
                .into(),
            );
            input.messages.push(
                MessageBlock::Tool(ToolMessageBlock {
                    id: MessageId::new("result"),
                    tool_call_id: ToolCallId::new("call"),
                    tool_id: ToolId::new("tool"),
                    occurrence: ToolCallOccurrenceRef::new(
                        MessageId::new("assistant"),
                        ContentBlockIndex::new(0),
                    ),
                    result: crate::scripted_suites::support::fake::success_result(&hostile),
                })
                .into(),
            );
            let wire = super::prepared_wire_request(&input).unwrap();
            let bytes = serde_json::to_vec(&wire).unwrap().len();
            assert_eq!(
                super::validate_bounded_prepared_request(&input, bytes).unwrap(),
                bytes
            );
            assert_eq!(
                super::validate_bounded_prepared_request(&input, bytes - 1)
                    .unwrap_err()
                    .kind,
                ModelErrorKind::ContextWindowExceeded
            );
            println!(
                "{protocol:?}: frozen_bytes={} translated_bytes={bytes}",
                serde_json::to_vec(&input).unwrap().len()
            );
        }
    }

    #[test]
    fn ambiguous_unresolved_wire_ids_fail_complete_adapter_preflight() {
        use crate::message::types::{
            AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
            ToolCallOccurrenceRef, ToolMessageBlock,
        };
        use crate::runtime::identity::{MessageId, ToolCallId};
        use crate::tools::types::ToolCall;
        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
            ModelProtocol::AnthropicMessages,
        ] {
            let mut input =
                crate::scripted_suites::common::simple_request(protocol, "test", "input");
            let call = ToolCall {
                id: ToolCallId::new("repeat"),
                tool_id: ToolId::new("tool"),
                name: "tool".into(),
                arguments: serde_json::json!({}),
            };
            input.messages.push(
                MessageBlock::Assistant(AssistantMessageBlock {
                    id: MessageId::new("assistant"),
                    content: vec![
                        AssistantContentBlock::ToolCall(call.clone()),
                        AssistantContentBlock::ToolCall(call),
                    ],
                })
                .into(),
            );
            for block in [1, 0] {
                input.messages.push(
                    MessageBlock::Tool(ToolMessageBlock {
                        id: MessageId::new(format!("result-{block}")),
                        tool_call_id: ToolCallId::new("repeat"),
                        tool_id: ToolId::new("tool"),
                        occurrence: ToolCallOccurrenceRef::new(
                            MessageId::new("assistant"),
                            ContentBlockIndex::new(block),
                        ),
                        result: crate::scripted_suites::support::fake::success_result("result"),
                    })
                    .into(),
                );
            }
            let error = super::validate_prepared_request(&input)
                .expect_err("raw wire IDs cannot disambiguate two pending physical occurrences");
            assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
            assert!(error.message.contains("correlation"));
        }
    }

    fn request() -> ModelRequest {
        ModelRequest {
            response_schema: None,
            response_byte_limit: None,
            continuation_source: None,
            images: std::collections::BTreeMap::new(),
            invocation: crate::model::invocation::ModelInvocationConfig {
                model: "m".to_owned(),
                protocol: ModelProtocol::OpenAiResponses,
                max_output_tokens: 512,
                request_params: crate::model::invocation::RequestParams::new(),
                capabilities: crate::model::catalog::ModelCapabilities::text_only(),
                compat: crate::model::catalog::ModelCompat::default(),
            },
            messages: crate::model::SelectedModelInput::default(),
            tools: Vec::new(),
            effective_system_prompt: String::new(),
            continuation: None,
        }
    }

    #[test]
    fn reconciled_coverage_replays_only_complete_native_units() {
        use crate::message::types::{
            AssistantContentBlock, AssistantMessageBlock, MessageBlock, ReasoningBlock,
            UserContentBlock,
        };
        use crate::runtime::continuation::AcceptedOutputContinuationSource;
        use crate::runtime::identity::{ConversationId, MessageId, RequestId};
        let states = [
            (
                ModelProtocol::OpenAiResponses,
                ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stored {
                    previous_response_id: "accepted-response".into(),
                }),
                None,
                true,
            ),
            (
                ModelProtocol::OpenAiResponses,
                ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stored {
                    previous_response_id: "accepted-response".into(),
                }),
                Some("opaque reasoning unavailable"),
                false,
            ),
            (
                ModelProtocol::OpenAiResponses,
                ProviderContinuationState::OpenAiResponses(
                    OpenAiResponsesContinuation::Stateless {
                        items: vec![
                            serde_json::json!({"type":"reasoning", "id":"rs-real", "encrypted_content":"exact-encrypted-state"}),
                        ].into(),
                    },
                ),
                None,
                true,
            ),
            (
                ModelProtocol::AnthropicMessages,
                ProviderContinuationState::Anthropic(AnthropicContinuation {
                    opaque: serde_json::json!({"type":"thinking", "thinking":"original thinking", "signature":"real-native-signature"}).into(),
                }),
                Some("original thinking"),
                true,
            ),
        ];
        for (protocol, state, text, replayable) in states {
            let mut input =
                crate::scripted_suites::common::simple_request(protocol, "test", "covered input");
            let output = MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("actual-output"),
                content: vec![AssistantContentBlock::Reasoning(ReasoningBlock {
                    text: text.map(str::to_owned),
                    provider_state: Some(state.clone()),
                })],
            });
            let source = AcceptedOutputContinuationSource::from_accepted_output(
                RequestId::new("actual-request"),
                &ConversationId::generate(),
                &input.messages,
                &output,
            )
            .unwrap();
            input.messages.push(output.clone().into());
            input.continuation = Some(state);
            input.continuation_source = Some(source);
            assert!(
                !input.reconcile_continuation().unwrap(),
                "unchanged coverage stays native"
            );
            let crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) =
                &mut input.messages[0]
            else {
                panic!("user")
            };
            user.content = vec![UserContentBlock::Text(crate::message::TextBlock {
                text: "request-local transformed input".into(),
            })];
            assert!(input.reconcile_continuation().unwrap());
            assert!(input.continuation.is_none());
            assert_eq!(
                input.messages[1].as_canonical(),
                Some(&output),
                "native history remains immutable"
            );
            if replayable {
                super::validate_prepared_request(&input).unwrap();
            } else {
                let error = super::validate_prepared_request(&input).unwrap_err();
                assert!(
                    error.message.contains("preserved provider-native"),
                    "{error:?}"
                );
            }
            assert!(
                !input.reconcile_continuation().unwrap(),
                "retry uses the reconciled version"
            );
        }
    }

    fn tool(name: &str, id: &str) -> ModelToolDefinition {
        ModelToolDefinition {
            id: ToolId::new(id),
            name: name.to_owned(),
            description: "d".to_owned(),
            input_schema: serde_json::json!({"type": "object"}).into(),
        }
    }

    #[test]
    fn protocol_mismatch_is_invalid_request() {
        let mut r = request();
        r.invocation.protocol = ModelProtocol::AnthropicMessages;
        let error = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
    }

    #[test]
    fn chat_completions_rejects_any_continuation() {
        for state in [
            ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stored {
                previous_response_id: "resp_1".to_owned(),
            }),
            ProviderContinuationState::Anthropic(AnthropicContinuation {
                opaque: serde_json::json!({}).into(),
            }),
        ] {
            let mut r = request();
            r.invocation.protocol = ModelProtocol::OpenAiChatCompletions;
            r.continuation = Some(state);
            let error =
                validate_request(&r, ModelProtocol::OpenAiChatCompletions).expect_err("must fail");
            assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
        }
    }

    #[test]
    fn responses_rejects_non_responses_continuation() {
        let mut r = request();
        r.continuation = Some(ProviderContinuationState::Anthropic(
            AnthropicContinuation {
                opaque: serde_json::json!({"signature": "s"}).into(),
            },
        ));
        let error = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
    }

    #[test]
    fn anthropic_rejects_non_anthropic_continuation() {
        let mut r = request();
        r.invocation.protocol = ModelProtocol::AnthropicMessages;
        r.continuation = Some(ProviderContinuationState::OpenAiResponses(
            OpenAiResponsesContinuation::Stateless {
                items: Vec::new().into(),
            },
        ));
        let error = validate_request(&r, ModelProtocol::AnthropicMessages).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
    }

    #[test]
    fn duplicate_tool_names_are_rejected() {
        let mut r = request();
        r.tools = vec![tool("dup", "tool-a"), tool("dup", "tool-b")];
        let error = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
        assert!(error.message.contains("dup"));
    }

    #[test]
    fn empty_tool_name_is_rejected() {
        let mut r = request();
        r.tools = vec![tool("", "tool-a")];
        let error = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
    }

    #[test]
    fn empty_tool_id_is_rejected() {
        let mut r = request();
        r.tools = vec![tool("name", "")];
        let error = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
    }

    #[test]
    fn valid_tools_resolve_by_name() {
        let mut r = request();
        r.tools = vec![tool("list", "tool-list"), tool("read", "tool-read")];
        let tools = validate_request(&r, ModelProtocol::OpenAiResponses).expect("must pass");
        assert_eq!(tools.resolve("list").map(ToolId::as_str), Some("tool-list"));
        assert_eq!(tools.resolve("read").map(ToolId::as_str), Some("tool-read"));
        assert_eq!(tools.resolve("missing"), None);
        assert_eq!(
            tools.names().collect::<Vec<_>>(),
            vec!["list", "read"],
            "names resolve in deterministic sorted order"
        );
    }
}
