//! `OpenAI` Responses adapter.
//!
//! Implements the canonical [`ModelAdapter`] for `ModelProtocol::OpenAiResponses`,
//! including Stored (provider-side storage, `previous_response_id`) and
//! Stateless (`store: false`, preserved output items) continuation.
//!
//! The request and the response stream are handled through the SDK's BYOT
//! facility as raw JSON: preserved stateless items must survive losslessly
//! (including opaque encrypted reasoning content), so they are never
//! round-tripped through typed SDK structs that would drop unknown fields.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::error::OpenAIError;
use futures_util::StreamExt;

use crate::message::types::ContentBlockIndex;
use crate::message::types::{AgentContentBlock, MessageBlock};
use crate::model::adapter::block_index::BlockAllocator;
use crate::model::adapter::openai::client::build_client;
use crate::model::adapter::openai::config::OpenAiAdapterConfig;
use crate::model::adapter::openai::mapping::{normalize_error, resolve_tool};
use crate::model::adapter::traits::{
    ModelAdapter, ModelEventStream, model_event_stream_of_failure,
};
use crate::model::adapter::validation::{ValidatedTools, validate_request};
use crate::model::catalog::ResponsesStorageMode;
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::invocation::finalize_provider_request;
use crate::model::types::{ModelProtocol, ModelRequest, ModelUsage, UsageDetails};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::continuation::{OpenAiResponsesContinuation, ProviderContinuationState};
use crate::runtime::identity::ToolCallId;
use crate::tools::types::{ToolCall, ToolCallStart};

/// Adapter for the `OpenAI` Responses protocol.
///
/// The provider storage/continuation mode is **per model**: it arrives with
/// each request's invocation configuration
/// ([`ModelCompat`](crate::model::catalog::ModelCompat)), so one adapter
/// serves every Responses model of its provider.
pub struct OpenAiResponsesAdapter {
    client: Client<OpenAIConfig>,
}

impl std::fmt::Debug for OpenAiResponsesAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiResponsesAdapter")
            .finish_non_exhaustive()
    }
}

impl OpenAiResponsesAdapter {
    /// Creates the adapter from rustX-owned configuration.
    #[must_use]
    pub fn new(config: OpenAiAdapterConfig) -> Self {
        let (api_key, api_base, http_client) = config.into_parts();
        Self {
            client: build_client(&api_key, &api_base, http_client),
        }
    }
}

impl ModelAdapter for OpenAiResponsesAdapter {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiResponses
    }

    fn stream(&self, request: ModelRequest, cancellation: CancellationSignal) -> ModelEventStream {
        let validated = match validate_request(&request, self.protocol()) {
            Ok(validated) => validated,
            Err(error) => return model_event_stream_of_failure(error),
        };
        let storage_mode = request.invocation.compat.responses_storage;
        let translated = match translate_request(&request, &validated, storage_mode) {
            Ok(translated) => translated,
            Err(error) => return model_event_stream_of_failure(error),
        };
        let client = self.client.clone();
        let normalizer = ResponsesNormalizer::new(validated, storage_mode);
        Box::pin(futures_util::stream::unfold(
            ResponsesPhase::Preparing {
                client,
                request: translated,
                normalizer,
                cancellation,
            },
            responses_phase_next,
        ))
    }
}

