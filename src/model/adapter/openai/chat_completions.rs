//! `OpenAI` Chat Completions adapter.
//!
//! Implements the canonical [`ModelAdapter`] for `ModelProtocol::OpenAiChatCompletions`.
//! Borrowed request views are serialized directly, with a checked shallow
//! overlay for opaque request parameters. Response events remain raw JSON so
//! unknown finish reasons and additional chunk fields are tolerated.
//! Chat rejects native continuation, which belongs to Responses.
//! No request-level reasoning control is ever synthesized: whatever the
//! selected Model Profile configured is exactly what appears on the
//! wire. Previous assistant reasoning uses `reasoning_content` by default;
//! an optional override selects `reasoning` or omission. Reasoning stays
//! separate from visible assistant text.

use std::borrow::Cow;
use std::collections::{BTreeMap, VecDeque};

use async_openai::error::OpenAIError;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::message::types::AssistantContentBlock;
use crate::message::types::ContentBlockIndex;
use crate::model::adapter::block_index::BlockAllocator;
use crate::model::adapter::openai::client::OpenAiTransport;
use crate::model::adapter::openai::config::OpenAiAdapterConfig;
use crate::model::adapter::openai::mapping::{
    map_chat_finish_reason, normalize_chat_usage, normalize_error, stream_retry_disposition,
};
use crate::model::adapter::openai::qwen_xml::{self, QwenReservedEnvelope};
use crate::model::adapter::proposal::{accept_tool_call_arguments, resolve_tool_identity};
use crate::model::adapter::traits::{
    ModelAdapter, ModelStream, ModelStreamItem, ModelStreamProgress, model_stream_of_failure,
};
use crate::model::adapter::validation::{PreparedModelRequest, ValidatedTools, prepare_request};
use crate::model::catalog::{
    ChatMaxTokensField, ChatReasoningReplay, ChatStreamUsage, ChatToolProtocol,
};
use crate::model::error::{
    MalformedToolProposalSource, ModelError, ModelErrorKind, is_context_window_error,
};
use crate::model::event::ModelEvent;
use crate::model::projected::ModelMessageView;
use crate::model::types::{ModelProtocol, ModelRequest, ModelUsage};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::ToolCallId;

/// Adapter for the `OpenAI` Chat Completions protocol.
pub struct OpenAiChatCompletionsAdapter {
    client: OpenAiTransport,
}

impl std::fmt::Debug for OpenAiChatCompletionsAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiChatCompletionsAdapter")
            .finish_non_exhaustive()
    }
}

impl OpenAiChatCompletionsAdapter {
    /// Creates the adapter from rustX-owned configuration.
    #[must_use]
    pub fn new(config: OpenAiAdapterConfig) -> Self {
        let (api_key, api_base, http_client) = config.into_parts();
        Self {
            client: OpenAiTransport::new(&api_key, &api_base, http_client),
        }
    }
}

impl ModelAdapter for OpenAiChatCompletionsAdapter {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }

    fn stream(&self, request: ModelRequest, cancellation: CancellationSignal) -> ModelStream {
        match prepare_request(request, self.protocol()) {
            Ok(prepared) => self.stream_prepared(prepared, cancellation),
            Err(error) => model_stream_of_failure(error),
        }
    }

    fn stream_prepared(
        &self,
        request: PreparedModelRequest,
        cancellation: CancellationSignal,
    ) -> ModelStream {
        let PreparedModelRequest {
            input, body, tools, ..
        } = request;
        if input.protocol() != self.protocol() {
            return model_stream_of_failure(invalid_request("prepared request protocol mismatch"));
        }
        Box::pin(futures_util::stream::unfold(
            ChatPhase::Preparing {
                client: self.client.clone(),
                request: body,
                response_byte_limit: input.response_byte_limit,
                normalizer: ChatStreamNormalizer::new(
                    tools,
                    input.invocation.compat.chat_tool_protocol,
                ),
                cancellation,
            },
            chat_phase_next,
        ))
    }
}

async fn chat_phase_next(phase: ChatPhase) -> Option<(ModelStreamItem, ChatPhase)> {
    match phase {
        ChatPhase::Preparing {
            client,
            request,
            response_byte_limit,
            normalizer,
            cancellation,
        } => {
            if cancellation.is_cancelled() {
                return Some((failed(cancelled_error()), ChatPhase::Finished));
            }
            // The provider request attempt begins: Started is emitted before
            // the network-opening await so the lifecycle stays consistent
            // when cancellation interrupts that await.
            Some((
                ModelStreamItem::Event(ModelEvent::Started),
                ChatPhase::Opening {
                    client,
                    request,
                    response_byte_limit,
                    normalizer,
                    cancellation,
                },
            ))
        }
        ChatPhase::Opening {
            client,
            request,
            response_byte_limit,
            mut normalizer,
            cancellation,
        } => {
            let outcome = tokio::select! {
                outcome = client.stream("/chat/completions", request, response_byte_limit) => outcome,
                () = cancellation.cancelled() => {
                    return Some((failed(cancelled_error()), ChatPhase::Finished));
                }
            };
            match outcome {
                Ok(mut stream) => {
                    let mut pending = VecDeque::new();
                    chat_pull(&mut stream, &mut normalizer, &cancellation, &mut pending).await;
                    let event = pending.pop_front().expect("pending is non-empty here");
                    let next_phase = if is_terminal(&event) {
                        ChatPhase::Finished
                    } else {
                        ChatPhase::Streaming {
                            stream,
                            normalizer,
                            cancellation,
                            pending,
                        }
                    };
                    Some((event, next_phase))
                }
                Err(error) => Some((failed(normalize_error(error)), ChatPhase::Finished)),
            }
        }
        ChatPhase::Streaming {
            mut stream,
            mut normalizer,
            cancellation,
            mut pending,
        } => {
            chat_pull(&mut stream, &mut normalizer, &cancellation, &mut pending).await;
            let event = pending.pop_front().expect("pending is non-empty here");
            let next_phase = if is_terminal(&event) {
                ChatPhase::Finished
            } else {
                ChatPhase::Streaming {
                    stream,
                    normalizer,
                    cancellation,
                    pending,
                }
            };
            Some((event, next_phase))
        }
        ChatPhase::Finished => None,
    }
}

/// Pulls provider wire events into `pending` until at least one stream item is
/// ready or the invocation is over.
async fn chat_pull(
    stream: &mut async_openai::types::stream::StreamResponse<serde_json::Value>,
    normalizer: &mut ChatStreamNormalizer,
    cancellation: &CancellationSignal,
    pending: &mut VecDeque<ModelStreamItem>,
) {
    while pending.is_empty() {
        let item = tokio::select! {
            item = stream.next() => item,
            () = cancellation.cancelled() => {
                pending.push_back(failed(cancelled_error()));
                break;
            }
        };
        match item {
            Some(Ok(chunk)) => match normalizer.push(chunk) {
                Ok(events) => pending.extend(events),
                Err(error) => pending.push_back(failed(error)),
            },
            Some(Err(error)) if is_done_marker(&error) => {
                finish(normalizer, pending);
                break;
            }
            Some(Err(error)) => {
                pending.push_back(failed(normalize_error(error)));
                break;
            }
            None => {
                finish(normalizer, pending);
                break;
            }
        }
    }
}

/// Emits the terminal event the stream was waiting for (completion or
/// unexpected-end failure) into `pending`.
fn finish(normalizer: &mut ChatStreamNormalizer, pending: &mut VecDeque<ModelStreamItem>) {
    match normalizer.finish() {
        Ok(events) => extend_events(pending, events),
        Err(error) => pending.push_back(failed(error)),
    }
}

fn extend_events(pending: &mut VecDeque<ModelStreamItem>, events: Vec<ModelEvent>) {
    pending.extend(events.into_iter().map(ModelStreamItem::Event));
}

fn is_done_marker(error: &OpenAIError) -> bool {
    matches!(error, OpenAIError::JSONDeserialize(_, content) if content == "[DONE]")
}

fn is_terminal(event: &ModelStreamItem) -> bool {
    matches!(
        event,
        ModelStreamItem::Event(ModelEvent::Completed { .. } | ModelEvent::Failed { .. })
    )
}

fn failed(error: ModelError) -> ModelStreamItem {
    ModelStreamItem::Event(ModelEvent::Failed { error })
}

fn cancelled_error() -> ModelError {
    ModelError {
        kind: ModelErrorKind::Cancelled,
        message: "model invocation cancelled".to_owned(),
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
        malformed_tool_proposal: None,
        timeout_phase: None,
        generation: None,
    }
}

