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
    CreateChatCompletionRequest, CreateChatCompletionRequestArgs, FunctionCall, FunctionObject,
    ReasoningEffort,
};
use futures_util::StreamExt;
use serde::Deserialize;

use crate::message::types::ContentBlockIndex;
use crate::message::types::{AgentContentBlock, MessageBlock, ToolMessageBlock};
use crate::model::adapter::block_index::BlockAllocator;
use crate::model::adapter::cancellation::ModelCancellation;
use crate::model::adapter::openai::client::build_client;
use crate::model::adapter::openai::config::OpenAiAdapterConfig;
use crate::model::adapter::openai::mapping::{
    map_chat_finish_reason, normalize_chat_usage, normalize_error, resolve_tool,
};
use crate::model::adapter::traits::{
    ModelAdapter, ModelEventStream, model_event_stream_of_failure,
};
use crate::model::adapter::validation::{ValidatedTools, validate_request};
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::event::ModelEvent;
use crate::model::types::{ModelProtocol, ModelRequest, ModelUsage};
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
        let (api_key, api_base, _responses_storage, http_client) = config.into_parts();
        Self {
            client: build_client(&api_key, &api_base, http_client),
        }
    }
}

impl ModelAdapter for OpenAiChatCompletionsAdapter {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }

    fn stream(&self, request: ModelRequest, cancellation: ModelCancellation) -> ModelEventStream {
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
    cancellation: &ModelCancellation,
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
/// the typed request and the normalizer until the provider stream opens.
#[allow(clippy::large_enum_variant)]
enum ChatPhase {
    Preparing {
        client: Client<OpenAIConfig>,
        request: CreateChatCompletionRequest,
        normalizer: ChatStreamNormalizer,
        cancellation: ModelCancellation,
    },
    Opening {
        client: Client<OpenAIConfig>,
        request: CreateChatCompletionRequest,
        normalizer: ChatStreamNormalizer,
        cancellation: ModelCancellation,
    },
    Streaming {
        stream: async_openai::types::stream::StreamResponse<serde_json::Value>,
        normalizer: ChatStreamNormalizer,
        cancellation: ModelCancellation,
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
}

#[derive(Debug, Deserialize)]
struct ChatChoiceWire {
    #[serde(default)]
    delta: ChatDeltaWire,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ChatDeltaWire {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ChatToolCallChunkWire>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCallChunkWire {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ChatFunctionChunkWire>,
}

#[derive(Debug, Deserialize, Default)]
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
}

impl ChatStreamNormalizer {
    fn new(tools: ValidatedTools) -> Self {
        Self {
            tools,
            blocks: BlockAllocator::new(),
            tool_calls: BTreeMap::new(),
            usage: None,
            finish_reason: None,
        }
    }

    /// Processes one raw chunk, returning the normalized events.
    fn push(&mut self, chunk: &serde_json::Value) -> Result<Vec<ModelEvent>, ModelError> {
        if let Some(error) = chunk.get("error") {
            return Err(provider_error(format!(
                "OpenAI stream error: {}",
                error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
            )));
        }
        let chunk: ChatChunkWire = serde_json::from_value(chunk.clone())
            .map_err(|e| provider_error(format!("malformed chat chunk: {e}")))?;
        let mut events = Vec::new();
        if let Some(usage) = chunk.usage {
            let usage = normalize_chat_usage(&usage);
            self.usage = Some(usage.clone());
            events.push(ModelEvent::UsageUpdate { usage });
        }
        if let Some(choice) = chunk.choices.first() {
            if let Some(text) = &choice.delta.content {
                if !text.is_empty() {
                    let block_index = self.blocks.allocate(ChatBlockKey::Text);
                    events.push(ModelEvent::TextDelta {
                        block_index,
                        text: text.clone(),
                    });
                }
            }
            if let Some(refusal) = &choice.delta.refusal {
                if !refusal.is_empty() {
                    let block_index = self.blocks.allocate(ChatBlockKey::Refusal);
                    events.push(ModelEvent::RefusalDelta {
                        block_index,
                        text: refusal.clone(),
                    });
                }
            }
            for tool_chunk in &choice.delta.tool_calls {
                self.push_tool_call_chunk(tool_chunk, &mut events)?;
            }
            if let Some(reason) = &choice.finish_reason {
                self.finish_reason = Some(map_chat_finish_reason(Some(reason)));
            }
        }
        Ok(events)
    }

    fn push_tool_call_chunk(
        &mut self,
        chunk: &ChatToolCallChunkWire,
        events: &mut Vec<ModelEvent>,
    ) -> Result<(), ModelError> {
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
            assembly.call_id = Some(ToolCallId::new(id.clone()));
        }
        if let Some(function) = &chunk.function {
            if let Some(name) = &function.name {
                assembly.name = Some(name.clone());
            }
            if let Some(arguments) = &function.arguments {
                assembly.arguments.push_str(arguments);
            }
        }
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
            events.push(ModelEvent::ToolCallStarted {
                block_index: assembly.block_index,
                call: start,
            });
        }
        if let Some(function) = &chunk.function {
            if let Some(arguments) = &function.arguments {
                if !arguments.is_empty() {
                    let call_id = assembly.call_id.clone().expect("call id known after start");
                    events.push(ModelEvent::ToolCallArgumentsDelta {
                        block_index: assembly.block_index,
                        call_id,
                        arguments_delta: arguments.clone(),
                    });
                }
            }
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

/// Translates a canonical request into a typed Chat Completions request.
fn translate_request(request: &ModelRequest) -> Result<CreateChatCompletionRequest, ModelError> {
    let messages = translate_messages(request)?;
    let mut builder = CreateChatCompletionRequestArgs::default();
    builder
        .model(request.model.clone())
        .messages(messages)
        .stream_options(ChatCompletionStreamOptions {
            include_usage: Some(true),
            include_obfuscation: None,
        });
    builder.max_completion_tokens(request.max_output_tokens);
    builder.reasoning_effort(match request.reasoning {
        crate::model::types::ReasoningEffort::Minimal => ReasoningEffort::Minimal,
        crate::model::types::ReasoningEffort::Low => ReasoningEffort::Low,
        crate::model::types::ReasoningEffort::Medium => ReasoningEffort::Medium,
        crate::model::types::ReasoningEffort::High => ReasoningEffort::High,
    });
    if !request.tools.is_empty() {
        builder.tools(translate_tools(&request.tools));
    }
    builder.build().map_err(|e| ModelError {
        kind: ModelErrorKind::InvalidRequest,
        message: format!("failed to build Chat Completions request: {e}"),
        retry_after_ms: None,
        provider_code: None,
    })
}

/// Translates the canonical message list into typed Chat Completions
/// messages, rejecting canonical content the protocol cannot represent
/// without changing its meaning.
fn translate_messages(
    request: &ModelRequest,
) -> Result<Vec<ChatCompletionRequestMessage>, ModelError> {
    let mut messages = Vec::new();
    for block in &request.messages {
        messages.push(match block {
            MessageBlock::System(system) => {
                let texts: Vec<String> = system.content.iter().map(|text| text.text.clone()).collect();
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
                ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                    content,
                    name: None,
                })
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
                ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                    content: ChatCompletionRequestUserMessageContent::Array(parts),
                    name: None,
                })
            }
            MessageBlock::Agent(agent) => {
                ChatCompletionRequestMessage::Assistant(translate_agent_message(agent)?)
            }
            MessageBlock::Tool(tool_message) => {
                ChatCompletionRequestMessage::Tool(translate_tool_message(tool_message)?)
            }
        });
    }
    if messages.is_empty() {
        return Err(ModelError {
            kind: ModelErrorKind::InvalidRequest,
            message: "a Chat Completions request requires at least one message".to_owned(),
            retry_after_ms: None,
            provider_code: None,
        });
    }
    Ok(messages)
}

/// Translates one canonical agent message into an assistant message,
/// rejecting previous reasoning (which Chat Completions cannot represent
/// without flattening it into text) and generated images.
fn translate_agent_message(
    agent: &crate::message::types::AgentMessageBlock,
) -> Result<async_openai::types::chat::ChatCompletionRequestAssistantMessage, ModelError> {
    let mut parts = Vec::new();
    let mut tool_calls = Vec::new();
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
            AgentContentBlock::Reasoning(_) => {
                return Err(unsupported(
                    "OpenAI Chat Completions cannot represent previous reasoning blocks; \
                     refusing to flatten reasoning into text",
                ));
            }
            AgentContentBlock::Image(_) => {
                return Err(unsupported(
                    "OpenAI Chat Completions cannot represent generated image references",
                ));
            }
        }
    }
    Ok(
        async_openai::types::chat::ChatCompletionRequestAssistantMessage {
            content: Some(ChatCompletionRequestAssistantMessageContent::Array(parts)),
            tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
            ..Default::default()
        },
    )
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
/// schema. Execution mode, replay policy, origin, and `ToolId` are runtime
/// semantics and never reach the provider.
fn translate_tools(tools: &[crate::tools::types::ToolDefinition]) -> Vec<ChatCompletionTools> {
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
