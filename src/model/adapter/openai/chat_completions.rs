//! `OpenAI` Chat Completions adapter.
//!
//! Implements the canonical [`ModelAdapter`] for `ModelProtocol::OpenAiChatCompletions`.
//! Requests are translated from canonical `MessageBlock` values into typed
//! SDK request values; the response stream is consumed through the SDK's
//! BYOT facility as raw JSON so that unknown finish reasons and future chunk
//! fields are tolerated instead of failing the whole stream.
//!
//! Chat Completions has no Responses-style continuation: the adapter rejects
//! any non-`None` canonical continuation state before executing.
//!
//! # Opaque request parameters
//!
//! Canonical translation still uses the typed SDK request builder. Before
//! sending, the typed request is serialized to a `serde_json::Value`,
//! required to be an object, checked against the runtime-owned protected
//! wire keys, and shallow-overlaid with the request's effective
//! `requestParams`. The resulting value is sent through the SDK's BYOT
//! streaming entry point, so there is exactly one HTTP implementation and no
//! invented `extra_body` nesting level.
//!
//! No request-level reasoning control is ever synthesized: whatever the
//! selected reasoning profile configured is exactly what appears on the
//! wire. Previous assistant reasoning is replayed through the model-declared
//! provider-specific `reasoning` or `reasoning_content` field, or omitted by
//! policy, rather than flattened into visible assistant text.

use std::collections::{BTreeMap, VecDeque};

use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::error::OpenAIError;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageContent, ChatCompletionRequestAssistantMessageContentPart,
    ChatCompletionRequestMessage, ChatCompletionRequestMessageContentPartRefusal,
    ChatCompletionRequestMessageContentPartText, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestSystemMessageContent, ChatCompletionRequestSystemMessageContentPart,
    ChatCompletionRequestToolMessage, ChatCompletionRequestToolMessageContent,
    ChatCompletionRequestToolMessageContentPart, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, ChatCompletionRequestUserMessageContentPart,
    ChatCompletionStreamOptions, ChatCompletionTool, ChatCompletionTools, CompletionUsage,
    CreateChatCompletionRequestArgs, FunctionCall, FunctionObject,
};
use futures_util::StreamExt;
use serde::Deserialize;

use crate::message::types::ContentBlockIndex;
use crate::message::types::{AgentContentBlock, MessageBlock, ToolMessageBlock};
use crate::model::adapter::block_index::BlockAllocator;
use crate::model::adapter::openai::client::build_client;
use crate::model::adapter::openai::config::OpenAiAdapterConfig;
use crate::model::adapter::openai::mapping::{
    is_context_window_message, map_chat_finish_reason, normalize_chat_usage, normalize_error,
    resolve_tool,
};
use crate::model::adapter::traits::{
    ModelAdapter, ModelEventStream, model_event_stream_of_failure,
};
use crate::model::adapter::validation::{ValidatedTools, validate_request};
use crate::model::catalog::{ChatReasoningReplay, ChatStreamUsage};
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::event::ModelEvent;
use crate::model::invocation::finalize_provider_request;
use crate::model::types::{ModelProtocol, ModelRequest, ModelUsage};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::ToolCallId;
use crate::tools::types::{ToolCall, ToolCallStart};

/// Adapter for the `OpenAI` Chat Completions protocol.
pub struct OpenAiChatCompletionsAdapter {
    client: Client<OpenAIConfig>,
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
            client: build_client(&api_key, &api_base, http_client),
        }
    }
}

impl ModelAdapter for OpenAiChatCompletionsAdapter {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }

    fn stream(&self, request: ModelRequest, cancellation: CancellationSignal) -> ModelEventStream {
        let validated = match validate_request(&request, self.protocol()) {
            Ok(validated) => validated,
            Err(error) => return model_event_stream_of_failure(error),
        };
        let translated = match translate_request(&request) {
            Ok(translated) => translated,
            Err(error) => return model_event_stream_of_failure(error),
        };
        let client = self.client.clone();
        Box::pin(futures_util::stream::unfold(
            ChatPhase::Preparing {
                client,
                request: translated,
                normalizer: ChatStreamNormalizer::new(validated),
                cancellation,
            },
            chat_phase_next,
        ))
    }
}