/// Phase machine driving one adapter invocation.
///
/// The `Preparing` variant is deliberately larger than the others: it owns
/// the final request JSON and the normalizer until the provider stream opens.
#[allow(clippy::large_enum_variant)]
enum ChatPhase {
    Preparing {
        client: OpenAiTransport,
        request: bytes::Bytes,
        response_byte_limit: Option<usize>,
        normalizer: ChatStreamNormalizer,
        cancellation: CancellationSignal,
    },
    Opening {
        client: OpenAiTransport,
        request: bytes::Bytes,
        response_byte_limit: Option<usize>,
        normalizer: ChatStreamNormalizer,
        cancellation: CancellationSignal,
    },
    Streaming {
        stream: async_openai::types::stream::StreamResponse<serde_json::Value>,
        normalizer: ChatStreamNormalizer,
        cancellation: CancellationSignal,
        pending: VecDeque<ModelStreamItem>,
    },
    Finished,
}

/// Adapter-local canonical block keys for Chat Completions chunks, which
/// carry no provider content-block indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ChatBlockKey {
    /// The streamed text block of the single choice.
    Text,
    /// The streamed reasoning block of the single choice.
    Reasoning,
    /// The streamed refusal block of the single choice.
    Refusal,
    /// A tool call assembled by its provider tool index.
    Tool(u32),
}

/// Wire shape of one streamed chunk; unknown future fields are tolerated.
#[derive(Debug, Deserialize)]
struct ChatChunkWire {
    #[serde(default)]
    choices: Vec<ChatChoiceWire>,
    #[serde(default)]
    usage: Option<serde_json::Value>,
    /// Moderation carries output semantics that the canonical event model
    /// cannot represent losslessly.
    #[serde(default)]
    moderation: Option<serde_json::Value>,
    #[serde(default)]
    video_result: Option<serde_json::Value>,
    #[serde(default)]
    web_search: Option<serde_json::Value>,
    #[serde(default)]
    content_filter: Option<serde_json::Value>,
    #[serde(default)]
    input_sensitive: Option<bool>,
    #[serde(default)]
    output_sensitive: Option<bool>,
    #[serde(default)]
    input_sensitive_type: Option<i64>,
    #[serde(default)]
    output_sensitive_type: Option<i64>,
    #[serde(default)]
    base_resp: Option<ChatBaseResponseWire>,
}

#[derive(Debug, Deserialize)]
struct ChatBaseResponseWire {
    #[serde(default)]
    status_code: Option<i64>,
    #[serde(default)]
    status_msg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceWire {
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    delta: Option<ChatDeltaWire>,
    #[serde(default)]
    message: Option<ChatMessageSnapshotWire>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ChatDeltaWire {
    #[serde(default)]
    content: Option<String>,
    /// Qwen/vLLM exposes enabled thinking as `delta.reasoning`.
    #[serde(default)]
    reasoning: Option<String>,
    /// `DeepSeek` exposes enabled thinking as `delta.reasoning_content`.
    #[serde(default)]
    reasoning_content: Option<String>,
    /// `OpenRouter`'s structured reasoning blocks carry signatures and opaque
    /// data that cannot be flattened into canonical visible reasoning.
    #[serde(default)]
    reasoning_details: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    refusal: Option<String>,
    /// Deprecated pre-tool-calls function shape. It has no provider call id,
    /// so it cannot become a canonical `ToolCallId` without fabrication.
    #[serde(default)]
    function_call: Option<serde_json::Value>,
    #[serde(default)]
    audio: Option<serde_json::Value>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCallChunkWire>>,
}

#[derive(Debug, Deserialize, Default)]
struct ChatMessageSnapshotWire {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning_details: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default)]
    audio: Option<serde_json::Value>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCallSnapshotWire>>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCallSnapshotWire {
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "type", default)]
    tool_type: Option<String>,
    #[serde(default)]
    function: Option<ChatFunctionChunkWire>,
    #[serde(default)]
    custom: Option<serde_json::Value>,
    #[serde(default)]
    mcp: Option<serde_json::Value>,
}

impl ChatDeltaWire {
    /// Returns the provider's reasoning fragment across the two established
    /// OpenAI-compatible spellings. Conflicting simultaneous values are a
    /// protocol error rather than a reason to silently discard one.
    fn reasoning_delta(&self) -> Result<Option<&str>, ModelError> {
        let reasoning = self.reasoning.as_deref().filter(|text| !text.is_empty());
        let reasoning_content = self
            .reasoning_content
            .as_deref()
            .filter(|text| !text.is_empty());
        match (reasoning, reasoning_content) {
            (Some(reasoning), Some(reasoning_content)) if reasoning != reasoning_content => {
                Err(provider_error(
                    "chat chunk contains conflicting reasoning and reasoning_content deltas"
                        .to_owned(),
                ))
            }
            (Some(reasoning), _) => Ok(Some(reasoning)),
            (_, Some(reasoning_content)) => Ok(Some(reasoning_content)),
            (None, None) => Ok(None),
        }
    }
}

impl ChatMessageSnapshotWire {
    fn reasoning_value(&self) -> Result<Option<&str>, ModelError> {
        reasoning_value(self.reasoning.as_deref(), self.reasoning_content.as_deref())
    }
}

fn reasoning_value<'a>(
    reasoning: Option<&'a str>,
    reasoning_content: Option<&'a str>,
) -> Result<Option<&'a str>, ModelError> {
    match (reasoning, reasoning_content) {
        (Some(reasoning), Some(reasoning_content))
            if !reasoning.is_empty()
                && !reasoning_content.is_empty()
                && reasoning != reasoning_content =>
        {
            Err(provider_error(
                "chat message contains conflicting reasoning and reasoning_content values"
                    .to_owned(),
            ))
        }
        (Some(reasoning), Some(_reasoning_content)) if !reasoning.is_empty() => Ok(Some(reasoning)),
        (Some(_), Some(reasoning_content)) if !reasoning_content.is_empty() => {
            Ok(Some(reasoning_content))
        }
        (Some(reasoning), _) => Ok(Some(reasoning)),
        (_, Some(reasoning_content)) => Ok(Some(reasoning_content)),
        (None, None) => Ok(None),
    }
}

fn append_streamed(buffer: &mut Option<String>, text: &str) {
    buffer.get_or_insert_with(String::new).push_str(text);
}