async fn responses_phase_next(phase: ResponsesPhase) -> Option<(ModelEvent, ResponsesPhase)> {
    match phase {
        ResponsesPhase::Preparing {
            client,
            request,
            normalizer,
            cancellation,
        } => {
            if cancellation.is_cancelled() {
                return Some((
                    ModelEvent::Failed {
                        error: cancelled_error(),
                    },
                    ResponsesPhase::Finished,
                ));
            }
            // The provider request attempt begins: Started is emitted before
            // the network-opening await so the lifecycle stays consistent
            // when cancellation interrupts that await.
            Some((
                ModelEvent::Started,
                ResponsesPhase::Opening {
                    client,
                    request,
                    normalizer,
                    cancellation,
                },
            ))
        }
        ResponsesPhase::Opening {
            client,
            request,
            mut normalizer,
            cancellation,
        } => {
            let api = client.responses();
            let outcome = tokio::select! {
                outcome = api.create_stream_byot(&request) => outcome,
                () = cancellation.cancelled() => {
                    return Some((
                        ModelEvent::Failed {
                            error: cancelled_error(),
                        },
                        ResponsesPhase::Finished,
                    ));
                }
            };
            match outcome {
                Ok(mut stream) => {
                    let mut pending = VecDeque::new();
                    responses_pull(&mut stream, &mut normalizer, &cancellation, &mut pending).await;
                    let event = pending.pop_front().expect("pending is non-empty here");
                    let next_phase = if is_terminal(&event) {
                        ResponsesPhase::Finished
                    } else {
                        ResponsesPhase::Streaming {
                            stream,
                            normalizer,
                            cancellation,
                            pending,
                        }
                    };
                    Some((event, next_phase))
                }
                Err(error) => Some((
                    ModelEvent::Failed {
                        error: normalize_error(error),
                    },
                    ResponsesPhase::Finished,
                )),
            }
        }
        ResponsesPhase::Streaming {
            mut stream,
            mut normalizer,
            cancellation,
            mut pending,
        } => {
            responses_pull(&mut stream, &mut normalizer, &cancellation, &mut pending).await;
            let event = pending.pop_front().expect("pending is non-empty here");
            let next_phase = if is_terminal(&event) {
                ResponsesPhase::Finished
            } else {
                ResponsesPhase::Streaming {
                    stream,
                    normalizer,
                    cancellation,
                    pending,
                }
            };
            Some((event, next_phase))
        }
        ResponsesPhase::Finished => None,
    }
}

/// Pulls provider events into `pending` until at least one event is ready or
/// the invocation is over.
async fn responses_pull(
    stream: &mut async_openai::types::stream::StreamResponse<serde_json::Value>,
    normalizer: &mut ResponsesNormalizer,
    cancellation: &CancellationSignal,
    pending: &mut VecDeque<ModelEvent>,
) {
    while pending.is_empty() {
        let item = tokio::select! {
            item = stream.next() => item,
            () = cancellation.cancelled() => {
                pending.push_back(ModelEvent::Failed {
                    error: cancelled_error(),
                });
                break;
            }
        };
        match item {
            Some(Ok(event)) => match normalizer.push(&event) {
                Ok(events) => pending.extend(events),
                Err(error) => pending.push_back(ModelEvent::Failed { error }),
            },
            Some(Err(error)) => {
                if is_done_marker(&error) {
                    finish_responses(normalizer, pending);
                } else {
                    pending.push_back(ModelEvent::Failed {
                        error: normalize_error(error),
                    });
                }
                break;
            }
            None => {
                finish_responses(normalizer, pending);
                break;
            }
        }
    }
}

