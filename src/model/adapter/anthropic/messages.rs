//! Anthropic Messages adapter (direct HTTP/SSE transport).
//!
//! Anthropic has no official Rust SDK, and the evaluated community SDK has
//! stale typed stop-reason coverage relative to the current Messages API, so
//! the adapter talks to `/v1/messages` directly with `reqwest` and
//! `eventsource-stream`. The wire representation lives in `wire.rs`, the
//! request/error/finish mapping in `mapping.rs`, and no Anthropic SDK type
//! exists anywhere in rustX.
//!
//! The transport performs exactly one HTTP request per adapter invocation:
//! no retry, no reconnect, no failover.

use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;

use eventsource_stream::Eventsource;
use futures_util::{Stream, StreamExt};
use reqwest::header::HeaderValue;

use crate::message::types::ContentBlockIndex;
use crate::model::adapter::block_index::BlockAllocator;
use crate::model::adapter::cancellation::ModelCancellation;
use crate::model::adapter::traits::{
    ModelAdapter, ModelEventStream, model_event_stream_of_failure,
};
use crate::model::adapter::validation::{ValidatedTools, validate_request};
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::event::ModelEvent;
use crate::model::types::{ModelProtocol, ModelRequest, ModelUsage};
use crate::runtime::continuation::{AnthropicContinuation, ProviderContinuationState};
use crate::runtime::identity::{ToolCallId, ToolId};
use crate::tools::types::{ToolCall, ToolCallStart};

use super::config::AnthropicAdapterConfig;
use super::mapping::{
    is_refusal, map_finish_reason, normalize_http_error, normalize_usage, resolve_tool,
    translate_request,
};
use super::wire::{WireEvent, WireUsage, parse_event};

/// The boxed provider SSE stream type.
type SseStream = Pin<
    Box<
        dyn Stream<
                Item = Result<
                    eventsource_stream::Event,
                    eventsource_stream::EventStreamError<reqwest::Error>,
                >,
            > + Send,
    >,
>;

/// Adapter for the Anthropic Messages protocol.
pub struct AnthropicMessagesAdapter {
    api_key: String,
    api_base: String,
    anthropic_version: String,
    http_client: reqwest::Client,
}

impl std::fmt::Debug for AnthropicMessagesAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicMessagesAdapter")
            .field("api_key", &"<redacted>")
            .field("api_base", &self.api_base)
            .field("anthropic_version", &self.anthropic_version)
            .finish_non_exhaustive()
    }
}

impl AnthropicMessagesAdapter {
    /// Creates the adapter from rustX-owned configuration.
    #[must_use]
    pub fn new(config: AnthropicAdapterConfig) -> Self {
        let (api_key, api_base, anthropic_version, http_client) = config.into_parts();
        Self {
            api_key,
            api_base,
            anthropic_version,
            http_client: http_client.unwrap_or_default(),
        }
    }
}

impl ModelAdapter for AnthropicMessagesAdapter {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::AnthropicMessages
    }

    fn stream(&self, request: ModelRequest, cancellation: ModelCancellation) -> ModelEventStream {
        let validated = match validate_request(&request, self.protocol()) {
            Ok(validated) => validated,
            Err(error) => return model_event_stream_of_failure(error),
        };
        let wire_request = match translate_request(&request, &validated) {
            Ok(translated) => translated,
            Err(error) => return model_event_stream_of_failure(error),
        };
        let state = AnthropicPhase::Preparing {
            api_key: self.api_key.clone(),
            url: format!("{}/v1/messages", self.api_base),
            anthropic_version: self.anthropic_version.clone(),
            http_client: self.http_client.clone(),
            wire_request,
            normalizer: AnthropicStreamNormalizer::new(validated),
            cancellation,
        };
        Box::pin(futures_util::stream::unfold(state, anthropic_phase_next))
    }
}