fn validate_snapshot(
    buffer: Option<&str>,
    snapshot: Option<&str>,
    semantic: &str,
) -> Result<(), ModelError> {
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    if buffer.is_some_and(|streamed| streamed != snapshot) {
        return Err(provider_error(format!(
            "Chat cumulative {semantic} snapshot disagrees with streamed {semantic}"
        )));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ChatToolCallChunkWire {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "type", default)]
    tool_type: Option<String>,
    #[serde(default)]
    custom: Option<serde_json::Value>,
    #[serde(default)]
    mcp: Option<serde_json::Value>,
    #[serde(default)]
    function: Option<ChatFunctionChunkWire>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ChatFunctionChunkWire {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Recognizes an actual in-band tool-protocol emission in generated output,
/// for a model that declared the dialect.
///
/// Each dialect owns its own recognizer; the default `native` profile has
/// none, so generated text is never inspected at all. A recognizer proves
/// *emitted structure* rather than the co-occurrence of reserved tokens, so
/// an assistant answer that quotes or discusses the exact reserved syntax
/// stays an ordinary completion. It runs on the fully assembled output, so
/// provider chunk boundaries carry no meaning.
fn reserved_protocol_emission(protocol: ChatToolProtocol, output: &str) -> Option<&'static str> {
    match protocol {
        ChatToolProtocol::Native => None,
        ChatToolProtocol::QwenXml => {
            qwen_xml::tool_protocol_emission(output).map(QwenReservedEnvelope::shape)
        }
    }
}

/// Per-tool-call assembly state, keyed by the provider tool index.
#[derive(Debug)]
struct ToolAssembly {
    block_index: ContentBlockIndex,
    call_id: Option<ToolCallId>,
    name: Option<String>,
    arguments: String,
    started: bool,
}

/// Normalizes Chat Completions chunks into canonical events.
#[derive(Debug)]
struct ChatStreamNormalizer {
    tools: std::sync::Arc<ValidatedTools>,
    /// The model's declared in-band tool protocol. It is the only evidence
    /// this adapter uses to recognize reserved tool-protocol markup that
    /// leaked into ordinary output; nothing is inferred from a provider name
    /// or a base URL.
    tool_protocol: ChatToolProtocol,
    blocks: BlockAllocator<ChatBlockKey>,
    tool_calls: BTreeMap<u32, ToolAssembly>,
    usage: Option<ModelUsage>,
    finish_reason: Option<crate::model::finish::ModelFinishReason>,
    text_buffer: Option<String>,
    reasoning_buffer: Option<String>,
    refusal_buffer: Option<String>,
    buffered_generation: bool,
}

impl ChatStreamNormalizer {
    fn new(tools: std::sync::Arc<ValidatedTools>, tool_protocol: ChatToolProtocol) -> Self {
        Self {
            tools,
            tool_protocol,
            blocks: BlockAllocator::new(),
            tool_calls: BTreeMap::new(),
            usage: None,
            finish_reason: None,
            text_buffer: None,
            reasoning_buffer: None,
            refusal_buffer: None,
            buffered_generation: false,
        }
    }

    /// Processes one raw chunk, returning canonical events and ephemeral
    /// provider-derived progress in the same stream contract.
    fn push(&mut self, chunk: serde_json::Value) -> Result<Vec<ModelStreamItem>, ModelError> {
        self.buffered_generation = false;
        let events = self.push_events(chunk)?;
        let mut items = events
            .into_iter()
            .map(ModelStreamItem::Event)
            .collect::<Vec<_>>();
        if self.buffered_generation {
            items.push(ModelStreamItem::Progress(ModelStreamProgress::Generation));
        }
        Ok(items)
    }

    /// Processes one raw chunk into canonical events. Argument fragments that
    /// cannot yet be attributed to a canonical call are reported by `push` as
    /// ephemeral generation progress, while remaining buffered here.
    #[allow(clippy::too_many_lines)] // one provider stream chunk state machine
    fn push_events(&mut self, chunk: serde_json::Value) -> Result<Vec<ModelEvent>, ModelError> {
        if let Some(error) = chunk.get("error") {
            return Err(chat_stream_error(error));
        }
        let chunk: ChatChunkWire = serde_json::from_value(chunk)
            .map_err(|e| provider_error(format!("malformed chat chunk: {e}")))?;
        if value_has_output(chunk.moderation.as_ref())
            || value_has_output(chunk.video_result.as_ref())
            || value_has_output(chunk.web_search.as_ref())
            || value_has_output(chunk.content_filter.as_ref())
            || chunk.input_sensitive == Some(true)
            || chunk.output_sensitive == Some(true)
            || chunk.input_sensitive_type.is_some_and(|kind| kind != 0)
            || chunk.output_sensitive_type.is_some_and(|kind| kind != 0)
        {
            return Err(unsupported(
                "Chat Completions moderation, search, video, or sensitive-output data has no canonical representation",
            ));
        }
        if let Some(base) = &chunk.base_resp
            && base.status_code.is_some_and(|code| code != 0)
        {
            let message = base
                .status_msg
                .clone()
                .unwrap_or_else(|| "provider reported an unsuccessful base_resp".to_owned());
            let provider_code = base.status_code.map(|code| code.to_string());
            let numeric_code = base.status_code.and_then(|code| u64::try_from(code).ok());
            let kind = match base.status_code {
                Some(401 | 403) => ModelErrorKind::Authentication,
                Some(408) => ModelErrorKind::Timeout,
                Some(429) => ModelErrorKind::RateLimit,
                _ if is_context_window_error(&message, provider_code.as_deref()) => {
                    ModelErrorKind::ContextWindowExceeded
                }
                Some(400) => ModelErrorKind::InvalidRequest,
                _ => ModelErrorKind::ProviderError,
            };
            return Err(ModelError {
                kind: kind.clone(),
                message,
                retry_disposition: if kind == ModelErrorKind::ContextWindowExceeded {
                    crate::model::error::ModelRetryDisposition::Never
                } else {
                    stream_retry_disposition(
                        None,
                        provider_code.as_deref(),
                        numeric_code,
                        base.status_msg.as_deref().unwrap_or_default(),
                    )
                },
                retry_after_ms: None,
                provider_code,
                context_overflow: None,
                malformed_tool_proposal: None,
                timeout_phase: None,
                generation: None,
            }
            .normalized());
        }
        if chunk.choices.len() > 1 {
            return Err(unsupported(
                "multiple Chat Completions choices cannot be represented as one canonical Assistant turn",
            ));
        }
        let mut events = Vec::new();
        if let Some(usage) = chunk.usage.as_ref().and_then(normalize_chat_usage) {
            self.usage = Some(usage.clone());
            events.push(ModelEvent::UsageUpdate { usage });
        }
        if let Some(choice) = chunk.choices.first() {
            let choice_index = choice.index.ok_or_else(|| {
                provider_error("Chat Completions choice lacks an index".to_owned())
            })?;
            if choice_index != 0 {
                return Err(unsupported(format!(
                    "Chat Completions choice index {choice_index} cannot be represented as the single canonical Assistant turn"
                )));
            }
            if let Some(delta) = &choice.delta {
                self.push_delta(delta, &mut events)?;
            }
            if let Some(message) = &choice.message {
                self.push_message_snapshot(message, &mut events)?;
            }
            if let Some(reason) = &choice.finish_reason {
                if reason == "function_call" {
                    return Err(unsupported(
                        "deprecated Chat Completions function_call finish semantics lack a canonical invocation id",
                    ));
                }
                // Some OpenAI-compatible services declare, as their own
                // termination reason, that the model's function call could
                // not be parsed. That is provider-declared malformed tool
                // intent, and it never becomes an ordinary completion.
                if reason.eq_ignore_ascii_case("malformed_function_call")
                    || reason.eq_ignore_ascii_case("malformed_tool_call")
                {
                    return Err(ModelError::malformed_tool_proposal(
                        MalformedToolProposalSource::ProviderDeclared,
                        format!(
                            "the provider terminated the generation with {reason:?}: \
                             it declared the model's tool call malformed"
                        ),
                    ));
                }
                if is_context_window_error("", Some(reason)) {
                    return Err(ModelError {
                        kind: ModelErrorKind::ContextWindowExceeded,
                        message: format!(
                            "provider terminated Chat Completions generation with {reason:?}"
                        ),
                        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                        retry_after_ms: None,
                        provider_code: Some(reason.clone()),
                        context_overflow: None,
                        malformed_tool_proposal: None,
                        timeout_phase: None,
                        generation: None,
                    });
                }
                if matches!(
                    reason.as_str(),
                    "error" | "network_error" | "insufficient_system_resource" | "abort"
                ) {
                    return Err(ModelError {
                        kind: ModelErrorKind::ProviderError,
                        message: format!(
                            "provider terminated Chat Completions generation with {reason:?}"
                        ),
                        retry_disposition: if matches!(
                            reason.as_str(),
                            "network_error" | "insufficient_system_resource"
                        ) {
                            crate::model::error::ModelRetryDisposition::Transient
                        } else {
                            crate::model::error::ModelRetryDisposition::Never
                        },
                        retry_after_ms: None,
                        provider_code: Some(reason.clone()),
                        context_overflow: None,
                        malformed_tool_proposal: None,
                        timeout_phase: None,
                        generation: None,
                    });
                }
                self.finish_reason = Some(map_chat_finish_reason(Some(reason)));
            }
        }
        Ok(events)
    }

    fn push_delta(
        &mut self,
        delta: &ChatDeltaWire,
        events: &mut Vec<ModelEvent>,
    ) -> Result<(), ModelError> {
        if delta.function_call.is_some() {
            return Err(unsupported(
                "deprecated Chat Completions function_call deltas lack a stable invocation id",
            ));
        }
        if delta.audio.as_ref().is_some_and(non_null_value) {
            return Err(unsupported(
                "Chat Completions audio deltas have no canonical representation",
            ));
        }
        if delta
            .reasoning_details
            .as_ref()
            .is_some_and(|details| !details.is_empty())
        {
            return Err(unsupported(
                "structured Chat Completions reasoning_details require lossless replay state",
            ));
        }
        if let Some(reasoning) = delta.reasoning_delta()? {
            append_streamed(&mut self.reasoning_buffer, reasoning);
            events.push(ModelEvent::ReasoningDelta {
                block_index: self.blocks.allocate(ChatBlockKey::Reasoning),
                text: reasoning.to_owned(),
            });
        }
        if let Some(text) = &delta.content
            && !text.is_empty()
        {
            append_streamed(&mut self.text_buffer, text);
            events.push(ModelEvent::TextDelta {
                block_index: self.blocks.allocate(ChatBlockKey::Text),
                text: text.clone(),
            });
        }
        if let Some(refusal) = &delta.refusal
            && !refusal.is_empty()
        {
            append_streamed(&mut self.refusal_buffer, refusal);
            events.push(ModelEvent::RefusalDelta {
                block_index: self.blocks.allocate(ChatBlockKey::Refusal),
                text: refusal.clone(),
            });
        }
        for tool_chunk in delta.tool_calls.as_deref().unwrap_or_default() {
            self.push_tool_call_chunk(tool_chunk, events)?;
        }
        Ok(())
    }

    fn push_message_snapshot(
        &mut self,
        message: &ChatMessageSnapshotWire,
        events: &mut Vec<ModelEvent>,
    ) -> Result<(), ModelError> {
        if message.audio.as_ref().is_some_and(non_null_value) {
            return Err(unsupported(
                "Chat Completions audio output has no canonical representation",
            ));
        }
        if message
            .reasoning_details
            .as_ref()
            .is_some_and(|details| !details.is_empty())
        {
            return Err(unsupported(
                "structured Chat Completions reasoning_details require lossless replay state",
            ));
        }
        let reasoning = message.reasoning_value()?;
        let text = message.content.as_deref();
        let refusal = message.refusal.as_deref();
        validate_snapshot(self.reasoning_buffer.as_deref(), reasoning, "reasoning")?;
        validate_snapshot(self.text_buffer.as_deref(), text, "text")?;
        validate_snapshot(self.refusal_buffer.as_deref(), refusal, "refusal")?;

        if let Some(reasoning) = reasoning
            && self.reasoning_buffer.is_none()
        {
            self.reasoning_buffer = Some(reasoning.to_owned());
            if !reasoning.is_empty() {
                events.push(ModelEvent::ReasoningDelta {
                    block_index: self.blocks.allocate(ChatBlockKey::Reasoning),
                    text: reasoning.to_owned(),
                });
            }
        }
        if let Some(text) = text
            && self.text_buffer.is_none()
        {
            self.text_buffer = Some(text.to_owned());
            if !text.is_empty() {
                events.push(ModelEvent::TextDelta {
                    block_index: self.blocks.allocate(ChatBlockKey::Text),
                    text: text.to_owned(),
                });
            }
        }
        if let Some(refusal) = refusal
            && self.refusal_buffer.is_none()
        {
            self.refusal_buffer = Some(refusal.to_owned());
            if !refusal.is_empty() {
                events.push(ModelEvent::RefusalDelta {
                    block_index: self.blocks.allocate(ChatBlockKey::Refusal),
                    text: refusal.to_owned(),
                });
            }
        }
        for (index, snapshot) in message
            .tool_calls
            .as_deref()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let index = u32::try_from(index)
                .map_err(|_| provider_error("too many tool call snapshots".to_owned()))?;
            self.push_tool_call_snapshot(index, snapshot, events)?;
        }
        Ok(())
    }

    fn push_tool_call_snapshot(
        &mut self,
        index: u32,
        snapshot: &ChatToolCallSnapshotWire,
        events: &mut Vec<ModelEvent>,
    ) -> Result<(), ModelError> {
        if snapshot.mcp.as_ref().is_some_and(non_null_value) {
            return Err(unsupported(
                "provider-hosted MCP calls are not canonical rustX function calls",
            ));
        }
        if let Some(existing) = self.tool_calls.get(&index) {
            if !existing.started {
                let has_argument_fragment = snapshot
                    .function
                    .as_ref()
                    .and_then(|function| function.arguments.as_deref())
                    .is_some_and(|arguments| !arguments.is_empty());
                self.merge_tool_call_snapshot(index, snapshot)?;
                let identity_complete = self
                    .tool_calls
                    .get(&index)
                    .is_some_and(|assembly| assembly.call_id.is_some() && assembly.name.is_some());
                if has_argument_fragment && !identity_complete {
                    self.buffered_generation = true;
                }
                return self.start_buffered_tool_call(index, events);
            }
            let snapshot_id = snapshot.id.as_deref().filter(|id| !id.is_empty());
            if snapshot_id.is_some_and(|id| {
                existing
                    .call_id
                    .as_ref()
                    .is_some_and(|known| known.as_str() != id)
            }) {
                return Err(provider_error(format!(
                    "tool call snapshot changed the invocation id at index {index}"
                )));
            }
            if let Some(function) = &snapshot.function {
                if function
                    .name
                    .as_ref()
                    .is_some_and(|name| existing.name.as_ref().is_some_and(|known| known != name))
                {
                    return Err(provider_error(format!(
                        "tool call snapshot changed the function name at index {index}"
                    )));
                }
                if function.arguments.as_ref().is_some_and(|arguments| {
                    !existing.arguments.is_empty() && existing.arguments != *arguments
                }) {
                    return Err(provider_error(format!(
                        "tool call snapshot disagrees with streamed arguments at index {index}"
                    )));
                }
            }
            return Ok(());
        }
        self.push_tool_call_chunk(
            &ChatToolCallChunkWire {
                index,
                id: snapshot.id.clone(),
                tool_type: snapshot.tool_type.clone(),
                custom: snapshot.custom.clone(),
                mcp: snapshot.mcp.clone(),
                function: snapshot.function.clone(),
            },
            events,
        )
    }

    fn merge_tool_call_snapshot(
        &mut self,
        index: u32,
        snapshot: &ChatToolCallSnapshotWire,
    ) -> Result<(), ModelError> {
        let assembly = self
            .tool_calls
            .get_mut(&index)
            .expect("snapshot merge has an existing assembly");
        if let Some(id) = snapshot.id.as_deref().filter(|id| !id.is_empty()) {
            let id = ToolCallId::new(id);
            if assembly.call_id.as_ref().is_some_and(|known| known != &id) {
                return Err(provider_error(format!(
                    "tool call snapshot changed the invocation id at index {index}"
                )));
            }
            assembly.call_id = Some(id);
        }
        if let Some(function) = &snapshot.function {
            if let Some(name) = &function.name {
                if assembly.name.as_ref().is_some_and(|known| known != name) {
                    return Err(provider_error(format!(
                        "tool call snapshot changed the function name at index {index}"
                    )));
                }
                assembly.name = Some(name.clone());
            }
            if let Some(arguments) = &function.arguments {
                if assembly.arguments.is_empty() || arguments.starts_with(&assembly.arguments) {
                    arguments.clone_into(&mut assembly.arguments);
                } else if !assembly.arguments.starts_with(arguments) {
                    return Err(provider_error(format!(
                        "tool call snapshot disagrees with streamed arguments at index {index}"
                    )));
                }
            }
        }
        Ok(())
    }

    fn start_buffered_tool_call(
        &mut self,
        index: u32,
        events: &mut Vec<ModelEvent>,
    ) -> Result<(), ModelError> {
        let (block_index, call_id, name, arguments) = {
            let assembly = self
                .tool_calls
                .get(&index)
                .expect("buffered tool call has an assembly");
            if assembly.started {
                return Ok(());
            }
            let Some(call_id) = assembly.call_id.clone() else {
                return Ok(());
            };
            let Some(name) = assembly.name.clone() else {
                return Ok(());
            };
            (
                assembly.block_index,
                call_id,
                name,
                assembly.arguments.clone(),
            )
        };
        // Identity resolution: the proposal becomes attributable, which is
        // what licenses the canonical start event. This is not yet ToolCall
        // acceptance — no executable call exists until the arguments are
        // accepted at block end.
        let start =
            resolve_tool_identity(Some(call_id.as_str()), Some(name.as_str()), &self.tools)?;
        let assembly = self
            .tool_calls
            .get_mut(&index)
            .expect("buffered tool call has an assembly");
        assembly.started = true;
        events.push(ModelEvent::ToolCallStarted {
            block_index,
            call: start,
        });
        if !arguments.is_empty() {
            events.push(ModelEvent::ToolCallArgumentsDelta {
                block_index,
                call_id,
                arguments_delta: arguments,
            });
        }
        Ok(())
    }

    fn push_tool_call_chunk(
        &mut self,
        chunk: &ChatToolCallChunkWire,
        events: &mut Vec<ModelEvent>,
    ) -> Result<(), ModelError> {
        if chunk
            .tool_type
            .as_deref()
            .is_some_and(|kind| kind != "function")
            || chunk.custom.as_ref().is_some_and(non_null_value)
            || chunk.mcp.as_ref().is_some_and(non_null_value)
        {
            return Err(unsupported(
                "custom Chat Completions tool calls cannot be represented as canonical JSON function calls",
            ));
        }
        self.mark_buffered_generation(chunk);
        let assembly = self
            .tool_calls
            .entry(chunk.index)
            .or_insert_with(|| ToolAssembly {
                block_index: self.blocks.allocate(ChatBlockKey::Tool(chunk.index)),
                call_id: None,
                name: None,
                arguments: String::new(),
                started: false,
            });
        if let Some(id) = &chunk.id {
            let id = ToolCallId::new(id.clone());
            if assembly.call_id.as_ref().is_some_and(|known| known != &id) {
                return Err(provider_error(format!(
                    "provider changed the invocation id of tool call index {}",
                    chunk.index
                )));
            }
            assembly.call_id = Some(id);
        }
        if let Some(function) = &chunk.function {
            if let Some(name) = &function.name {
                if assembly.name.as_ref().is_some_and(|known| known != name) {
                    return Err(provider_error(format!(
                        "provider changed the function name of tool call index {}",
                        chunk.index
                    )));
                }
                assembly.name = Some(name.clone());
            }
            if let Some(arguments) = &function.arguments {
                assembly.arguments.push_str(arguments);
            }
        }
        let mut started_now = false;
        if !assembly.started {
            let Some(call_id) = &assembly.call_id else {
                // Identity is not yet known; argument fragments stay buffered
                // in `assembly.arguments` until the call can start.
                return Ok(());
            };
            let Some(name) = &assembly.name else {
                return Ok(());
            };
            let start =
                resolve_tool_identity(Some(call_id.as_str()), Some(name.as_str()), &self.tools)?;
            assembly.started = true;
            started_now = true;
            events.push(ModelEvent::ToolCallStarted {
                block_index: assembly.block_index,
                call: start,
            });
        }
        let arguments_delta = if started_now {
            // Identity can arrive after one or more argument chunks. Emit the
            // complete buffered prefix exactly once when the call becomes
            // attributable.
            assembly.arguments.clone()
        } else {
            chunk
                .function
                .as_ref()
                .and_then(|function| function.arguments.clone())
                .unwrap_or_default()
        };
        if !arguments_delta.is_empty() {
            let call_id = assembly.call_id.clone().expect("call id known after start");
            events.push(ModelEvent::ToolCallArgumentsDelta {
                block_index: assembly.block_index,
                call_id,
                arguments_delta,
            });
        }
        Ok(())
    }

    fn mark_buffered_generation(&mut self, chunk: &ChatToolCallChunkWire) {
        let has_argument_fragment = chunk
            .function
            .as_ref()
            .and_then(|function| function.arguments.as_deref())
            .is_some_and(|arguments| !arguments.is_empty());
        let call_id_known = self
            .tool_calls
            .get(&chunk.index)
            .and_then(|assembly| assembly.call_id.as_ref())
            .is_some()
            || chunk.id.is_some();
        let name_known = self
            .tool_calls
            .get(&chunk.index)
            .and_then(|assembly| assembly.name.as_ref())
            .is_some()
            || chunk
                .function
                .as_ref()
                .and_then(|function| function.name.as_ref())
                .is_some();
        let call_is_unstarted = self
            .tool_calls
            .get(&chunk.index)
            .is_none_or(|assembly| !assembly.started);
        if has_argument_fragment && call_is_unstarted && !(call_id_known && name_known) {
            self.buffered_generation = true;
        }
    }

    /// Produces the terminal events for the invocation once the provider
    /// stream has ended: completes in-flight tool calls (parsing their
    /// complete JSON exactly once) and emits `Completed`, or fails when the
    /// provider ended without a legitimate terminal condition.
    fn finish(&mut self) -> Result<Vec<ModelEvent>, ModelError> {
        let mut events = Vec::new();
        let assemblies: Vec<ToolAssembly> = self
            .tool_calls
            .values()
            .map(|assembly| ToolAssembly {
                block_index: assembly.block_index,
                call_id: assembly.call_id.clone(),
                name: assembly.name.clone(),
                arguments: assembly.arguments.clone(),
                started: assembly.started,
            })
            .collect();
        let assembled_call_count = assemblies.len();
        for assembly in assemblies {
            // The ToolCall acceptance linearization point: the complete
            // argument text is parsed exactly once, and only on success does
            // the full canonical ToolCall exist. Nothing is repaired to make
            // a proposal fit.
            let start = resolve_tool_identity(
                assembly.call_id.as_ref().map(ToolCallId::as_str),
                assembly.name.as_deref(),
                &self.tools,
            )?;
            events.push(ModelEvent::ToolCallCompleted {
                block_index: assembly.block_index,
                call: accept_tool_call_arguments(&start, &assembly.arguments)?,
            });
        }
        let finish_reason = self.finish_reason.take().ok_or_else(|| {
            provider_error("provider stream ended without a finish reason".to_owned())
        })?;
        if assembled_call_count == 0
            && let Some(residue) = self.reserved_protocol_residue(&finish_reason)
        {
            return Err(residue);
        }
        events.push(ModelEvent::Completed {
            finish_reason,
            usage: self.usage.take(),
        });
        Ok(events)
    }

    /// The malformed-proposal failure of a generation that leaked reserved
    /// in-band tool-protocol markup instead of emitting a structured call.
    ///
    /// This is the one piece of Qwen-shaped protocol knowledge in the
    /// runtime, and it lives where every other Chat Completions dialect
    /// difference lives. It is deliberately narrow, and every condition is
    /// independent evidence:
    ///
    /// - the model must *declare* an in-band tool protocol through
    ///   `compat.chatToolProtocol`; the default `native` profile never
    ///   inspects output at all, and the dialect is never inferred from a
    ///   model name, a provider name, or a hostname;
    /// - tools must actually have been exposed to the model, so reserved
    ///   markup is meaningful in this request;
    /// - the generation must have produced no structured tool call;
    /// - the provider must have terminated as a *complete normal*
    ///   generation, so a truncated (`length`) or filtered stream is not
    ///   reinterpreted;
    /// - the output must contain an actual *emission* of the declared
    ///   dialect — a correctly ordered, identifier-bearing reserved region
    ///   that the generation's own output *is*, rather than material inside
    ///   a document the generation is writing — and not merely the reserved
    ///   tokens somewhere in text. [`qwen_xml`] owns that recognition,
    ///   follows the dialect's reserved grammar rather than any
    ///   pretty-printed newline layout, and documents why quoting or
    ///   discussing the exact syntax is not an emission.
    ///
    /// Recognition proves a leak; it never reconstructs one. The leaked
    /// region is not parsed back into a `ToolCall`, because a proposal this
    /// runtime had to infer is exactly the invented model intent the
    /// acceptance boundary exists to refuse.
    fn reserved_protocol_residue(
        &self,
        finish_reason: &crate::model::finish::ModelFinishReason,
    ) -> Option<ModelError> {
        use crate::model::finish::ModelFinishReason;

        if self.tool_protocol == ChatToolProtocol::Native || self.tools.is_empty() {
            return None;
        }
        if !matches!(
            finish_reason,
            ModelFinishReason::Stop | ModelFinishReason::ToolCalls
        ) {
            return None;
        }
        let envelope = [
            self.text_buffer.as_deref(),
            self.reasoning_buffer.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find_map(|output| reserved_protocol_emission(self.tool_protocol, output))?;
        Some(ModelError::malformed_tool_proposal(
            MalformedToolProposalSource::ReservedProtocolLeak,
            format!(
                "the model emitted the reserved {} tool-protocol region {envelope} into \
                 ordinary output and produced no structured tool call",
                self.tool_protocol.as_str(),
            ),
        ))
    }
}

fn provider_error(message: String) -> ModelError {
    ModelError {
        kind: ModelErrorKind::ProviderError,
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

fn non_null_value(value: &serde_json::Value) -> bool {
    !value.is_null()
}

fn value_has_output(value: Option<&serde_json::Value>) -> bool {
    value.is_some_and(|value| match value {
        serde_json::Value::Null => false,
        serde_json::Value::Array(values) => !values.is_empty(),
        serde_json::Value::Object(values) => !values.is_empty(),
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(_) => true,
    })
}

fn chat_stream_error(error: &serde_json::Value) -> ModelError {
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown OpenAI-compatible stream error");
    let metadata = error.get("metadata");
    let error_type = metadata
        .and_then(|value| value.get("error_type"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| error.get("type").and_then(serde_json::Value::as_str));
    let numeric_code = error.get("code").and_then(serde_json::Value::as_u64);
    let provider_code = metadata
        .and_then(|value| value.get("provider_code"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            error.get("code").and_then(|code| match code {
                serde_json::Value::String(code) => Some(code.clone()),
                serde_json::Value::Number(code) => Some(code.to_string()),
                _ => None,
            })
        })
        .or_else(|| error_type.map(str::to_owned));
    let provider_code = provider_code.as_deref();
    let kind = if matches!(
        error_type,
        Some("authentication" | "authentication_error" | "permission_error" | "401" | "403")
    ) || matches!(
        provider_code,
        Some("authentication" | "authentication_error" | "permission_error" | "401" | "403")
    ) || matches!(numeric_code, Some(401 | 403))
    {
        ModelErrorKind::Authentication
    } else if matches!(
        error_type,
        Some("rate_limit_exceeded" | "rate_limit_error" | "429")
    ) || matches!(
        provider_code,
        Some("rate_limit_exceeded" | "rate_limit_error" | "429")
    ) || numeric_code == Some(429)
    {
        ModelErrorKind::RateLimit
    } else if matches!(error_type, Some("timeout" | "408"))
        || matches!(provider_code, Some("timeout" | "408"))
        || numeric_code == Some(408)
    {
        ModelErrorKind::Timeout
    } else if is_context_window_error(message, provider_code)
        || is_context_window_error(message, error_type)
    {
        ModelErrorKind::ContextWindowExceeded
    } else if matches!(
        error_type,
        Some("invalid_request" | "invalid_prompt" | "invalid_request_error" | "400")
    ) || matches!(
        provider_code,
        Some("invalid_request" | "invalid_prompt" | "invalid_request_error" | "400")
    ) || numeric_code == Some(400)
    {
        ModelErrorKind::InvalidRequest
    } else {
        ModelErrorKind::ProviderError
    };
    ModelError {
        kind,
        message: format!("OpenAI-compatible stream error: {message}"),
        retry_disposition: stream_retry_disposition(
            error_type,
            provider_code,
            numeric_code,
            message,
        ),
        retry_after_ms: None,
        provider_code: provider_code.map(str::to_owned),
        context_overflow: None,
        malformed_tool_proposal: None,
        timeout_phase: None,
        generation: None,
    }
    .normalized()
}

#[derive(Serialize)]
pub(crate) struct WireRequest<'a> {
    model: &'a str,
    messages: WireMessages<'a>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<WireStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<std::sync::Arc<serde_json::value::RawValue>>,
    #[serde(flatten)]
    params: Cow<'a, crate::model::invocation::RequestParams>,
}

#[derive(Serialize)]
struct WireStreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct WireTool<'a> {
    r#type: &'static str,
    function: WireFunction<'a>,
}

#[derive(Serialize)]
struct WireFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a serde_json::Value,
}

#[derive(Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum WireMessage<'a> {
    System {
        content: &'a str,
    },
    User {
        content: Vec<WirePart<'a>>,
    },
    Assistant {
        content: AssistantContent<'a>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<WireCall<'a>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<Cow<'a, str>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<Cow<'a, str>>,
    },
    Tool {
        tool_call_id: serde_json::Value,
        content: Vec<WirePart<'a>>,
    },
}