fn finish_responses(normalizer: &mut ResponsesNormalizer, pending: &mut VecDeque<ModelEvent>) {
    match normalizer.finish() {
        Ok(events) => pending.extend(events),
        Err(error) => pending.push_back(ModelEvent::Failed { error }),
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

fn cancelled_error() -> ModelError {
    ModelError {
        kind: ModelErrorKind::Cancelled,
        message: "model invocation cancelled".to_owned(),
        retry_after_ms: None,
        provider_code: None,
    }
}

enum ResponsesPhase {
    Preparing {
        client: Client<OpenAIConfig>,
        request: serde_json::Value,
        normalizer: ResponsesNormalizer,
        cancellation: CancellationSignal,
    },
    Opening {
        client: Client<OpenAIConfig>,
        request: serde_json::Value,
        normalizer: ResponsesNormalizer,
        cancellation: CancellationSignal,
    },
    Streaming {
        stream: async_openai::types::stream::StreamResponse<serde_json::Value>,
        normalizer: ResponsesNormalizer,
        cancellation: CancellationSignal,
        pending: VecDeque<ModelEvent>,
    },
    Finished,
}

/// Adapter-local canonical block keys for Responses output coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ResponsesBlockKey {
    /// A message content part identified by output item and part index.
    ItemPart(u32, u32),
    /// A whole output item (reasoning or function call) by output index.
    Item(u32),
}

/// Per-function-call assembly state keyed by the provider output index.
#[derive(Debug)]
struct ToolAssembly {
    block_index: ContentBlockIndex,
    call_id: Option<ToolCallId>,
    name: Option<String>,
    arguments: String,
    started: bool,
}

/// Normalizes Responses stream events into canonical events.
#[derive(Debug)]
struct ResponsesNormalizer {
    tools: ValidatedTools,
    storage_mode: ResponsesStorageMode,
    blocks: BlockAllocator<ResponsesBlockKey>,
    tool_calls: BTreeMap<u32, ToolAssembly>,
    usage: Option<ModelUsage>,
    reasoning_emitted: BTreeSet<u32>,
    terminal_emitted: bool,
    response_id: Option<String>,
}

impl ResponsesNormalizer {
    fn new(tools: ValidatedTools, storage_mode: ResponsesStorageMode) -> Self {
        Self {
            tools,
            storage_mode,
            blocks: BlockAllocator::new(),
            tool_calls: BTreeMap::new(),
            usage: None,
            reasoning_emitted: BTreeSet::new(),
            terminal_emitted: false,
            response_id: None,
        }
    }

    /// Processes one raw stream event, returning the normalized events.
    fn push(&mut self, event: &serde_json::Value) -> Result<Vec<ModelEvent>, ModelError> {
        let event_type = str_field(event, "type").unwrap_or("");
        match event_type {
            "response.created"
            | "response.in_progress"
            | "response.queued"
            | "response.content_part.added"
            | "response.content_part.done"
            | "response.output_text.done"
            | "response.refusal.done"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.done"
            | "response.function_call_arguments.done" => Ok(Vec::new()),
            "response.output_item.added" => self.push_output_item_added(event),
            "response.output_item.done" => self.push_output_item_done(event),
            "response.output_text.delta" => {
                let block_index = self.blocks.allocate(ResponsesBlockKey::ItemPart(
                    u32_field(event, "output_index")?,
                    u32_field(event, "content_index")?,
                ));
                Ok(vec![ModelEvent::TextDelta {
                    block_index,
                    text: str_field(event, "delta").unwrap_or_default().to_owned(),
                }])
            }
            "response.refusal.delta" => {
                let block_index = self.blocks.allocate(ResponsesBlockKey::ItemPart(
                    u32_field(event, "output_index")?,
                    u32_field(event, "content_index")?,
                ));
                Ok(vec![ModelEvent::RefusalDelta {
                    block_index,
                    text: str_field(event, "delta").unwrap_or_default().to_owned(),
                }])
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let output_index = u32_field(event, "output_index")?;
                let block_index = self.blocks.allocate(ResponsesBlockKey::Item(output_index));
                self.reasoning_emitted.insert(output_index);
                Ok(vec![ModelEvent::ReasoningDelta {
                    block_index,
                    text: str_field(event, "delta").unwrap_or_default().to_owned(),
                }])
            }
            "response.function_call_arguments.delta" => {
                let output_index = u32_field(event, "output_index")?;
                let assembly = self.tool_assembly(output_index);
                if !assembly.started {
                    // Identity is not yet known; fragments stay buffered.
                    if let Some(function) = event.get("delta").and_then(serde_json::Value::as_str) {
                        assembly.arguments.push_str(function);
                    }
                    return Ok(Vec::new());
                }
                let call_id = assembly.call_id.clone().expect("started implies known id");
                let mut events = Vec::new();
                if let Some(delta) = event.get("delta").and_then(serde_json::Value::as_str) {
                    assembly.arguments.push_str(delta);
                    if !delta.is_empty() {
                        events.push(ModelEvent::ToolCallArgumentsDelta {
                            block_index: assembly.block_index,
                            call_id,
                            arguments_delta: delta.to_owned(),
                        });
                    }
                }
                Ok(events)
            }
            "response.completed" | "response.incomplete" => {
                self.finish_terminal(event, event_type == "response.incomplete")
            }
            "response.failed" => Ok(self.push_response_failed(event)),
            "error" => Err(provider_error(
                event
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(|| "OpenAI stream error".to_owned(), str::to_owned),
            )),
            _ => {
                // Unknown top-level events carry no known output semantics
                // and must not crash the parser.
                Ok(Vec::new())
            }
        }
    }

    fn tool_assembly(&mut self, output_index: u32) -> &mut ToolAssembly {
        self.tool_calls.entry(output_index).or_insert_with(|| {
            let block_index = self.blocks.allocate(ResponsesBlockKey::Item(output_index));
            ToolAssembly {
                block_index,
                call_id: None,
                name: None,
                arguments: String::new(),
                started: false,
            }
        })
    }

    fn push_output_item_added(
        &mut self,
        event: &serde_json::Value,
    ) -> Result<Vec<ModelEvent>, ModelError> {
        let Some(item) = event.get("item") else {
            return Ok(Vec::new());
        };
        let item_type = str_field(item, "type").unwrap_or("");
        match item_type {
            "message" | "reasoning" | "function_call_output" => Ok(Vec::new()),
            "function_call" => {
                let output_index = u32_field(event, "output_index")?;
                let block_index = self.blocks.allocate(ResponsesBlockKey::Item(output_index));
                let call_id = item
                    .get("call_id")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| item.get("id").and_then(serde_json::Value::as_str));
                let Some(call_id) = call_id.filter(|id| !id.is_empty()) else {
                    return Err(provider_error(
                        "provider function call lacks a stable invocation id".to_owned(),
                    ));
                };
                let Some(name) = str_field(item, "name") else {
                    return Err(provider_error(
                        "provider function call lacks a function name".to_owned(),
                    ));
                };
                let tool_id = resolve_tool(&self.tools, name)?;
                let call = ToolCallStart {
                    id: ToolCallId::new(call_id),
                    tool_id,
                    name: name.to_owned(),
                };
                self.tool_calls.insert(
                    output_index,
                    ToolAssembly {
                        block_index,
                        call_id: Some(call.id.clone()),
                        name: Some(call.name.clone()),
                        arguments: String::new(),
                        started: true,
                    },
                );
                Ok(vec![ModelEvent::ToolCallStarted { block_index, call }])
            }
            other => Err(unsupported(format!(
                "provider-hosted or unsupported output item type {other:?}; \
                 refusing to reinterpret it as a rustX tool call"
            ))),
        }
    }

    fn push_output_item_done(
        &mut self,
        event: &serde_json::Value,
    ) -> Result<Vec<ModelEvent>, ModelError> {
        let Some(item) = event.get("item") else {
            return Ok(Vec::new());
        };
        let item_type = str_field(item, "type").unwrap_or("");
        match item_type {
            "function_call" => self.complete_function_call(event, item),
            "reasoning" => {
                let output_index = u32_field(event, "output_index")?;
                // Done events finalize adapter state; they never replay text
                // already emitted by delta events. When no delta was emitted
                // for this item at all, the done summary is the only source
                // of visible reasoning and is emitted once.
                if self.reasoning_emitted.contains(&output_index) {
                    return Ok(Vec::new());
                }
                let summary = item
                    .get("summary")
                    .and_then(serde_json::Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| {
                                part.get("text")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_owned)
                            })
                            .collect::<Vec<String>>()
                            .concat()
                    })
                    .unwrap_or_default();
                if summary.is_empty() {
                    return Ok(Vec::new());
                }
                let block_index = self.blocks.allocate(ResponsesBlockKey::Item(output_index));
                self.reasoning_emitted.insert(output_index);
                Ok(vec![ModelEvent::ReasoningDelta {
                    block_index,
                    text: summary,
                }])
            }
            "message"
            | "function_call_output"
            | "file_search_call"
            | "web_search_call"
            | "computer_call"
            | "computer_call_output"
            | "code_interpreter_call"
            | "mcp_call"
            | "custom_tool_call"
            | "image_generation_call" => Ok(Vec::new()),
            _other => Ok(Vec::new()),
        }
    }

    fn complete_function_call(
        &mut self,
        event: &serde_json::Value,
        item: &serde_json::Value,
    ) -> Result<Vec<ModelEvent>, ModelError> {
        let output_index = u32_field(event, "output_index")?;
        let Some(assembly) = self.tool_calls.get_mut(&output_index) else {
            return Err(provider_error(
                "function call completed without a matching start".to_owned(),
            ));
        };
        let call_id = assembly
            .call_id
            .clone()
            .ok_or_else(|| provider_error("function call lacks an invocation id".to_owned()))?;
        let name = assembly
            .name
            .clone()
            .ok_or_else(|| provider_error("function call lacks a name".to_owned()))?;
        let tool_id = resolve_tool(&self.tools, &name)?;
        let arguments = item
            .get("arguments")
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| assembly.arguments.clone(), str::to_owned);
        let arguments = serde_json::from_str(&arguments).map_err(|e| {
            provider_error(format!(
                "malformed complete tool arguments for {name:?} ({call_id}): {e}"
            ))
        })?;
        Ok(vec![ModelEvent::ToolCallCompleted {
            block_index: assembly.block_index,
            call: ToolCall {
                id: call_id,
                tool_id,
                name,
                arguments,
            },
        }])
    }

    /// Handles `response.completed` / `response.incomplete`.
    fn finish_terminal(
        &mut self,
        event: &serde_json::Value,
        incomplete: bool,
    ) -> Result<Vec<ModelEvent>, ModelError> {
        let Some(response) = event.get("response") else {
            return Err(provider_error(
                "terminal event lacks a response object".to_owned(),
            ));
        };
        let response_id = str_field(response, "id")
            .ok_or_else(|| provider_error("response lacks an id".to_owned()))?;
        self.response_id = Some(response_id.to_owned());
        if let Some(usage) = response.get("usage") {
            self.usage = parse_usage(usage);
        }
        let finish_reason = if incomplete {
            let reason = response
                .get("incomplete_details")
                .and_then(|d| d.get("reason"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            match reason {
                "max_output_tokens" => ModelFinishReason::Length,
                "content_filter" => ModelFinishReason::ContentFilter,
                other => ModelFinishReason::Other {
                    reason: other.to_owned(),
                },
            }
        } else {
            derive_completed_finish_reason(response)
        };
        let continuation = match self.storage_mode {
            ResponsesStorageMode::Stored => OpenAiResponsesContinuation::Stored {
                previous_response_id: response_id.to_owned(),
            },
            ResponsesStorageMode::Stateless => {
                let items = response
                    .get("output")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                OpenAiResponsesContinuation::Stateless { items }
            }
        };
        let block_index = self.blocks.peek_next();
        self.terminal_emitted = true;
        let mut events = vec![ModelEvent::ContinuationState {
            block_index,
            state: crate::runtime::continuation::ProviderContinuationState::OpenAiResponses(
                continuation,
            ),
        }];
        events.push(ModelEvent::Completed {
            finish_reason,
            usage: self.usage.take(),
        });
        Ok(events)
    }

    fn push_response_failed(&mut self, event: &serde_json::Value) -> Vec<ModelEvent> {
        self.terminal_emitted = true;
        let response = event.get("response");
        let error = response.and_then(|r| r.get("error"));
        let message = error
            .and_then(|e| e.get("message"))
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| "OpenAI response failed".to_owned(), str::to_owned);
        let provider_code = error
            .and_then(|e| e.get("code"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        vec![ModelEvent::Failed {
            error: ModelError {
                kind: ModelErrorKind::ProviderError,
                message,
                retry_after_ms: None,
                provider_code,
            },
        }]
    }

    /// Terminal handling for a stream that ended without a terminal response
    /// event: that is a normalized failure, never a success.
    fn finish(&mut self) -> Result<Vec<ModelEvent>, ModelError> {
        if self.terminal_emitted {
            return Ok(Vec::new());
        }
        Err(provider_error(
            "provider stream ended without a terminal response event".to_owned(),
        ))
    }
}

/// Derives the semantic finish reason from a completed response's status and
/// normalized output.
fn derive_completed_finish_reason(response: &serde_json::Value) -> ModelFinishReason {
    let status = str_field(response, "status").unwrap_or_default();
    match status {
        "failed" => ModelFinishReason::Other {
            reason: "failed".to_owned(),
        },
        "cancelled" => ModelFinishReason::Other {
            reason: "cancelled".to_owned(),
        },
        "incomplete" => {
            let reason = response
                .get("incomplete_details")
                .and_then(|d| d.get("reason"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            ModelFinishReason::Other {
                reason: reason.to_owned(),
            }
        }
        _ => {
            let output = response
                .get("output")
                .and_then(serde_json::Value::as_array)
                .map_or(&[][..], |items| items.as_slice());
            let has_tool_call = output
                .iter()
                .any(|item| str_field(item, "type").is_some_and(|t| t == "function_call"));
            if has_tool_call {
                return ModelFinishReason::ToolCalls;
            }
            let has_refusal = output.iter().any(|item| {
                item.get("content")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|parts| {
                        parts
                            .iter()
                            .any(|part| str_field(part, "type").is_some_and(|t| t == "refusal"))
                    })
            });
            if has_refusal {
                return ModelFinishReason::Refusal;
            }
            ModelFinishReason::Stop
        }
    }
}

fn parse_usage(usage: &serde_json::Value) -> Option<ModelUsage> {
    let input_tokens = u64_field(usage, "input_tokens")?;
    let output_tokens = u64_field(usage, "output_tokens")?;
    let reasoning_tokens = usage
        .get("output_tokens_details")
        .and_then(|d| u64_field(d, "reasoning_tokens"));
    let cached_input_tokens = usage
        .get("input_tokens_details")
        .and_then(|d| u64_field(d, "cached_tokens"));
    let details =
        (reasoning_tokens.is_some() || cached_input_tokens.is_some()).then_some(UsageDetails {
            reasoning_tokens,
            cached_input_tokens,
        });
    Some(ModelUsage {
        input_tokens,
        output_tokens,
        total_tokens: u64_field(usage, "total_tokens").unwrap_or(input_tokens + output_tokens),
        details,
    })
}

/// Translates a canonical request into a raw Responses request JSON value.
fn translate_request(
    request: &ModelRequest,
    tools: &ValidatedTools,
    storage_mode: ResponsesStorageMode,
) -> Result<serde_json::Value, ModelError> {
    let continuation_variant = match &request.continuation {
        Some(crate::runtime::continuation::ProviderContinuationState::OpenAiResponses(variant)) => {
            Some(variant)
        }
        _ => None,
    };
    match (storage_mode, continuation_variant) {
        (ResponsesStorageMode::Stored, Some(OpenAiResponsesContinuation::Stateless { .. })) => {
            return Err(invalid_request(
                "configured Responses storage mode is Stored but the continuation \
                 variant is Stateless",
            ));
        }
        (ResponsesStorageMode::Stateless, Some(OpenAiResponsesContinuation::Stored { .. })) => {
            return Err(invalid_request(
                "configured Responses storage mode is Stateless but the continuation \
                 variant is Stored",
            ));
        }
        _ => {}
    }

    let (input_items, instructions, previous_response_id) =
        translate_inputs(request, continuation_variant)?;

    // Every runtime-owned structural field is written first; no reasoning
    // field is ever synthesized here, so the selected reasoning profile's
    // configured parameters are exactly what reaches the wire.
    let mut request_value = serde_json::json!({
        "model": request.model(),
        "input": input_items,
        "stream": true,
        "store": storage_mode == ResponsesStorageMode::Stored,
    });
    if storage_mode == ResponsesStorageMode::Stateless {
        // Opaque encrypted reasoning must be requested for stateless replay.
        request_value["include"] = serde_json::json!(["reasoning.encrypted_content"]);
    }
    if let Some(previous_response_id) = previous_response_id {
        request_value["previous_response_id"] = previous_response_id.into();
    }
    if !instructions.is_empty() {
        request_value["instructions"] = instructions.join("\n\n").into();
    }
    request_value["max_output_tokens"] = request.max_output_tokens().into();
    if !request.tools.is_empty() {
        if !request.invocation.capabilities.tool_calls {
            return Err(unsupported(
                "the effective model capabilities do not include tool calls; \
                 tool definitions are never sent to a text-only model",
            ));
        }
        request_value["tools"] = serde_json::json!(translate_tools(&request.tools));
    }
    let _ = tools;
    // The effective opaque request parameters are overlaid last, after every
    // runtime-owned continuation/input/tool/output field is present.
    finalize_provider_request(
        request_value,
        request.request_params(),
        ModelProtocol::OpenAiResponses,
    )
}

/// Translated canonical context: input items, instructions, and the optional
/// stored previous response id.
type TranslatedInputs = (Vec<serde_json::Value>, Vec<String>, Option<String>);

/// Translates the canonical context into Responses input items and
/// instructions according to the continuation variant.
fn translate_inputs(
    request: &ModelRequest,
    continuation_variant: Option<&OpenAiResponsesContinuation>,
) -> Result<TranslatedInputs, ModelError> {
    let mut input_items: Vec<serde_json::Value> = Vec::new();
    let mut instructions: Vec<String> = Vec::new();
    let mut previous_response_id: Option<String> = None;
    let blocks: &[MessageBlock] = match continuation_variant {
        None => &request.messages,
        Some(OpenAiResponsesContinuation::Stored {
            previous_response_id: stored_id,
        }) => {
            previous_response_id = Some(stored_id.clone());
            // Only canonical context after the continuation boundary is sent;
            // the provider-stored conversation supplies everything before it.
            tail_after_boundary(request)?
        }
        Some(OpenAiResponsesContinuation::Stateless { items }) => {
            // The preserved provider-native output items are replayed first,
            // then the canonical context after the continuation boundary.
            // Opaque items (including encrypted reasoning) pass through
            // verbatim and are never re-translated.
            input_items.extend(items.iter().cloned());
            tail_after_boundary(request)?
        }
    };
    // When a continuation slices the canonical request after the previous
    // response boundary, the Agent Status target must exist in the
    // transmitted tail; a status whose target was sliced away fails
    // explicitly instead of being silently dropped.
    if continuation_variant.is_some()
        && let Some(status) = &request.agent_status
        && !blocks.iter().any(|block| {
            matches!(block, MessageBlock::User(user) if user.id == status.target_message_id)
        })
    {
        return Err(invalid_request(
            "Agent Status target message is not in the transmitted continuation tail",
        ));
    }
    for block in blocks {
        match block {
            MessageBlock::System(system) => {
                for text in &system.content {
                    instructions.push(text.text.clone());
                }
            }
            MessageBlock::User(user) => {
                input_items.push(translate_user_input(user, request.agent_status.as_ref())?);
            }
            MessageBlock::Agent(agent) => {
                input_items.extend(translate_agent_inputs(agent)?);
            }
            MessageBlock::Tool(tool) => {
                input_items.push(translate_tool_result(tool)?);
            }
        }
    }
    // The Skill catalog is trusted system context: it is combined
    // deterministically with the canonical system instructions on every
    // request, so a continuation that slices canonical history away never
    // loses the catalog.
    if let Some(catalog) = &request.skill_catalog {
        instructions.push(catalog.rendered.clone());
    }
    Ok((input_items, instructions, previous_response_id))
}

/// The canonical context occurring after the continuation boundary: the
/// latest preceding `AgentMessageBlock` is the boundary, and everything after
/// it is the tail. Requests with continuation state but no preceding agent
/// boundary fail explicitly rather than guessing.
fn tail_after_boundary(request: &ModelRequest) -> Result<&[MessageBlock], ModelError> {
    let boundary = request
        .messages
        .iter()
        .rposition(|block| matches!(block, MessageBlock::Agent(_)))
        .ok_or_else(|| {
            invalid_request(
                "continuation request has no preceding agent message to use as the \
                 continuation boundary",
            )
        })?;
    Ok(&request.messages[boundary + 1..])
}

fn translate_user_input(
    user: &crate::message::types::UserMessageBlock,
    agent_status: Option<&crate::model::types::AgentStatusAttachment>,
) -> Result<serde_json::Value, ModelError> {
    let mut content = Vec::new();
    for block in &user.content {
        match block {
            crate::message::types::UserContentBlock::Text(text) => {
                content.push(serde_json::json!({
                    "type": "input_text",
                    "text": text.text,
                }));
            }
            crate::message::types::UserContentBlock::Image(_)
            | crate::message::types::UserContentBlock::File(_) => {
                return Err(unsupported(
                    "OpenAI Responses cannot represent canonical image/file references \
                     without artifact resolution",
                ));
            }
        }
    }
    // The target fresh inbound user input receives one final input_text unit
    // containing the rendered Agent Status. The status is never a separate
    // input item and never appended to other user inputs of the batch.
    if let Some(status) = agent_status.filter(|status| status.target_message_id == user.id) {
        content.push(serde_json::json!({
            "type": "input_text",
            "text": status.rendered,
        }));
    }
    Ok(serde_json::json!({
        "type": "message",
        "role": "user",
        "content": content,
    }))
}

fn translate_agent_inputs(
    agent: &crate::message::types::AgentMessageBlock,
) -> Result<Vec<serde_json::Value>, ModelError> {
    let mut items = Vec::new();
    let mut message_content = Vec::new();
    let mut tool_calls = Vec::new();
    for content in &agent.content {
        match content {
            AgentContentBlock::Text(text) => {
                message_content.push(serde_json::json!({
                    "type": "input_text",
                    "text": text.text,
                }));
            }
            AgentContentBlock::Refusal(refusal) => {
                message_content.push(serde_json::json!({
                    "type": "refusal",
                    "refusal": refusal.text,
                }));
            }
            AgentContentBlock::Reasoning(reasoning) => {
                // Canonical readable reasoning text is not sufficient evidence
                // to reconstruct a provider-native reasoning item: provider
                // ids, summary structure, and encrypted content cannot be
                // fabricated. Only lossless preserved provider-native state
                // is replayed; anything else fails explicitly instead of
                // degrading into a fabricated summary item.
                match &reasoning.provider_state {
                    Some(ProviderContinuationState::OpenAiResponses(
                        OpenAiResponsesContinuation::Stateless { items: preserved },
                    )) => {
                        items.extend(preserved.iter().cloned());
                    }
                    _ => {
                        return Err(unsupported(
                            "canonical reasoning text alone is not sufficient to reconstruct \
                             an OpenAI Responses reasoning item; preserved provider-native \
                             reasoning state is required for replay",
                        ));
                    }
                }
            }
            AgentContentBlock::ToolCall(call) => {
                let arguments = serde_json::to_string(&call.arguments).map_err(|e| {
                    unsupported(format!(
                        "tool call arguments are not JSON-serializable: {e}"
                    ))
                })?;
                tool_calls.push(serde_json::json!({
                    "type": "function_call",
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": arguments,
                }));
            }
            AgentContentBlock::Image(_) => {
                return Err(unsupported(
                    "OpenAI Responses cannot represent generated image references",
                ));
            }
        }
    }
    if !message_content.is_empty() {
        items.push(serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": message_content,
        }));
    }
    items.extend(tool_calls);
    Ok(items)
}

fn translate_tool_result(
    tool: &crate::message::types::ToolMessageBlock,
) -> Result<serde_json::Value, ModelError> {
    let mut text_parts = Vec::new();
    for content in &tool.result.content {
        match content {
            crate::tools::types::ToolResultContent::Text(text) => {
                text_parts.push(text.text.clone());
            }
            crate::tools::types::ToolResultContent::Json { value } => {
                text_parts.push(serde_json::to_string(value).map_err(|e| {
                    unsupported(format!("tool JSON result is not serializable: {e}"))
                })?);
            }
            crate::tools::types::ToolResultContent::File(_)
            | crate::tools::types::ToolResultContent::Image(_) => {
                return Err(unsupported(
                    "OpenAI Responses cannot represent file/image tool results",
                ));
            }
        }
    }
    Ok(serde_json::json!({
        "type": "function_call_output",
        "call_id": tool.tool_call_id,
        "output": text_parts.join("\n"),
    }))
}

/// Only model-facing tool fields are sent; runtime semantics stay behind.
fn translate_tools(tools: &[crate::tools::types::ModelToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.input_schema,
            })
        })
        .collect()
}

fn str_field<'a>(value: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(serde_json::Value::as_str)
}

fn u32_field(value: &serde_json::Value, field: &str) -> Result<u32, ModelError> {
    value
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| {
            provider_error(format!(
                "provider stream event lacks an integer {field:?} field"
            ))
        })
}

fn u64_field(value: &serde_json::Value, field: &str) -> Option<u64> {
    value.get(field).and_then(serde_json::Value::as_u64)
}

fn provider_error(message: String) -> ModelError {
    ModelError {
        kind: ModelErrorKind::ProviderError,
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

fn unsupported(message: impl Into<String>) -> ModelError {
    let message = message.into();
    ModelError {
        kind: ModelErrorKind::Unsupported,
        message,
        retry_after_ms: None,
        provider_code: None,
    }
}