async fn anthropic_phase_next(phase: AnthropicPhase) -> Option<(ModelEvent, AnthropicPhase)> {
    match phase {
        AnthropicPhase::Preparing {
            api_key,
            url,
            anthropic_version,
            http_client,
            wire_request,
            normalizer,
            cancellation,
        } => {
            match preparing_poll(
                &api_key,
                &url,
                &anthropic_version,
                &http_client,
                &wire_request,
                cancellation.clone(),
            )
            .await
            {
                Some(PreparationOutcome::Streaming(stream)) => Some((
                    ModelEvent::Started,
                    AnthropicPhase::Streaming {
                        stream,
                        normalizer,
                        cancellation,
                        pending: VecDeque::new(),
                    },
                )),
                Some(PreparationOutcome::Failed(error)) => {
                    Some((ModelEvent::Started, AnthropicPhase::Failing { error }))
                }
                None => Some((
                    ModelEvent::Failed {
                        error: cancelled_error(),
                    },
                    AnthropicPhase::Finished,
                )),
            }
        }
        AnthropicPhase::Failing { error } => {
            Some((ModelEvent::Failed { error }, AnthropicPhase::Finished))
        }
        AnthropicPhase::Streaming {
            mut stream,
            mut normalizer,
            cancellation,
            mut pending,
        } => {
            streaming_poll(&mut stream, &mut normalizer, &cancellation, &mut pending).await;
            let event = pending.pop_front().expect("pending is non-empty here");
            let next_phase = if is_terminal(&event) {
                AnthropicPhase::Finished
            } else {
                AnthropicPhase::Streaming {
                    stream,
                    normalizer,
                    cancellation,
                    pending,
                }
            };
            Some((event, next_phase))
        }
        AnthropicPhase::Finished => None,
    }
}

/// Pulls provider events into `pending` until at least one event is ready
/// or the invocation is over.
enum PreparationOutcome {
    Streaming(SseStream),
    Failed(ModelError),
}

/// Performs the single provider HTTP request attempt. Returns `None` when
/// cancellation arrived before any network request.
async fn preparing_poll(
    api_key: &str,
    url: &str,
    anthropic_version: &str,
    http_client: &reqwest::Client,
    wire_request: &super::mapping::WireRequest,
    cancellation: ModelCancellation,
) -> Option<PreparationOutcome> {
    if cancellation.is_cancelled() {
        return None;
    }
    let response = http_client
        .post(url)
        .header(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .header("x-api-key", api_key)
        .header("anthropic-version", anthropic_version)
        .json(wire_request)
        .send()
        .await;
    match response {
        Ok(response) if !response.status().is_success() => {
            let status = response.status();
            let headers = response.headers().clone();
            let body = response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .unwrap_or_default();
            Some(PreparationOutcome::Failed(normalize_http_error(
                status, &headers, &body,
            )))
        }
        Ok(response) => {
            let stream: SseStream = Box::pin(response.bytes_stream().eventsource());
            Some(PreparationOutcome::Streaming(stream))
        }
        Err(reqwest_error) => {
            let kind = if reqwest_error.is_timeout() {
                ModelErrorKind::Timeout
            } else {
                ModelErrorKind::Transport
            };
            Some(PreparationOutcome::Failed(ModelError {
                kind,
                message: reqwest_error.to_string(),
                retry_after_ms: None,
                provider_code: None,
            }))
        }
    }
}

async fn streaming_poll(
    stream: &mut SseStream,
    normalizer: &mut AnthropicStreamNormalizer,
    cancellation: &ModelCancellation,
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
            Some(Ok(event)) => match normalizer.push_event(&event.data) {
                Ok(events) => pending.extend(events),
                Err(error) => pending.push_back(ModelEvent::Failed { error }),
            },
            Some(Err(error)) => {
                pending.push_back(ModelEvent::Failed {
                    error: sse_failure(&error),
                });
                break;
            }
            None => {
                // The provider stream ended; if the normalizer has not
                // already emitted a terminal event this is a failure.
                match normalizer.finish() {
                    Ok(events) => pending.extend(events),
                    Err(error) => pending.push_back(ModelEvent::Failed { error }),
                }
                break;
            }
        }
    }
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

fn sse_failure(error: &eventsource_stream::EventStreamError<reqwest::Error>) -> ModelError {
    ModelError {
        kind: ModelErrorKind::Transport,
        message: format!("Anthropic SSE stream failed: {error}"),
        retry_after_ms: None,
        provider_code: None,
    }
}