#[derive(Serialize)]
#[serde(untagged)]
enum AssistantContent<'a> {
    Parts(Vec<WirePart<'a>>),
    Text(String),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WirePart<'a> {
    Text { text: Cow<'a, str> },
    Refusal { refusal: &'a str },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Serialize)]
struct ImageUrl {
    url: String,
}

#[derive(Serialize)]
struct WireCall<'a> {
    id: serde_json::Value,
    r#type: &'static str,
    function: WireCallFunction<'a>,
}

#[derive(Serialize)]
struct WireCallFunction<'a> {
    name: &'a str,
    arguments: String,
}

#[derive(Default)]
pub(crate) struct ChatPrefix {
    prefix: crate::model::adapter::cache::Prefix,
    replay: ChatReasoningReplay,
    origin: Option<crate::model::replay::ReplayOrigin>,
    images: crate::model::images::ResolvedImages,
    epoch: u64,
    pieces: imbl::Vector<Option<std::sync::Arc<serde_json::value::RawValue>>>,
    count: usize,
    bytes: usize,
}

enum WireMessages<'a> {
    Cached {
        system: &'a str,
        pieces: imbl::Vector<Option<std::sync::Arc<serde_json::value::RawValue>>>,
        count: usize,
        bytes: usize,
    },
    #[cfg(test)]
    Uncached(Vec<WireMessage<'a>>),
}
impl Serialize for WireMessages<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        match self {
            Self::Cached {
                system,
                pieces,
                count,
                ..
            } => {
                let mut array =
                    serializer.serialize_seq(Some(count + usize::from(!system.is_empty())))?;
                if !system.is_empty() {
                    array.serialize_element(&WireMessage::System { content: system })?;
                }
                for piece in pieces.iter().flatten() {
                    array.serialize_element(piece.as_ref())?;
                }
                array.end()
            }
            #[cfg(test)]
            Self::Uncached(messages) => messages.serialize(serializer),
        }
    }
}
impl WireRequest<'_> {
    pub(crate) fn capacity_hint(&self) -> usize {
        match &self.messages {
            WireMessages::Cached {
                system,
                bytes,
                count,
                ..
            } => bytes
                .saturating_add(*count)
                .saturating_add(system.len())
                .saturating_add(1024)
                .saturating_add(self.tools.as_ref().map_or(0, |tools| tools.get().len())),
            #[cfg(test)]
            WireMessages::Uncached(_) => 128,
        }
    }
}

