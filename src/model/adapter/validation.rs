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
use crate::model::invocation::validate_required_modalities;
use crate::model::types::{ModelProtocol, ModelRequest};
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::ToolId;

/// Tools validated for one request, resolved by model-facing name.
#[derive(Debug, Clone, Default)]
pub struct ValidatedTools {
    table: std::sync::Arc<ToolCatalog>,
    pub(super) correlation: super::correlation::Plan,
    pub(super) continuation_boundary: Option<usize>,
}

#[derive(Default)]
pub(super) struct ToolCatalog {
    definitions: std::sync::OnceLock<Vec<crate::tools::types::ModelToolDefinition>>,
    by_name: BTreeMap<String, ToolId>,
    wire: [std::sync::OnceLock<std::sync::Arc<serde_json::value::RawValue>>; 3],
}
impl std::fmt::Debug for ToolCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolCatalog")
            .field("by_name", &self.by_name)
            .finish_non_exhaustive()
    }
}
impl ToolCatalog {
    fn matches(&self, definitions: &[crate::tools::types::ModelToolDefinition]) -> bool {
        let Some(cached) = self.definitions.get() else {
            return self.by_name.is_empty() && definitions.is_empty();
        };
        cached.len() == definitions.len()
            && cached.iter().zip(definitions).all(|(a, b)| {
                a.id == b.id
                    && a.name == b.name
                    && a.description == b.description
                    && std::sync::Arc::ptr_eq(&a.input_schema, &b.input_schema)
            })
    }
}

impl ValidatedTools {
    pub(super) fn encoded_tools<T: serde::Serialize>(
        &self,
        protocol: ModelProtocol,
        limit: Option<usize>,
        definitions: &[crate::tools::types::ModelToolDefinition],
        encode: impl FnOnce() -> T,
    ) -> Result<Option<std::sync::Arc<serde_json::value::RawValue>>, ModelError> {
        if self.is_empty() {
            return Ok(None);
        }
        let index = match protocol {
            ModelProtocol::OpenAiChatCompletions => 0,
            ModelProtocol::OpenAiResponses => 1,
            ModelProtocol::AnthropicMessages => 2,
        };
        let slot = &self.table.wire[index];
        if let Some(raw) = slot.get() {
            if limit.is_some_and(|limit| raw.get().len() > limit) {
                return Err(wire_budget_error());
            }
            return Ok(Some(raw.clone()));
        }
        let raw = super::cache::encode_raw(&encode(), limit)?;
        #[cfg(test)]
        super::cache::work(|work| work.catalog_bytes += raw.get().len());
        // Keep the exact owner certificate only after bounded encoding succeeds.
        self.table.definitions.get_or_init(|| definitions.to_vec());
        let _ = slot.set(raw);
        Ok(slot.get().cloned())
    }