enum AnthropicPhase {
    Preparing {
        api_key: String,
        url: String,
        anthropic_version: String,
        http_client: reqwest::Client,
        wire_request: super::mapping::WireRequest,
        normalizer: AnthropicStreamNormalizer,
        cancellation: ModelCancellation,
    },
    Failing {
        error: ModelError,
    },
    Streaming {
        stream: SseStream,
        normalizer: AnthropicStreamNormalizer,
        cancellation: ModelCancellation,
        pending: VecDeque<ModelEvent>,
    },
    Finished,
}

/// Per-block assembly state keyed by the provider block index.
///
/// All block events are buffered per block and flushed in canonical order at
/// the terminal stop reason: Anthropic mandates discarding partial output on
/// a refusal, and buffering keeps the canonical event order deterministic.
#[derive(Debug)]
struct BlockState {
    /// The canonical content block index.
    canonical_index: ContentBlockIndex,
    /// Buffered normalized events for this block, flushed in canonical order.
    pending: Vec<ModelEvent>,
    /// Text, thinking, or tool-arguments accumulation buffer.
    buffer: String,
    /// Thinking signature, when known.
    signature: Option<String>,
    /// Redacted thinking marker, when present.
    redacted: Option<String>,
    /// Tool use identity (`tool_use` blocks only) and completion flag.
    tool: Option<(ToolCallId, ToolId, String, bool)>,
}

/// Normalizes the Anthropic stream into canonical events.
#[derive(Debug)]
struct AnthropicStreamNormalizer {
    tools: ValidatedTools,
    blocks: BlockAllocator<u32>,
    block_states: BTreeMap<u32, BlockState>,
    message_start_usage: Option<WireUsage>,
    latest_usage: Option<WireUsage>,
    stop_reason: Option<String>,
    terminal_emitted: bool,
}

impl AnthropicStreamNormalizer {
    fn new(tools: ValidatedTools) -> Self {
        Self {
            tools,
            blocks: BlockAllocator::new(),
            block_states: BTreeMap::new(),
            message_start_usage: None,
            latest_usage: None,
            stop_reason: None,
            terminal_emitted: false,
        }
    }

    fn push_event(&mut self, data: &str) -> Result<Vec<ModelEvent>, ModelError> {
        let event = parse_event(data)
            .map_err(|e| provider_error(format!("malformed Anthropic stream event: {e}")))?;
        match event {
            WireEvent::MessageStart { message } => {
                self.message_start_usage = message.usage;
                Ok(Vec::new())
            }
            WireEvent::ContentBlockStart { index, block } => {
                self.push_content_block_start(index, &block)
            }
            WireEvent::ContentBlockDelta { index, delta } => {
                self.push_content_block_delta(index, &delta)
            }
            WireEvent::ContentBlockStop { index } => self.push_content_block_stop(index),
            WireEvent::MessageDelta { delta, usage } => {
                if let Some(stop_reason) = delta.stop_reason {
                    self.stop_reason = Some(stop_reason);
                }
                if usage.is_some() {
                    self.latest_usage = usage;
                }
                Ok(Vec::new())
            }
            WireEvent::MessageStop => self.terminal(),
            WireEvent::Ping | WireEvent::Unknown => Ok(Vec::new()),
            WireEvent::Error { error } => Err(ModelError {
                kind: ModelErrorKind::ProviderError,
                message: format!(
                    "Anthropic stream error{}: {}",
                    error
                        .error_type
                        .as_deref()
                        .map(|t| format!(" ({t})"))
                        .unwrap_or_default(),
                    error.message.as_deref().unwrap_or("unknown")
                ),
                retry_after_ms: None,
                provider_code: error.error_type,
            }),
        }
    }