#[cfg(test)]
pub(crate) fn translate_request<'a>(
    request: &'a ModelRequest,
    tools: &ValidatedTools,
) -> Result<WireRequest<'a>, ModelError> {
    translate_request_with_limit(request, tools, None)
}

pub(crate) fn translate_request_with_limit<'a>(
    request: &'a ModelRequest,
    tools: &ValidatedTools,
    limit: Option<usize>,
) -> Result<WireRequest<'a>, ModelError> {
    #[cfg(feature = "issue-387-profile")]
    let _profile =
        crate::model::adapter::profile::phase(crate::model::adapter::profile::Phase::Translate);
    let staged = stage_messages(request, &tools.correlation)?;
    let params = crate::model::adapter::structured::request_params(request)?;
    let catalog = tools.encoded_tools(
        ModelProtocol::OpenAiChatCompletions,
        limit,
        &request.tools,
        || translate_tools(&request.tools),
    )?;
    let input_limit = limit
        .map(|limit| {
            limit
                .checked_sub(catalog.as_ref().map_or(0, |raw| raw.get().len()))
                .ok_or_else(crate::model::adapter::validation::wire_budget_error)
        })
        .transpose()?;
    let messages = finish_messages(request, &tools.correlation, staged, input_limit)?;
    Ok(wire_request(request, messages, catalog, params))
}