async fn chat_phase_next(phase: ChatPhase) -> Option<(ModelEvent, ChatPhase)> {
    match phase {
        ChatPhase::Preparing {
            client,
            request,
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
                ModelEvent::Started,
                ChatPhase::Opening {
                    client,
                    request,
                    normalizer,
                    cancellation,
                },
            ))
        }
        ChatPhase::Opening {
            client,
            request,
            mut normalizer,
            cancellation,
        } => {
            let api = client.chat();
            let outcome = tokio::select! {
                outcome = api.create_stream_byot(&request) => outcome,
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

/// Pulls provider events into `pending` until at least one event is ready or
/// the invocation is over.
async fn chat_pull(
    stream: &mut async_openai::types::stream::StreamResponse<serde_json::Value>,
    normalizer: &mut ChatStreamNormalizer,
    cancellation: &CancellationSignal,
    pending: &mut VecDeque<ModelEvent>,
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
            Some(Ok(chunk)) => match normalizer.push(&chunk) {
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
fn finish(normalizer: &mut ChatStreamNormalizer, pending: &mut VecDeque<ModelEvent>) {
    match normalizer.finish() {
        Ok(events) => pending.extend(events),
        Err(error) => pending.push_back(failed(error)),
    }
}

fn is_done_marker(error: &OpenAIError) -> bool {
    matches!(error, OpenAIError::JSONDeserialize(_, content) if content == "[DONE]")
}

fn is_terminal(event: &ModelEvent) -> bool {
    matches!(
        event,
        ModelEvent::Completed { .. } | ModelEvent::Failed { .. }
    )
}

fn failed(error: ModelError) -> ModelEvent {
    ModelEvent::Failed { error }
}

fn cancelled_error() -> ModelError {
    ModelError {
        kind: ModelErrorKind::Cancelled,
        message: "model invocation cancelled".to_owned(),
        retry_after_ms: None,
        provider_code: None,
    }
}

/// Phase machine driving one adapter invocation.
///
/// The `Preparing` variant is deliberately larger than the others: it owns
/// the final request JSON and the normalizer until the provider stream opens.
#[allow(clippy::large_enum_variant)]
enum ChatPhase {
    Preparing {
        client: Client<OpenAIConfig>,
        request: serde_json::Value,
        normalizer: ChatStreamNormalizer,
        cancellation: CancellationSignal,
    },
    Opening {
        client: Client<OpenAIConfig>,
        request: serde_json::Value,
        normalizer: ChatStreamNormalizer,
        cancellation: CancellationSignal,
    },
    Streaming {
        stream: async_openai::types::stream::StreamResponse<serde_json::Value>,
        normalizer: ChatStreamNormalizer,
        cancellation: CancellationSignal,
        pending: VecDeque<ModelEvent>,
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
    usage: Option<CompletionUsage>,
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
    tools: ValidatedTools,
    blocks: BlockAllocator<ChatBlockKey>,
    tool_calls: BTreeMap<u32, ToolAssembly>,
    usage: Option<ModelUsage>,
    finish_reason: Option<crate::model::finish::ModelFinishReason>,
    text_buffer: Option<String>,
    reasoning_buffer: Option<String>,
    refusal_buffer: Option<String>,
}

impl ChatStreamNormalizer {
    fn new(tools: ValidatedTools) -> Self {
        Self {
            tools,
            blocks: BlockAllocator::new(),
            tool_calls: BTreeMap::new(),
            usage: None,
            finish_reason: None,
            text_buffer: None,
            reasoning_buffer: None,
            refusal_buffer: None,
        }
    }

    /// Processes one raw chunk, returning the normalized events.
    fn push(&mut self, chunk: &serde_json::Value) -> Result<Vec<ModelEvent>, ModelError> {
        if let Some(error) = chunk.get("error") {
            return Err(chat_stream_error(error));
        }
        let chunk: ChatChunkWire = serde_json::from_value(chunk.clone())
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
            return Err(ModelError {
                kind: ModelErrorKind::ProviderError,
                message: base
                    .status_msg
                    .clone()
                    .unwrap_or_else(|| "provider reported an unsuccessful base_resp".to_owned()),
                retry_after_ms: None,
                provider_code: base.status_code.map(|code| code.to_string()),
            });
        }
        if chunk.choices.len() > 1 {
            return Err(unsupported(
                "multiple Chat Completions choices cannot be represented as one canonical agent turn",
            ));
        }
        let mut events = Vec::new();
        if let Some(usage) = chunk.usage {
            let usage = normalize_chat_usage(&usage);
            self.usage = Some(usage.clone());
            events.push(ModelEvent::UsageUpdate { usage });
        }
        if let Some(choice) = chunk.choices.first() {
            let choice_index = choice.index.ok_or_else(|| {
                provider_error("Chat Completions choice lacks an index".to_owned())
            })?;
            if choice_index != 0 {
                return Err(unsupported(format!(
                    "Chat Completions choice index {choice_index} cannot be represented as the single canonical agent turn"
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
                if matches!(
                    reason.as_str(),
                    "error" | "network_error" | "insufficient_system_resource" | "abort"
                ) {
                    return Err(ModelError {
                        kind: ModelErrorKind::ProviderError,
                        message: format!(
                            "provider terminated Chat Completions generation with {reason:?}"
                        ),
                        retry_after_ms: None,
                        provider_code: Some(reason.clone()),
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
            let tool_id = resolve_tool(&self.tools, name)?;
            let start = ToolCallStart {
                id: call_id.clone(),
                tool_id,
                name: name.clone(),
            };
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
        for assembly in assemblies {
            let Some(call_id) = assembly.call_id else {
                return Err(provider_error(
                    "provider tool call ended without an invocation id".to_owned(),
                ));
            };
            let Some(name) = assembly.name else {
                return Err(provider_error(format!(
                    "tool call {call_id} ended without a function name"
                )));
            };
            let tool_id = resolve_tool(&self.tools, &name)?;
            let arguments = serde_json::from_str(&assembly.arguments).map_err(|e| {
                provider_error(format!(
                    "malformed complete tool arguments for {name:?} ({call_id}): {e}"
                ))
            })?;
            events.push(ModelEvent::ToolCallCompleted {
                block_index: assembly.block_index,
                call: ToolCall {
                    id: call_id,
                    tool_id,
                    name,
                    arguments,
                },
            });
        }
        let finish_reason = self.finish_reason.take().ok_or_else(|| {
            provider_error("provider stream ended without a finish reason".to_owned())
        })?;
        events.push(ModelEvent::Completed {
            finish_reason,
            usage: self.usage.take(),
        });
        Ok(events)
    }
}

fn provider_error(message: String) -> ModelError {
    ModelError {
        kind: ModelErrorKind::ProviderError,
        message,
        retry_after_ms: None,
        provider_code: None,
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
    let kind = match error_type {
        Some("authentication" | "authentication_error" | "permission_error") => {
            ModelErrorKind::Authentication
        }
        Some("rate_limit_exceeded" | "rate_limit_error") => ModelErrorKind::RateLimit,
        Some(
            "context_length_exceeded"
            | "max_tokens_exceeded"
            | "token_limit_exceeded"
            | "string_too_long",
        ) => ModelErrorKind::ContextWindowExceeded,
        Some("invalid_request" | "invalid_prompt" | "invalid_request_error") => {
            ModelErrorKind::InvalidRequest
        }
        Some("timeout") => ModelErrorKind::Timeout,
        _ if is_context_window_message(message) => ModelErrorKind::ContextWindowExceeded,
        _ => match numeric_code {
            Some(400) => ModelErrorKind::InvalidRequest,
            Some(401 | 403) => ModelErrorKind::Authentication,
            Some(408) => ModelErrorKind::Timeout,
            Some(429) => ModelErrorKind::RateLimit,
            _ => ModelErrorKind::ProviderError,
        },
    };
    let provider_code = metadata
        .and_then(|value| value.get("provider_code"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| error_type.map(str::to_owned))
        .or_else(|| {
            error.get("code").and_then(|code| match code {
                serde_json::Value::String(code) => Some(code.clone()),
                serde_json::Value::Number(code) => Some(code.to_string()),
                _ => None,
            })
        });
    ModelError {
        kind,
        message: format!("OpenAI-compatible stream error: {message}"),
        retry_after_ms: None,
        provider_code,
    }
}

/// Translates a canonical request into the final Chat Completions request
/// JSON.
///
/// Canonical translation uses the typed SDK builder; the runtime-owned
/// structural fields are then written explicitly onto the serialized object
/// (including the compat-selected max-token spelling), and the effective
/// opaque request parameters are shallow-overlaid last under the
/// protected-key contract.
fn translate_request(request: &ModelRequest) -> Result<serde_json::Value, ModelError> {
    let messages = translate_messages(request)?;
    let assistant_reasoning: Vec<Option<String>> = messages
        .iter()
        .map(|message| message.reasoning.clone())
        .collect();
    let mut builder = CreateChatCompletionRequestArgs::default();
    builder.model(request.model().to_owned()).messages(
        messages
            .into_iter()
            .map(|message| message.message)
            .collect::<Vec<_>>(),
    );
    if request.invocation.compat.chat_stream_usage == ChatStreamUsage::Supported {
        builder.stream_options(ChatCompletionStreamOptions {
            include_usage: Some(true),
            include_obfuscation: None,
        });
    }
    // Tool definitions are only sent to a model whose effective capabilities
    // include tool calls; the runtime never compiles tools for a text-only
    // model, and this is the adapter-side guard for the same invariant.
    if !request.tools.is_empty() {
        if !request.invocation.capabilities.tool_calls {
            return Err(unsupported(
                "the effective model capabilities do not include tool calls; \
                 tool definitions are never sent to a text-only model",
            ));
        }
        builder.tools(translate_tools(&request.tools));
    }
    let typed = builder.build().map_err(|e| ModelError {
        kind: ModelErrorKind::InvalidRequest,
        message: format!("failed to build Chat Completions request: {e}"),
        retry_after_ms: None,
        provider_code: None,
    })?;
    let mut value = serde_json::to_value(&typed).map_err(|e| ModelError {
        kind: ModelErrorKind::InvalidRequest,
        message: format!("failed to serialize the Chat Completions request: {e}"),
        retry_after_ms: None,
        provider_code: None,
    })?;
    let wire_messages = value
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| ModelError {
            kind: ModelErrorKind::InvalidRequest,
            message: "serialized Chat Completions request has no message array".to_owned(),
            retry_after_ms: None,
            provider_code: None,
        })?;
    if wire_messages.len() != assistant_reasoning.len() {
        return Err(ModelError {
            kind: ModelErrorKind::InvalidRequest,
            message: "serialized Chat Completions message count changed unexpectedly".to_owned(),
            retry_after_ms: None,
            provider_code: None,
        });
    }
    for (wire_message, reasoning) in wire_messages.iter_mut().zip(assistant_reasoning) {
        let Some(reasoning) = reasoning else {
            continue;
        };
        let object = wire_message.as_object_mut().ok_or_else(|| ModelError {
            kind: ModelErrorKind::InvalidRequest,
            message: "serialized Chat Completions message is not an object".to_owned(),
            retry_after_ms: None,
            provider_code: None,
        })?;
        if let Some(field) = request
            .invocation
            .compat
            .chat_reasoning_replay
            .and_then(ChatReasoningReplay::wire_name)
        {
            object.insert(field.to_owned(), reasoning.into());
        }
    }
    // Runtime-owned structural fields: streaming is always on, and exactly
    // one max-token spelling is written, chosen by the model's compat
    // metadata. Both spellings are protected, so no request parameter can add
    // a second contradictory maximum.
    value["stream"] = serde_json::Value::Bool(true);
    value[request.invocation.compat.chat_max_tokens_field.wire_name()] =
        request.max_output_tokens().into();
    finalize_provider_request(
        value,
        request.request_params(),
        ModelProtocol::OpenAiChatCompletions,
    )
}

/// Translates the canonical message list into typed Chat Completions
/// messages, rejecting canonical content the protocol cannot represent
/// without changing its meaning.
struct TranslatedChatMessage {
    message: ChatCompletionRequestMessage,
    reasoning: Option<String>,
}

fn translate_messages(request: &ModelRequest) -> Result<Vec<TranslatedChatMessage>, ModelError> {
    let mut system_messages = Vec::new();
    let mut transcript_messages = Vec::new();
    let reasoning_replay = request.invocation.compat.chat_reasoning_replay;
    for block in &request.messages {
        let (translated, reasoning) = match block {
            MessageBlock::System(system) => {
                let texts: Vec<String> = system
                    .content
                    .iter()
                    .map(|text| text.text.clone())
                    .collect();
                let content = match texts.len() {
                    1 => ChatCompletionRequestSystemMessageContent::Text(
                        texts.into_iter().next().expect("exactly one text"),
                    ),
                    _ => ChatCompletionRequestSystemMessageContent::Array(
                        texts
                            .into_iter()
                            .map(|text| {
                                ChatCompletionRequestSystemMessageContentPart::Text(
                                    ChatCompletionRequestMessageContentPartText { text },
                                )
                            })
                            .collect(),
                    ),
                };
                (
                    ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                        content,
                        name: None,
                    }),
                    None,
                )
            }
            MessageBlock::User(user) => {
                let mut parts = Vec::new();
                for content in &user.content {
                    match content {
                        crate::message::types::UserContentBlock::Text(text) => {
                            parts.push(ChatCompletionRequestUserMessageContentPart::Text(
                                ChatCompletionRequestMessageContentPartText {
                                    text: text.text.clone(),
                                },
                            ));
                        }
                        crate::message::types::UserContentBlock::Image(_)
                        | crate::message::types::UserContentBlock::File(_) => {
                            return Err(unsupported(
                                "OpenAI Chat Completions cannot represent canonical image/file references without artifact resolution",
                            ));
                        }
                    }
                }
                // The target fresh inbound user message receives one final
                // rendered Agent Status text part. The status is never a
                // separate canonical message and never appended to other
                // user messages of the batch.
                if let Some(status) = request
                    .agent_status
                    .as_ref()
                    .filter(|status| status.target_message_id == user.id)
                {
                    parts.push(ChatCompletionRequestUserMessageContentPart::Text(
                        ChatCompletionRequestMessageContentPartText {
                            text: status.rendered.clone(),
                        },
                    ));
                }
                (
                    ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                        content: ChatCompletionRequestUserMessageContent::Array(parts),
                        name: None,
                    }),
                    None,
                )
            }
            MessageBlock::Agent(agent) => {
                let (message, reasoning) = translate_agent_message(agent, reasoning_replay)?;
                (ChatCompletionRequestMessage::Assistant(message), reasoning)
            }
            MessageBlock::Tool(tool_message) => (
                ChatCompletionRequestMessage::Tool(translate_tool_message(tool_message)?),
                None,
            ),
        };
        let translated = TranslatedChatMessage {
            message: translated,
            reasoning,
        };
        if matches!(&translated.message, ChatCompletionRequestMessage::System(_)) {
            system_messages.push(translated);
        } else {
            transcript_messages.push(translated);
        }
    }
    // The Skill catalog is trusted system context: it is translated through
    // the provider's system-level message mechanism together with the
    // canonical trusted system context, as one deterministic system message
    // after the canonical system messages. It is never attached to a user
    // message.
    append_skill_catalog(request, &mut system_messages);
    let mut messages = system_messages;
    messages.extend(transcript_messages);
    if messages.is_empty() {
        return Err(invalid_request(
            "a Chat Completions request requires at least one message",
        ));
    }
    Ok(messages)
}

fn append_skill_catalog(request: &ModelRequest, messages: &mut Vec<TranslatedChatMessage>) {
    let Some(catalog) = &request.skill_catalog else {
        return;
    };
    messages.push(TranslatedChatMessage {
        message: ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
            content: ChatCompletionRequestSystemMessageContent::Text(catalog.rendered.clone()),
            name: None,
        }),
        reasoning: None,
    });
}

/// Translates one canonical agent message into an assistant message.
///
/// A Chat Completions dialect represents one previous reasoning block in its
/// provider-specific extension field. The typed `OpenAI` SDK message and that
/// extension value stay separate until the final BYOT JSON is assembled.
/// Shapes that cannot be represented losslessly remain unsupported; the
/// explicit omit policy skips canonical reasoning before this validation.
fn translate_agent_message(
    agent: &crate::message::types::AgentMessageBlock,
    reasoning_replay: Option<ChatReasoningReplay>,
) -> Result<
    (
        async_openai::types::chat::ChatCompletionRequestAssistantMessage,
        Option<String>,
    ),
    ModelError,
> {
    let mut parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut reasoning = None;
    for content in &agent.content {
        match content {
            AgentContentBlock::Text(text) => {
                parts.push(ChatCompletionRequestAssistantMessageContentPart::Text(
                    ChatCompletionRequestMessageContentPartText {
                        text: text.text.clone(),
                    },
                ));
            }
            AgentContentBlock::Refusal(refusal) => {
                parts.push(ChatCompletionRequestAssistantMessageContentPart::Refusal(
                    ChatCompletionRequestMessageContentPartRefusal {
                        refusal: refusal.text.clone(),
                    },
                ));
            }
            AgentContentBlock::ToolCall(call) => {
                let arguments = serde_json::to_string(&call.arguments).map_err(|e| {
                    unsupported(format!(
                        "tool call arguments are not JSON-serializable: {e}"
                    ))
                })?;
                tool_calls.push(ChatCompletionMessageToolCalls::Function(
                    ChatCompletionMessageToolCall {
                        id: call.id.as_str().to_owned(),
                        function: FunctionCall {
                            name: call.name.clone(),
                            arguments,
                        },
                    },
                ));
            }
            AgentContentBlock::Reasoning(block) => {
                // Omit is a translation policy, not a serialized-JSON
                // cleanup step: this branch deliberately does nothing for
                // that policy, without inspecting text or provider state.
                if reasoning_replay != Some(ChatReasoningReplay::Omit) {
                    if reasoning_replay.is_none() {
                        return Err(unsupported(
                            "OpenAI Chat Completions requires an explicit chatReasoningReplay compat value to replay historical reasoning",
                        ));
                    }
                    let text = block.text.as_ref().ok_or_else(|| {
                        unsupported(
                            "OpenAI Chat Completions cannot replay a previous reasoning block \
                             whose text was not exposed by the provider",
                        )
                    })?;
                    if reasoning.replace(text.clone()).is_some() {
                        return Err(unsupported(
                            "OpenAI Chat Completions cannot losslessly represent multiple \
                             reasoning blocks in one assistant message",
                        ));
                    }
                }
            }
            AgentContentBlock::Image(_) => {
                return Err(unsupported(
                    "OpenAI Chat Completions cannot represent generated image references",
                ));
            }
        }
    }
    Ok((
        async_openai::types::chat::ChatCompletionRequestAssistantMessage {
            content: Some(ChatCompletionRequestAssistantMessageContent::Array(parts)),
            tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
            ..Default::default()
        },
        reasoning,
    ))
}

/// Translates a canonical tool result into a provider tool message. Only the
/// model-facing content is represented; runtime-only semantics stay behind.
fn translate_tool_message(
    message: &ToolMessageBlock,
) -> Result<ChatCompletionRequestToolMessage, ModelError> {
    let mut parts = Vec::new();
    for content in &message.result.content {
        match content {
            crate::tools::types::ToolResultContent::Text(text) => {
                parts.push(ChatCompletionRequestToolMessageContentPart::Text(
                    ChatCompletionRequestMessageContentPartText {
                        text: text.text.clone(),
                    },
                ));
            }
            crate::tools::types::ToolResultContent::Json { value } => {
                let text = serde_json::to_string(value).map_err(|e| {
                    unsupported(format!("tool JSON result is not serializable: {e}"))
                })?;
                parts.push(ChatCompletionRequestToolMessageContentPart::Text(
                    ChatCompletionRequestMessageContentPartText { text },
                ));
            }
            crate::tools::types::ToolResultContent::File(_)
            | crate::tools::types::ToolResultContent::Image(_) => {
                return Err(unsupported(
                    "OpenAI Chat Completions cannot represent file/image tool results",
                ));
            }
        }
    }
    Ok(ChatCompletionRequestToolMessage {
        content: ChatCompletionRequestToolMessageContent::Array(parts),
        tool_call_id: message.tool_call_id.as_str().to_owned(),
    })
}

/// Only model-facing tool fields are sent: name, description, and the input
/// schema. Execution policy, replay policy, origin, and runtime semantics
/// never reach the provider; the compiled model-facing definition is
/// translated verbatim.
fn translate_tools(tools: &[crate::tools::types::ModelToolDefinition]) -> Vec<ChatCompletionTools> {
    tools
        .iter()
        .map(|tool| {
            ChatCompletionTools::Function(ChatCompletionTool {
                function: FunctionObject {
                    name: tool.name.clone(),
                    description: Some(tool.description.clone()),
                    parameters: Some(tool.input_schema.clone()),
                    strict: None,
                },
            })
        })
        .collect()
}

fn unsupported(message: impl Into<String>) -> ModelError {
    let message = message.into();
    ModelError {
        kind: ModelErrorKind::Unsupported,
        message,
        retry_after_ms: None,
        provider_code: None,
    }
}

fn invalid_request(message: &str) -> ModelError {
    ModelError {
        kind: ModelErrorKind::InvalidRequest,
        message: message.to_owned(),
        retry_after_ms: None,
        provider_code: None,
    }
}