    fn push_content_block_start(
        &mut self,
        index: u32,
        block: &super::wire::WireContentBlock,
    ) -> Result<Vec<ModelEvent>, ModelError> {
        match block.block_type.as_deref() {
            Some("text") => {
                let canonical_index = self.blocks.allocate(index);
                self.block_states.insert(
                    index,
                    BlockState {
                        canonical_index,
                        pending: Vec::new(),
                        buffer: block.text.clone().unwrap_or_default(),
                        signature: None,
                        redacted: None,
                        tool: None,
                    },
                );
                Ok(Vec::new())
            }
            Some("thinking" | "redacted_thinking") => {
                let canonical_index = self.blocks.allocate(index);
                self.block_states.insert(
                    index,
                    BlockState {
                        canonical_index,
                        pending: Vec::new(),
                        buffer: block.thinking.clone().unwrap_or_default(),
                        signature: block.signature.clone(),
                        redacted: block.redacted_thinking.clone(),
                        tool: None,
                    },
                );
                Ok(Vec::new())
            }
            Some("tool_use") => {
                let canonical_index = self.blocks.allocate(index);
                let Some(id) = block.id.clone().filter(|id| !id.is_empty()) else {
                    return Err(provider_error(
                        "provider tool_use block lacks an invocation id".to_owned(),
                    ));
                };
                let Some(name) = block.name.clone() else {
                    return Err(provider_error(
                        "provider tool_use block lacks a tool name".to_owned(),
                    ));
                };
                let tool_id = resolve_tool(&self.tools, &name)?;
                let call = ToolCallStart {
                    id: ToolCallId::new(id.clone()),
                    tool_id: tool_id.clone(),
                    name: name.clone(),
                };
                self.block_states.insert(
                    index,
                    BlockState {
                        canonical_index,
                        pending: vec![ModelEvent::ToolCallStarted {
                            block_index: canonical_index,
                            call,
                        }],
                        buffer: String::new(),
                        signature: None,
                        redacted: None,
                        tool: Some((ToolCallId::new(id), tool_id, name, false)),
                    },
                );
                Ok(Vec::new())
            }
            Some("fallback") => {
                // Provider transport/control metadata: no canonical content
                // block is allocated and provider indexes do not shift
                // canonical indexes.
                Ok(Vec::new())
            }
            Some("server_tool_use" | "mcp_tool_use" | "web_search_tool_result") => {
                Err(unsupported(format!(
                    "provider-hosted tool block type {:?} is not a rustX tool call",
                    block.block_type.as_deref().expect("matched")
                )))
            }
            Some(other) => Err(unsupported(format!(
                "unsupported output-bearing content block type {other:?}"
            ))),
            None => Err(provider_error("content block without a type".to_owned())),
        }
    }

    fn push_content_block_delta(
        &mut self,
        index: u32,
        delta: &super::wire::WireDelta,
    ) -> Result<Vec<ModelEvent>, ModelError> {
        match delta.delta_type.as_deref() {
            Some("text_delta") => {
                let state = self.block_states.get_mut(&index).ok_or_else(|| {
                    provider_error(format!("text delta for unknown block index {index}"))
                })?;
                if state.tool.is_some() {
                    return Err(provider_error(format!(
                        "text delta for non-text block index {index}"
                    )));
                }
                state
                    .buffer
                    .push_str(delta.text.as_deref().unwrap_or_default());
                Ok(Vec::new())
            }
            Some("thinking_delta") => {
                let state = self.block_states.get_mut(&index).ok_or_else(|| {
                    provider_error(format!("thinking delta for unknown block index {index}"))
                })?;
                if state.tool.is_some() {
                    return Err(provider_error(format!(
                        "thinking delta for non-thinking block index {index}"
                    )));
                }
                state
                    .buffer
                    .push_str(delta.thinking.as_deref().unwrap_or_default());
                Ok(Vec::new())
            }
            Some("signature_delta") => {
                let state = self.block_states.get_mut(&index).ok_or_else(|| {
                    provider_error(format!("signature delta for unknown block index {index}"))
                })?;
                if state.tool.is_some() {
                    return Err(provider_error(format!(
                        "signature delta for non-thinking block index {index}"
                    )));
                }
                if let Some(new_signature) = delta.signature.clone() {
                    state.signature = Some(new_signature);
                }
                Ok(Vec::new())
            }
            Some("input_json_delta") => {
                let state = self.block_states.get_mut(&index).ok_or_else(|| {
                    provider_error(format!("input delta for unknown block index {index}"))
                })?;
                let Some((call_id, _, _, _)) = state.tool.as_ref() else {
                    return Err(provider_error(format!(
                        "input delta for non-tool block index {index}"
                    )));
                };
                let partial = delta.partial_json.clone().unwrap_or_default();
                state.buffer.push_str(&partial);
                if !partial.is_empty() {
                    state.pending.push(ModelEvent::ToolCallArgumentsDelta {
                        block_index: state.canonical_index,
                        call_id: call_id.clone(),
                        arguments_delta: partial,
                    });
                }
                Ok(Vec::new())
            }
            Some(other) => Err(unsupported(format!(
                "unsupported content block delta type {other:?}"
            ))),
            None => Err(provider_error(
                "content block delta without a type".to_owned(),
            )),
        }
    }