struct StagedMessages<'a> {
    prefix: ChatPrefix,
    translated: Vec<(usize, Option<WireMessage<'a>>)>,
}

fn stage_messages<'a>(
    request: &'a ModelRequest,
    plan: &crate::model::adapter::correlation::Plan,
) -> Result<StagedMessages<'a>, ModelError> {
    #[cfg(feature = "issue-387-profile")]
    let _profile = crate::model::adapter::profile::phase(
        crate::model::adapter::profile::Phase::TranslateMessages,
    );
    let replay = request
        .invocation
        .compat
        .chat_reasoning_replay
        .unwrap_or_default();
    let origin = request.invocation.replay_origin();
    let mut prefix = request
        .messages
        .preparation()
        .lock()
        .expect("input preparation cache")
        .chat
        .take()
        .filter(|prefix| {
            prefix.epoch == plan.epoch
                && prefix.replay == replay
                && prefix.origin == origin
                && prefix.images == request.images
                && prefix.prefix.matches(&request.messages)
        })
        .unwrap_or_else(|| ChatPrefix {
            replay,
            origin: origin.clone(),
            images: request.images.clone(),
            epoch: plan.epoch,
            ..ChatPrefix::default()
        });
    let mut dirty = plan.dirty.clone();
    for index in prefix.prefix.len()..request.messages.len() {
        prefix.pieces.push_back(None);
        dirty.insert(index);
    }
    let mut translated = Vec::with_capacity(dirty.len());
    for index in dirty {
        if index < request.messages.len() {
            translated.push((
                index,
                translate_message(
                    &request.messages[index],
                    &request.images,
                    replay,
                    origin.as_ref(),
                )?,
            ));
        }
    }
    let count = prefix.count
        - translated
            .iter()
            .filter(|(index, _)| prefix.pieces[*index].is_some())
            .count()
        + translated
            .iter()
            .filter(|(_, message)| message.is_some())
            .count();
    if count == 0 && request.effective_system_prompt.is_empty() {
        return Err(invalid_request(
            "a Chat Completions request requires at least one message",
        ));
    }
    plan.validate_wire().map_err(invalid_request)?;
    for (index, message) in &mut translated {
        if let Some(message) = message {
            plan.encode_message_fields(*index, &request.messages[*index], |field| match message {
                WireMessage::Assistant { tool_calls, .. } => {
                    for call in tool_calls {
                        field(true, &mut call.id);
                    }
                }
                WireMessage::Tool { tool_call_id, .. } => field(false, tool_call_id),
                _ => {}
            })
            .map_err(invalid_request)?;
        }
    }
    Ok(StagedMessages { prefix, translated })
}

fn finish_messages<'a>(
    request: &'a ModelRequest,
    plan: &crate::model::adapter::correlation::Plan,
    staged: StagedMessages<'a>,
    limit: Option<usize>,
) -> Result<WireMessages<'a>, ModelError> {
    let StagedMessages {
        mut prefix,
        translated,
    } = staged;
    for (index, _) in &translated {
        if let Some(previous) = prefix.pieces[*index].take() {
            prefix.bytes -= previous.get().len();
            prefix.count -= 1;
        }
    }
    if limit.is_some_and(|limit| prefix.bytes > limit) {
        return Err(crate::model::adapter::validation::wire_budget_error());
    }
    for (index, message) in translated {
        let raw = message
            .as_ref()
            .map(|message| {
                crate::model::adapter::cache::encode_raw(
                    message,
                    limit.map(|limit| limit - prefix.bytes),
                )
            })
            .transpose()?;
        #[cfg(test)]
        crate::model::adapter::cache::work(|work| {
            work.encoded_items += 1;
            work.encoded_bytes += raw.as_ref().map_or(0, |raw| raw.get().len());
        });
        if let Some(raw) = &raw {
            prefix.bytes += raw.get().len();
            prefix.count += 1;
        }
        prefix.pieces.set(index, raw);
    }
    prefix.prefix = crate::model::adapter::cache::Prefix::capture(&request.messages);
    let messages = WireMessages::Cached {
        system: &request.effective_system_prompt,
        pieces: prefix.pieces.clone(),
        count: prefix.count,
        bytes: prefix.bytes,
    };
    let mut cache = request
        .messages
        .preparation()
        .lock()
        .expect("input preparation cache");
    if let Some(current) = cache
        .correlation
        .as_mut()
        .filter(|current| current.epoch == plan.epoch && current.matches(&request.messages))
    {
        current.dirty.clear();
        cache.chat = Some(prefix);
    }
    Ok(messages)
}