    /// The tool names in deterministic (sorted) order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.table.by_name.keys().map(String::as_str)
    }

    /// Whether the request exposed no tool to the model at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.by_name.is_empty()
    }

    /// Resolves a model-facing tool name to its canonical [`ToolId`].
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<&ToolId> {
        self.table.by_name.get(name)
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

    // Capability-independent certificates survive changes of model capabilities.
    let (required, images) = super::cache::admission(&request.messages);
    validate_required_modalities(&required, &request.invocation.capabilities)?;
    if request.images.len() > 16 || images > 16 {
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
    let correlation =
        super::correlation::request_plan(request, boundary).map_err(invalid_request)?;
    let mut tools = validate_tools(request)?;
    tools.continuation_boundary = boundary;
    tools.correlation = correlation;
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
    #[cfg(test)]
    body_lifetime: std::sync::Weak<()>,
}

impl PreparedModelRequest {
    pub(crate) fn input(&self) -> &ModelRequest {
        &self.input
    }

    #[cfg(test)]
    pub(crate) fn ownership_probe(&self) -> PreparedOwnershipProbe {
        PreparedOwnershipProbe {
            body: (self.body.as_ptr() as usize, self.body.len()),
            body_lifetime: self.body_lifetime.clone(),
            input: std::sync::Arc::downgrade(&self.input),
        }
    }
}

#[cfg(test)]
pub(crate) struct PreparedOwnershipProbe {
    pub body: (usize, usize),
    pub body_lifetime: std::sync::Weak<()>,
    pub input: std::sync::Weak<ModelRequest>,
}

#[cfg(test)]
fn track_body_lifetime(body: Vec<u8>) -> (bytes::Bytes, std::sync::Weak<()>) {
    struct TrackedBody {
        bytes: Vec<u8>,
        _lifetime: std::sync::Arc<()>,
    }
    impl AsRef<[u8]> for TrackedBody {
        fn as_ref(&self) -> &[u8] {
            &self.bytes
        }
    }
    let lifetime = std::sync::Arc::new(());
    let probe = std::sync::Arc::downgrade(&lifetime);
    let bytes = bytes::Bytes::from_owner(TrackedBody {
        bytes: body,
        _lifetime: lifetime,
    });
    (bytes, probe)
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
    prepare_request_with_limit(request, protocol, None)
}

pub(crate) fn prepare_request_with_limit(
    request: ModelRequest,
    protocol: ModelProtocol,
    body_limit: Option<usize>,
) -> Result<PreparedModelRequest, ModelError> {
    #[cfg(feature = "issue-387-profile")]
    let _request = super::profile::RequestScope::new(protocol, "prepare");
    #[cfg(test)]
    PREFLIGHTS.with(|v| v.set(v.get() + 1));
    let tools = validate_request(&request, protocol)?;
    let wire = translate_prepared(&request, &tools, body_limit)?;
    #[cfg(feature = "issue-387-profile")]
    let _encoding = super::profile::phase(super::profile::Phase::HttpEncode);
    #[cfg(test)]
    PREPARATION_WORK.with(|work| work.update(|(translated, encoded)| (translated, encoded + 1)));
    let body = if let Some(limit) = body_limit {
        crate::bounded_json::encode_capped_bytes(&wire, limit)
    } else {
        let mut body = Vec::with_capacity(wire.capacity_hint());
        serde_json::to_writer(&mut body, &wire).map(|()| body)
    }
    .map_err(|error| wire_encoding_error(&error))?;
    #[cfg(test)]
    let (body, body_lifetime) = track_body_lifetime(body);
    #[cfg(not(test))]
    let body = body.into();
    Ok(PreparedModelRequest {
        input: std::sync::Arc::new(request),
        body,
        tools: std::sync::Arc::new(tools),
        #[cfg(test)]
        body_lifetime,
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
    let translated = prepared_wire_request_with_limit(request, Some(limit))?;
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

impl PreparedWireRequest<'_> {
    fn capacity_hint(&self) -> usize {
        match self {
            Self::Chat(request) => request.capacity_hint(),
            Self::Responses(request) => request.capacity_hint(),
            Self::Anthropic(request) => request.capacity_hint(),
        }
    }
}

#[cfg(test)]
fn prepared_wire_request(request: &ModelRequest) -> Result<PreparedWireRequest<'_>, ModelError> {
    prepared_wire_request_with_limit(request, None)
}

fn prepared_wire_request_with_limit(
    request: &ModelRequest,
    limit: Option<usize>,
) -> Result<PreparedWireRequest<'_>, ModelError> {
    #[cfg(test)]
    PREFLIGHTS.with(|v| v.set(v.get() + 1));
    #[cfg(feature = "issue-387-profile")]
    let _request = super::profile::RequestScope::new(request.protocol(), "preflight");
    let tools = validate_request(request, request.protocol())?;
    translate_prepared(request, &tools, limit)
}

fn translate_prepared<'a>(
    request: &'a ModelRequest,
    tools: &ValidatedTools,
    limit: Option<usize>,
) -> Result<PreparedWireRequest<'a>, ModelError> {
    #[cfg(test)]
    PREPARATION_WORK.with(|work| work.update(|(translated, encoded)| (translated + 1, encoded)));
    match request.protocol() {
        ModelProtocol::OpenAiChatCompletions => {
            super::openai::chat_completions::translate_request_with_limit(request, tools, limit)
                .map(PreparedWireRequest::Chat)
        }
        ModelProtocol::OpenAiResponses => super::openai::responses::translate_request_with_limit(
            request,
            tools,
            request.invocation.compat.responses_storage,
            limit,
        )
        .map(PreparedWireRequest::Responses),
        ModelProtocol::AnthropicMessages => {
            super::anthropic::mapping::translate_request_with_limit(request, tools, limit)
                .map(PreparedWireRequest::Anthropic)
        }
    }
}