    fn push_content_block_stop(&mut self, index: u32) -> Result<Vec<ModelEvent>, ModelError> {
        let Some(state) = self.block_states.get_mut(&index) else {
            // Unknown blocks (for example a fallback block pair) stop without
            // allocating any canonical content.
            return Ok(Vec::new());
        };
        let Some((call_id, tool_id, name, completed)) = state.tool.as_mut() else {
            return Ok(Vec::new());
        };
        if *completed {
            return Ok(Vec::new());
        }
        // The complete JSON is parsed exactly once, at block stop.
        let parsed = serde_json::from_str(&state.buffer).map_err(|e| {
            provider_error(format!(
                "malformed complete tool arguments for {name:?} ({call_id}): {e}"
            ))
        })?;
        *completed = true;
        state.pending.push(ModelEvent::ToolCallCompleted {
            block_index: state.canonical_index,
            call: ToolCall {
                id: call_id.clone(),
                tool_id: tool_id.clone(),
                name: name.clone(),
                arguments: parsed,
            },
        });
        Ok(Vec::new())
    }

    /// Handles `message_stop`: flushes buffered content in canonical block
    /// order and emits the terminal event.
    ///
    /// On a refusal the provider mandates discarding partial output: the
    /// buffered content is dropped instead of emitted.
    fn terminal(&mut self) -> Result<Vec<ModelEvent>, ModelError> {
        if self.terminal_emitted {
            return Ok(Vec::new());
        }
        self.terminal_emitted = true;
        let stop_reason = self.stop_reason.clone().ok_or_else(|| {
            provider_error("provider stream reached message_stop without a stop reason".to_owned())
        })?;
        let mut events = Vec::new();
        if !is_refusal(Some(&stop_reason)) {
            for state in self.block_states.values_mut() {
                let mut block_events = std::mem::take(&mut state.pending);
                if state.tool.is_none() {
                    // Text and thinking blocks finalize their buffered content
                    // here; tool blocks were already assembled at stop.
                    if !state.buffer.is_empty() {
                        if state.signature.is_none() && state.redacted.is_none() {
                            block_events.push(ModelEvent::TextDelta {
                                block_index: state.canonical_index,
                                text: state.buffer.clone(),
                            });
                        } else {
                            block_events.push(ModelEvent::ReasoningDelta {
                                block_index: state.canonical_index,
                                text: state.buffer.clone(),
                            });
                        }
                    }
                    if state.signature.is_some() || state.redacted.is_some() {
                        let opaque = serde_json::json!({
                            "type": if state.redacted.is_some() { "redacted_thinking" } else { "thinking" },
                            "thinking": state.buffer,
                            "signature": state.signature,
                            "redacted_thinking": state.redacted,
                        });
                        block_events.push(ModelEvent::ContinuationState {
                            block_index: state.canonical_index,
                            state: ProviderContinuationState::Anthropic(AnthropicContinuation {
                                opaque,
                            }),
                        });
                    }
                }
                events.extend(block_events);
            }
        }
        events.push(ModelEvent::Completed {
            finish_reason: map_finish_reason(Some(&stop_reason)),
            usage: Some(self.final_usage()),
        });
        Ok(events)
    }

    fn final_usage(&self) -> ModelUsage {
        normalize_usage(
            self.message_start_usage.as_ref(),
            self.latest_usage.as_ref(),
        )
    }

    /// Stream ended without a terminal event: normalized failure.
    fn finish(&mut self) -> Result<Vec<ModelEvent>, ModelError> {
        if self.terminal_emitted {
            return Ok(Vec::new());
        }
        Err(provider_error(
            "provider stream ended without a terminal message event".to_owned(),
        ))
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

fn unsupported(message: impl Into<String>) -> ModelError {
    let message = message.into();
    ModelError {
        kind: ModelErrorKind::Unsupported,
        message,
        retry_after_ms: None,
        provider_code: None,
    }
}