fn wire_request<'a>(
    request: &'a ModelRequest,
    messages: WireMessages<'a>,
    catalog: Option<std::sync::Arc<serde_json::value::RawValue>>,
    params: Cow<'a, crate::model::invocation::RequestParams>,
) -> WireRequest<'a> {
    let max = Some(request.max_output_tokens());
    let (max_tokens, max_completion_tokens) = match request.invocation.compat.chat_max_tokens_field
    {
        ChatMaxTokensField::MaxTokens => (max, None),
        ChatMaxTokensField::MaxCompletionTokens => (None, max),
    };
    WireRequest {
        model: request.model(),
        messages,
        stream: true,
        max_tokens,
        max_completion_tokens,
        stream_options: (request.invocation.compat.chat_stream_usage == ChatStreamUsage::Supported)
            .then_some(WireStreamOptions {
                include_usage: true,
            }),
        tools: catalog,
        params,
    }
}

#[cfg(test)]
pub(crate) fn uncached_request(request: &ModelRequest) -> Result<WireRequest<'_>, ModelError> {
    let tools = crate::model::adapter::validation::validate_request(
        request,
        ModelProtocol::OpenAiChatCompletions,
    )?;
    let mut messages = Vec::new();
    if !request.effective_system_prompt.is_empty() {
        messages.push(WireMessage::System {
            content: &request.effective_system_prompt,
        });
    }
    let origin = request.invocation.replay_origin();
    for block in &request.messages {
        if let Some(message) = translate_message(
            block,
            &request.images,
            request
                .invocation
                .compat
                .chat_reasoning_replay
                .unwrap_or_default(),
            origin.as_ref(),
        )? {
            messages.push(message);
        }
    }
    let raw = request
        .messages
        .iter()
        .flat_map(|message| match message.view() {
            ModelMessageView::Assistant(content) => content
                .iter()
                .filter_map(|block| {
                    if let AssistantContentBlock::ToolCall(call) = block {
                        Some(call.id.to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    let labels =
        crate::model::adapter::correlation::wire_labels(&raw, ModelProtocol::OpenAiChatCompletions);
    crate::model::adapter::correlation::encode_fields(&labels, |field| {
        for message in &mut messages {
            match message {
                WireMessage::Assistant { tool_calls, .. } => {
                    for call in tool_calls {
                        field(true, &mut call.id);
                    }
                }
                WireMessage::Tool { tool_call_id, .. } => field(false, tool_call_id),
                _ => {}
            }
        }
    })
    .map_err(invalid_request)?;
    Ok(wire_request(
        request,
        WireMessages::Uncached(messages),
        tools.encoded_tools(
            ModelProtocol::OpenAiChatCompletions,
            None,
            &request.tools,
            || translate_tools(&request.tools),
        )?,
        crate::model::adapter::structured::request_params(request)?,
    ))
}

fn translate_message<'a>(
    block: &'a crate::model::ModelInputMessage,
    images: &crate::model::images::ResolvedImages,
    replay: ChatReasoningReplay,
    origin: Option<&crate::model::replay::ReplayOrigin>,
) -> Result<Option<WireMessage<'a>>, ModelError> {
    let translated = match block.view() {
        ModelMessageView::User(user) => {
            let mut content = Vec::with_capacity(user.len());
            for block in user {
                #[cfg(feature = "issue-387-profile")]
                crate::model::adapter::profile::block();
                match block {
                    crate::message::types::UserContentBlock::Text(text) => {
                        content.push(WirePart::Text {
                            text: Cow::Borrowed(&text.text),
                        });
                    }
                    crate::message::types::UserContentBlock::Image(image) => {
                        content.push(chat_image_part(image, images)?);
                    }
                    _ => {
                        return Err(unsupported(
                            "OpenAI Chat Completions cannot represent canonical file references",
                        ));
                    }
                }
            }
            WireMessage::User { content }
        }
        ModelMessageView::Assistant(assistant) => {
            let translated = translate_assistant_message(assistant, replay, origin)?;
            if matches!(&translated, WireMessage::Assistant { content: AssistantContent::Parts(parts), tool_calls, reasoning: None, reasoning_content: None } if parts.is_empty() && tool_calls.is_empty())
            {
                return Ok(None);
            }
            translated
        }
        ModelMessageView::Tool(tool_call_id, result) => {
            translate_tool_message(tool_call_id, result, images)?
        }
        ModelMessageView::RequestOnly(carryover) if carryover.is_assistant() => {
            WireMessage::Assistant {
                content: AssistantContent::Text(carryover.render()),
                tool_calls: Vec::new(),
                reasoning: None,
                reasoning_content: None,
            }
        }
        ModelMessageView::RequestOnly(carryover) => WireMessage::User {
            content: vec![WirePart::Text {
                text: Cow::Owned(carryover.render()),
            }],
        },
    };
    Ok(Some(translated))
}

fn translate_tool_message<'a>(
    tool_call_id: &ToolCallId,
    result: &crate::tools::types::ToolExecutionResult,
    images: &crate::model::images::ResolvedImages,
) -> Result<WireMessage<'a>, ModelError> {
    let content = result
        .image_projection()
        .map_err(unsupported)?
        .into_iter()
        .map(|part| match part {
            crate::tools::types::ModelToolResultPart::Text(text) => Ok(WirePart::Text {
                text: Cow::Owned(text),
            }),
            crate::tools::types::ModelToolResultPart::Image(image) => {
                chat_image_part(&image, images)
            }
        })
        .collect::<Result<_, _>>()?;
    Ok(WireMessage::Tool {
        tool_call_id: tool_call_id.as_str().into(),
        content,
    })
}

fn translate_tools(tools: &[crate::tools::types::ModelToolDefinition]) -> Vec<WireTool<'_>> {
    tools
        .iter()
        .map(|tool| WireTool {
            r#type: "function",
            function: WireFunction {
                name: &tool.name,
                description: &tool.description,
                parameters: &tool.input_schema,
            },
        })
        .collect()
}

fn translate_assistant_message<'a>(
    assistant: &'a [AssistantContentBlock],
    replay: ChatReasoningReplay,
    origin: Option<&crate::model::replay::ReplayOrigin>,
) -> Result<WireMessage<'a>, ModelError> {
    let mut parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut reasoning: Option<Cow<'_, str>> = None;
    for content in assistant {
        #[cfg(feature = "issue-387-profile")]
        crate::model::adapter::profile::block();
        match content {
            AssistantContentBlock::Text(text) => parts.push(WirePart::Text {
                text: Cow::Borrowed(&text.text),
            }),
            AssistantContentBlock::Refusal(refusal) => parts.push(WirePart::Refusal {
                refusal: &refusal.text,
            }),
            AssistantContentBlock::ToolCall(call) => tool_calls.push(WireCall {
                id: call.id.as_str().into(),
                r#type: "function",
                function: WireCallFunction {
                    name: &call.name,
                    arguments: serde_json::to_string(&call.arguments).map_err(|error| {
                        unsupported(format!(
                            "tool call arguments are not JSON-serializable: {error}"
                        ))
                    })?,
                },
            }),
            AssistantContentBlock::Reasoning(block) => {
                if !block.belongs_to(origin) {
                    if let Some(text) = block.text.as_deref().filter(|text| !text.trim().is_empty())
                    {
                        parts.push(WirePart::Text {
                            text: Cow::Borrowed(text),
                        });
                    }
                    continue;
                }
                if replay != ChatReasoningReplay::Omit
                    && let Some(text) = block.text.as_deref().filter(|text| !text.is_empty())
                {
                    if let Some(previous) = &mut reasoning {
                        previous.to_mut().push_str(text);
                    } else {
                        reasoning = Some(Cow::Borrowed(text));
                    }
                }
            }
            AssistantContentBlock::Image(_) => {
                return Err(unsupported(
                    "OpenAI Chat Completions cannot represent generated image references",
                ));
            }
        }
    }
    let (reasoning, reasoning_content) = match replay {
        ChatReasoningReplay::Reasoning => (reasoning, None),
        ChatReasoningReplay::ReasoningContent => (None, reasoning),
        ChatReasoningReplay::Omit => (None, None),
    };
    Ok(WireMessage::Assistant {
        content: AssistantContent::Parts(parts),
        tool_calls,
        reasoning,
        reasoning_content,
    })
}

fn chat_image_part<'a>(
    image: &crate::message::content::ImageReference,
    images: &crate::model::images::ResolvedImages,
) -> Result<WirePart<'a>, ModelError> {
    Ok(WirePart::ImageUrl {
        image_url: ImageUrl {
            url: super::image_data_url(image, images).map_err(|error| unsupported(&error))?,
        },
    })
}