fn validate_continuation(
    request: &ModelRequest,
    protocol: ModelProtocol,
) -> Result<(), ModelError> {
    if request.continuation.is_some()
        && let Some(boundary) = request.continuation_boundary().map_err(invalid_request)?
        && let crate::model::projected::ModelMessageView::Assistant(content) =
            request.messages[boundary].view()
        && !content.iter().any(|block| {
            matches!(block,
            crate::message::types::AssistantContentBlock::Reasoning(reasoning)
            if reasoning.belongs_to(request.invocation.replay_origin().as_ref())
                && reasoning.provider_state.as_ref() == request.continuation.as_ref())
        })
    {
        return Err(invalid_request(
            "continuation belongs to a different model or provider binding".into(),
        ));
    }
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
    let mut cache = request
        .messages
        .preparation()
        .lock()
        .expect("input preparation cache");
    if let Some(table) = cache
        .tools
        .as_ref()
        .filter(|table| table.matches(&request.tools))
    {
        return Ok(ValidatedTools {
            table: table.clone(),
            ..ValidatedTools::default()
        });
    }
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
    let table = std::sync::Arc::new(ToolCatalog {
        definitions: std::sync::OnceLock::new(),
        by_name,
        wire: std::array::from_fn(|_| std::sync::OnceLock::new()),
    });
    cache.tools = Some(table.clone());
    Ok(ValidatedTools {
        table,
        ..ValidatedTools::default()
    })
}

pub(super) fn wire_budget_error() -> ModelError {
    let mut error = invalid_request("translated provider request exceeds byte budget".into());
    error.kind = ModelErrorKind::ContextWindowExceeded;
    error
}
pub(super) fn wire_encoding_error(error: &serde_json::Error) -> ModelError {
    if error.is_io() {
        wire_budget_error()
    } else {
        invalid_request(format!("failed to encode provider request: {error}"))
    }
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
    fn provider_translation_reuses_one_image_proof_for_repeated_occurrences() {
        use crate::message::content::ImageReference;
        use crate::message::types::{MessageBlock, UserContentBlock};
        use crate::model::images::{FrozenImage, take_decodes};
        use crate::model::{ModelInputMessage, catalog::Modality};
        use crate::runtime::identity::ArtifactId;

        for protocol in [
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
            ModelProtocol::AnthropicMessages,
        ] {
            let mut request =
                crate::scripted_suites::common::simple_request(protocol, "test", "input");
            request
                .invocation
                .capabilities
                .input_modalities
                .insert(Modality::Image);
            let id = ArtifactId::new("repeat");
            let block = UserContentBlock::Image(ImageReference {
                artifact_id: id.clone(),
                alt: None,
            });
            let ModelInputMessage::Canonical(MessageBlock::User(user)) = &mut request.messages[0]
            else {
                panic!("fixture starts with User input");
            };
            user.content.extend([block.clone(), block]);
            let mut bytes = Vec::new();
            png::Encoder::new(&mut bytes, 1, 1)
                .write_header()
                .unwrap()
                .write_image_data(&[0])
                .unwrap();
            take_decodes();
            let image = FrozenImage::new(bytes).unwrap();
            request.images.insert(id.clone(), image.clone());
            let prepared = super::prepare_request(request, protocol).unwrap();
            assert_eq!(prepared.input().images[&id].as_ptr(), image.as_ptr());
            let body = std::str::from_utf8(&prepared.body).unwrap();
            assert_eq!(
                body.matches("iVBORw0KGgo").count(),
                2,
                "both occurrences are transported"
            );
            assert_eq!(take_decodes(), 1);
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
                    result: crate::scripted_suites::support::fake::success_result(&hostile).into(),
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
                        result: crate::scripted_suites::support::fake::success_result("result")
                            .into(),
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
                replay_binding: Some("test-binding".into()),
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
    #[allow(clippy::too_many_lines)] // One native/readable replay matrix shares coverage assertions.
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
                Some("readable stored reasoning"),
                true,
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
                ModelProtocol::OpenAiResponses,
                ProviderContinuationState::OpenAiResponses(
                    OpenAiResponsesContinuation::Stateless {
                        items: vec![
                            serde_json::json!({"type":"reasoning", "id":"rs-incomplete", "summary":[]}),
                        ].into(),
                    },
                ),
                None,
                false,
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
                    origin: input.invocation.replay_origin(),
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
                assert_eq!(error.kind, crate::model::ModelErrorKind::Unsupported);
                assert!(
                    error
                        .message
                        .contains("complete supported native replay unit"),
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