fn unsupported(message: impl Into<String>) -> ModelError {
    let message = message.into();
    ModelError {
        kind: ModelErrorKind::Unsupported,
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

fn invalid_request(message: &str) -> ModelError {
    ModelError {
        kind: ModelErrorKind::InvalidRequest,
        message: message.to_owned(),
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
        malformed_tool_proposal: None,
        timeout_phase: None,
        generation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatStreamNormalizer, translate_tool_message, translate_tools};
    use crate::message::content::TextBlock;
    use crate::message::types::ToolMessageBlock;
    use crate::model::adapter::validation::ValidatedTools;
    use crate::model::catalog::ChatToolProtocol;
    use crate::model::error::{ModelErrorKind, ModelRetryDisposition};
    use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
    use crate::runtime::types::CancellationReason;
    use crate::tools::types::{
        ModelToolDefinition, ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus,
        ToolResultContent,
    };
    use serde_json::json;

    #[test]
    fn chat_completions_preserves_the_nested_questionnaire_schema() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["questions"],
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["question", "header", "options"],
                        "properties": {
                            "question": {"type": "string", "maxLength": 4096},
                            "header": {"type": "string", "maxLength": 16},
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["label", "description"],
                                    "properties": {
                                        "label": {"type": "string", "maxLength": 60},
                                        "description": {"type": "string", "maxLength": 1024},
                                        "preview": {"type": "string", "maxLength": 8192}
                                    }
                                }
                            },
                            "multi_select": {"type": "boolean"}
                        }
                    }
                }
            }
        });
        let encoded = serde_json::to_value(translate_tools(&[ModelToolDefinition {
            id: crate::runtime::identity::ToolId::new("tool-ask-user"),
            name: "ask_user".to_owned(),
            description: "structured questionnaire".to_owned(),
            input_schema: schema.clone().into(),
        }]))
        .expect("Chat Completions tools serialize");
        assert_eq!(encoded[0]["function"]["name"], "ask_user");
        assert_eq!(encoded[0]["function"]["parameters"], schema);
    }

    #[test]
    fn chat_base_response_status_uses_structured_retry_evidence() {
        let mut normalizer =
            ChatStreamNormalizer::new(ValidatedTools::default().into(), ChatToolProtocol::Native);
        let error = normalizer
            .push(json!({
                "base_resp": {"status_code": 500, "status_msg": "upstream unavailable"}
            }))
            .expect_err("a non-zero base response is a provider failure");
        assert_eq!(error.kind, ModelErrorKind::ProviderError);
        assert_eq!(
            error.retry_disposition,
            ModelRetryDisposition::Transient,
            "a structured 5xx status is retryable evidence"
        );
        assert_eq!(error.provider_code.as_deref(), Some("500"));

        let error = normalizer
            .push(json!({
                "base_resp": {"status_code": 409, "status_msg": "request rejected"}
            }))
            .expect_err("a second non-zero base response is a provider failure");
        assert_eq!(error.retry_disposition, ModelRetryDisposition::Never);
    }

    #[test]
    fn chat_in_band_error_precedes_typed_chunk_decoding() {
        let mut normalizer =
            ChatStreamNormalizer::new(ValidatedTools::default().into(), ChatToolProtocol::Native);
        let error = normalizer
            .push(json!({
                "choices": "not a typed choice list",
                "error": {"code": 429, "message": "upstream rate limit"}
            }))
            .expect_err("in-band errors are classified before decoding the rest of the chunk");
        assert_eq!(error.kind, ModelErrorKind::RateLimit);
        assert_eq!(error.provider_code.as_deref(), Some("429"));
        assert_eq!(error.retry_disposition, ModelRetryDisposition::Transient);
    }

    #[test]
    fn issue136_chat_translation_consumes_typed_cancellation_status() {
        let message = ToolMessageBlock {
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                crate::runtime::identity::MessageId::new("assistant"),
                crate::message::types::ContentBlockIndex::new(0),
            ),
            id: MessageId::new("tool-result-1"),
            tool_call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-1"),
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Cancelled {
                    reason: CancellationReason::RuntimeShutdown,
                    phase: ToolCancellationPhase::BeforeStart,
                },
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
            }
            .into(),
        };
        let encoded = serde_json::to_string(
            &translate_tool_message(
                &message.tool_call_id,
                &message.result,
                &crate::model::images::ResolvedImages::new(),
            )
            .expect("translate cancelled result"),
        )
        .expect("serialize provider message");
        assert!(encoded.contains("runtime_shutdown"));
        assert!(encoded.contains("did not start execution"));
    }

    /// Issue #202: a proven terminal timeout and an admitted unknown outcome
    /// project their typed status feedback through the wire unchanged — the
    /// adapter renders the canonical projection and manufactures no
    /// provider-specific prose of its own.
    #[test]
    fn issue202_chat_translation_consumes_typed_timeout_and_unknown_outcome() {
        for (status, required) in [
            (
                ToolExecutionStatus::TimedOut,
                "established terminal settlement",
            ),
            (
                ToolExecutionStatus::OutcomeUnknown {
                    detail: "transport closed after dispatch".to_owned(),
                },
                "could not establish its final external outcome",
            ),
        ] {
            let message = ToolMessageBlock {
                occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                    crate::runtime::identity::MessageId::new("assistant"),
                    crate::message::types::ContentBlockIndex::new(0),
                ),
                id: MessageId::new("tool-result-1"),
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-1"),
                result: ToolExecutionResult {
                    deliveries: Vec::new(),
                    status,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                }
                .into(),
            };
            let encoded = serde_json::to_value(
                translate_tool_message(
                    &message.tool_call_id,
                    &message.result,
                    &crate::model::images::ResolvedImages::new(),
                )
                .expect("translate status result"),
            )
            .expect("serialize provider message");
            let projection = message.result.model_facing_projection();
            let wire_parts: Vec<String> = encoded["content"]
                .as_array()
                .expect("text content array")
                .iter()
                .map(|part| part["text"].as_str().expect("text part").to_owned())
                .collect();
            assert_eq!(
                wire_parts.as_slice(),
                projection.parts(),
                "the wire content is the canonical projection, verbatim"
            );
            assert!(
                wire_parts[0].contains(required),
                "the typed status feedback reaches the wire: {}",
                wire_parts[0]
            );
        }
    }

    #[test]
    fn failed_tool_status_reaches_the_provider_as_correction_evidence() {
        let message = ToolMessageBlock {
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                crate::runtime::identity::MessageId::new("assistant"),
                crate::message::types::ContentBlockIndex::new(0),
            ),
            id: MessageId::new("tool-result-failed"),
            tool_call_id: ToolCallId::new("call-failed"),
            tool_id: ToolId::new("tool-web-search-exa"),
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Failed {
                    error: "input schema validation failed: query is required".to_owned(),
                },
                content: vec![
                    ToolResultContent::Text(TextBlock {
                        text: "existing tool content".to_owned(),
                    }),
                    ToolResultContent::Json {
                        value: json!({"ok": true}),
                    },
                ],
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
            }
            .into(),
        };
        let encoded = serde_json::to_value(
            translate_tool_message(
                &message.tool_call_id,
                &message.result,
                &crate::model::images::ResolvedImages::new(),
            )
            .expect("translate rejected tool result"),
        )
        .expect("serialize provider message");
        assert_eq!(encoded["tool_call_id"], "call-failed");
        let projection = message.result.model_facing_projection();
        let wire_parts: Vec<String> = encoded["content"]
            .as_array()
            .expect("text content array")
            .iter()
            .map(|part| part["text"].as_str().expect("text part").to_owned())
            .collect();
        assert_eq!(wire_parts.as_slice(), projection.parts());
        assert!(wire_parts[0].contains("input schema validation failed: query is required"));
    }
}

#[cfg(test)]
mod prefix_publication_tests {
    use super::*;
    use crate::model::adapter::cache::tests::{append, oracle, prepared, request};

    #[test]
    fn late_old_preparation_cannot_replace_current_prefix() {
        let mut current = request();
        current.invocation.protocol = crate::model::ModelProtocol::OpenAiChatCompletions;
        append(&mut current, 0, true, true);
        let old = current.clone();
        assert_eq!(prepared(&old), oracle(&old));
        let tools =
            crate::model::adapter::validation::validate_request(&old, old.protocol()).unwrap();
        // Pause the old request after it has taken its immutable cached prefix.
        let staged = stage_messages(&old, &tools.correlation).unwrap();
        append(&mut current, 1, true, true);
        let expected = oracle(&current);
        assert_eq!(prepared(&current), expected);
        // Complete the old preparation after the new one has published its
        // prefix and consumed the old call/result's relabeling work.
        finish_messages(&old, &tools.correlation, staged, None).unwrap();
        assert_eq!(prepared(&current), expected);
    }
}
