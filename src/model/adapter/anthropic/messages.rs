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

use std::collections::BTreeMap;
use std::pin::Pin;

use eventsource_stream::Eventsource;
use futures_util::{Stream, StreamExt};
use reqwest::header::HeaderValue;

use crate::message::types::ContentBlockIndex;
use crate::model::adapter::block_index::BlockAllocator;
use crate::model::adapter::traits::{
    ModelAdapter, ModelEventStream, model_event_stream_of_failure,
};
use crate::model::adapter::validation::{ValidatedTools, validate_request};
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::event::ModelEvent;
use crate::model::types::{ModelProtocol, ModelRequest, ModelUsage};
use crate::runtime::cancellation::CancellationSignal;
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

    fn stream(&self, request: ModelRequest, cancellation: CancellationSignal) -> ModelEventStream {
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
            if cancellation.is_cancelled() {
                // Cancellation before any network request: no provider
                // attempt, no Started, exactly one Failed(Cancelled).
                return Some((
                    ModelEvent::Failed {
                        error: cancelled_error(),
                    },
                    AnthropicPhase::Finished,
                ));
            }
            // The provider request attempt begins: Started is emitted before
            // the network-opening await so the lifecycle stays consistent
            // when cancellation interrupts that await.
            Some((
                ModelEvent::Started,
                AnthropicPhase::Opening {
                    api_key,
                    url,
                    anthropic_version,
                    http_client,
                    wire_request,
                    normalizer,
                    cancellation,
                },
            ))
        }
        AnthropicPhase::Opening {
            api_key,
            url,
            anthropic_version,
            http_client,
            wire_request,
            mut normalizer,
            cancellation,
        } => {
            match open_stream(
                &api_key,
                &url,
                &anthropic_version,
                &http_client,
                &wire_request,
                cancellation.clone(),
            )
            .await
            {
                OpeningOutcome::Streaming(mut stream) => {
                    let mut pending = std::collections::VecDeque::new();
                    streaming_pull(&mut stream, &mut normalizer, &cancellation, &mut pending).await;
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
                OpeningOutcome::Failed(error) => {
                    Some((ModelEvent::Failed { error }, AnthropicPhase::Finished))
                }
            }
        }
        AnthropicPhase::Streaming {
            mut stream,
            mut normalizer,
            cancellation,
            mut pending,
        } => {
            streaming_pull(&mut stream, &mut normalizer, &cancellation, &mut pending).await;
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
enum OpeningOutcome {
    Streaming(SseStream),
    Failed(ModelError),
}

/// Opens the provider SSE stream. The network-opening await itself is
/// cancellation-aware: cancellation while waiting for the response headers
/// drops the in-flight request and terminates with `Failed(Cancelled)`
/// without retry.
async fn open_stream(
    api_key: &str,
    url: &str,
    anthropic_version: &str,
    http_client: &reqwest::Client,
    wire_request: &serde_json::Value,
    cancellation: CancellationSignal,
) -> OpeningOutcome {
    let send = http_client
        .post(url)
        .header(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .header("x-api-key", api_key)
        .header("anthropic-version", anthropic_version)
        .json(wire_request)
        .send();
    let response = tokio::select! {
        response = send => response,
        () = cancellation.cancelled() => {
            // The send future is dropped, aborting the in-flight request.
            return OpeningOutcome::Failed(cancelled_error());
        }
    };
    match response {
        Ok(response) if !response.status().is_success() => {
            let status = response.status();
            let headers = response.headers().clone();
            let body = response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .unwrap_or_default();
            OpeningOutcome::Failed(normalize_http_error(status, &headers, &body))
        }
        Ok(response) => {
            let stream: SseStream = Box::pin(response.bytes_stream().eventsource());
            OpeningOutcome::Streaming(stream)
        }
        Err(reqwest_error) => {
            let kind = if reqwest_error.is_timeout() {
                ModelErrorKind::Timeout
            } else {
                ModelErrorKind::Transport
            };
            OpeningOutcome::Failed(ModelError {
                kind,
                message: reqwest_error.to_string(),
                retry_after_ms: None,
                provider_code: None,
            })
        }
    }
}

/// Pulls provider events into `pending` until at least one event is ready or
/// the invocation is over.
async fn streaming_pull(
    stream: &mut SseStream,
    normalizer: &mut AnthropicStreamNormalizer,
    cancellation: &CancellationSignal,
    pending: &mut std::collections::VecDeque<ModelEvent>,
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
        wire_request: serde_json::Value,
        normalizer: AnthropicStreamNormalizer,
        cancellation: CancellationSignal,
    },
    Opening {
        api_key: String,
        url: String,
        anthropic_version: String,
        http_client: reqwest::Client,
        wire_request: serde_json::Value,
        normalizer: AnthropicStreamNormalizer,
        cancellation: CancellationSignal,
    },
    Streaming {
        stream: SseStream,
        normalizer: AnthropicStreamNormalizer,
        cancellation: CancellationSignal,
        pending: std::collections::VecDeque<ModelEvent>,
    },
    Finished,
}

/// Adapter-local canonical block keys for Anthropic content blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AnthropicBlockKey {
    /// A provider content block identified by its provider index.
    Provider(u32),
    /// The deterministic refusal block allocated for `stop_details`.
    Refusal,
}

/// Per-block assembly state, keyed by the provider block index.
///
/// The block kind is explicit type-state: text, thinking, redacted thinking,
/// and tool use hold only the state their kind needs, so impossible
/// combinations (for example a text delta on a tool block) fail hard.
#[derive(Debug)]
enum AnthropicBlockState {
    Text {
        canonical_index: ContentBlockIndex,
    },
    Thinking {
        canonical_index: ContentBlockIndex,
        /// Accumulated visible thinking text (replayed verbatim inside the
        /// opaque provider state; never emitted as a whole).
        buffer: String,
        /// The provider signature, when received.
        signature: Option<String>,
        /// Whether the continuation state has been emitted.
        state_emitted: bool,
    },
    RedactedThinking {
        canonical_index: ContentBlockIndex,
        /// The opaque encrypted provider state, preserved verbatim.
        data: String,
        state_emitted: bool,
    },
    ToolUse {
        canonical_index: ContentBlockIndex,
        call_id: ToolCallId,
        tool_id: ToolId,
        name: String,
        argument_buffer: String,
        completed: bool,
    },
}

/// Normalizes the Anthropic stream into canonical events.
#[derive(Debug)]
struct AnthropicStreamNormalizer {
    tools: ValidatedTools,
    blocks: BlockAllocator<AnthropicBlockKey>,
    block_states: BTreeMap<u32, AnthropicBlockState>,
    message_start_usage: Option<WireUsage>,
    latest_usage: Option<WireUsage>,
    stop_reason: Option<String>,
    stop_details: Option<super::wire::WireStopDetails>,
    refusal_block: Option<ContentBlockIndex>,
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
            stop_details: None,
            refusal_block: None,
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
            WireEvent::MessageDelta {
                delta,
                usage,
                stop_details,
            } => {
                if let Some(stop_reason) = delta.stop_reason {
                    self.stop_reason = Some(stop_reason);
                }
                if stop_details.is_some() {
                    self.stop_details = stop_details;
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
                let canonical_index = self.blocks.allocate(AnthropicBlockKey::Provider(index));
                self.block_states
                    .insert(index, AnthropicBlockState::Text { canonical_index });
                Ok(Vec::new())
            }
            Some("thinking") => {
                let canonical_index = self.blocks.allocate(AnthropicBlockKey::Provider(index));
                self.block_states.insert(
                    index,
                    AnthropicBlockState::Thinking {
                        canonical_index,
                        buffer: block.thinking.clone().unwrap_or_default(),
                        signature: block.signature.clone(),
                        state_emitted: false,
                    },
                );
                Ok(Vec::new())
            }
            Some("redacted_thinking") => {
                // `data` is the block's required opaque content; a block
                // without it cannot be preserved or replayed losslessly, so
                // it is a provider protocol error rather than an empty
                // fabricated opaque value.
                let Some(data) = block.data.clone() else {
                    return Err(provider_error(
                        "provider redacted_thinking block lacks the opaque data field".to_owned(),
                    ));
                };
                let canonical_index = self.blocks.allocate(AnthropicBlockKey::Provider(index));
                self.block_states.insert(
                    index,
                    AnthropicBlockState::RedactedThinking {
                        canonical_index,
                        data,
                        state_emitted: false,
                    },
                );
                Ok(Vec::new())
            }
            Some("tool_use") => {
                let canonical_index = self.blocks.allocate(AnthropicBlockKey::Provider(index));
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
                    AnthropicBlockState::ToolUse {
                        canonical_index,
                        call_id: ToolCallId::new(id),
                        tool_id,
                        name,
                        argument_buffer: String::new(),
                        completed: false,
                    },
                );
                Ok(vec![ModelEvent::ToolCallStarted {
                    block_index: canonical_index,
                    call,
                }])
            }
            Some("fallback") => {
                // A provider `fallback` block is not disposable transport
                // metadata: it carries provider positional/replay semantics
                // (its position validates the thinking blocks around a model
                // handoff, and it must be echoed back unchanged in later
                // requests). rustX cannot preserve that losslessly with the
                // current canonical continuation model, so the adapter fails
                // explicitly rather than silently discarding the block. No
                // canonical index is allocated and no continuation state is
                // fabricated for it.
                Err(unsupported(
                    "Anthropic fallback blocks require lossless positional \
                     replay; server-side fallback is not supported by the M2 \
                     adapter",
                ))
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
                let AnthropicBlockState::Text { canonical_index } = state else {
                    return Err(provider_error(format!(
                        "text delta for non-text block index {index}"
                    )));
                };
                let text = delta.text.clone().unwrap_or_default();
                if text.is_empty() {
                    return Ok(Vec::new());
                }
                Ok(vec![ModelEvent::TextDelta {
                    block_index: *canonical_index,
                    text,
                }])
            }
            Some("thinking_delta") => {
                let state = self.block_states.get_mut(&index).ok_or_else(|| {
                    provider_error(format!("thinking delta for unknown block index {index}"))
                })?;
                let AnthropicBlockState::Thinking {
                    canonical_index,
                    buffer,
                    ..
                } = state
                else {
                    return Err(provider_error(format!(
                        "thinking delta for non-thinking block index {index}"
                    )));
                };
                let text = delta.thinking.clone().unwrap_or_default();
                buffer.push_str(&text);
                if text.is_empty() {
                    return Ok(Vec::new());
                }
                Ok(vec![ModelEvent::ReasoningDelta {
                    block_index: *canonical_index,
                    text,
                }])
            }
            Some("signature_delta") => {
                let state = self.block_states.get_mut(&index).ok_or_else(|| {
                    provider_error(format!("signature delta for unknown block index {index}"))
                })?;
                let AnthropicBlockState::Thinking { signature, .. } = state else {
                    return Err(provider_error(format!(
                        "signature delta for non-thinking block index {index}"
                    )));
                };
                if let Some(new_signature) = delta.signature.clone() {
                    *signature = Some(new_signature);
                }
                Ok(Vec::new())
            }
            Some("input_json_delta") => {
                let state = self.block_states.get_mut(&index).ok_or_else(|| {
                    provider_error(format!("input delta for unknown block index {index}"))
                })?;
                let AnthropicBlockState::ToolUse {
                    canonical_index,
                    call_id,
                    argument_buffer,
                    ..
                } = state
                else {
                    return Err(provider_error(format!(
                        "input delta for non-tool block index {index}"
                    )));
                };
                let partial = delta.partial_json.clone().unwrap_or_default();
                argument_buffer.push_str(&partial);
                if partial.is_empty() {
                    return Ok(Vec::new());
                }
                Ok(vec![ModelEvent::ToolCallArgumentsDelta {
                    block_index: *canonical_index,
                    call_id: call_id.clone(),
                    arguments_delta: partial,
                }])
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
            // A stop for a block that never opened a canonical state (for
            // example a provider control block) has nothing to finalize.
            return Ok(Vec::new());
        };
        match state {
            AnthropicBlockState::Text { .. } => Ok(Vec::new()),
            AnthropicBlockState::Thinking {
                canonical_index,
                buffer,
                signature,
                state_emitted,
            } => {
                if *state_emitted {
                    return Ok(Vec::new());
                }
                // The streaming protocol always sends a `signature_delta`
                // before the thinking block stops (the signature is the
                // encrypted provider state that makes replay possible). A
                // block without one (including the empty placeholder from
                // `content_block_start`) cannot be replayed losslessly, so
                // it is a provider protocol error instead of a reasoning
                // block without continuation state.
                let Some(signature) = signature.clone().filter(|s| !s.is_empty()) else {
                    return Err(provider_error(
                        "provider thinking block stopped without a signature; a \
                         thinking block without provider state cannot be replayed \
                         losslessly"
                            .to_owned(),
                    ));
                };
                *state_emitted = true;
                let opaque = serde_json::json!({
                    "type": "thinking",
                    "thinking": buffer,
                    "signature": signature,
                });
                Ok(vec![ModelEvent::ContinuationState {
                    block_index: *canonical_index,
                    state: ProviderContinuationState::Anthropic(AnthropicContinuation { opaque }),
                }])
            }
            AnthropicBlockState::RedactedThinking {
                canonical_index,
                data,
                state_emitted,
            } => {
                if *state_emitted {
                    return Ok(Vec::new());
                }
                *state_emitted = true;
                // The provider block is preserved losslessly as its full
                // provider object; the opaque data is never interpreted,
                // decrypted, modified, or fabricated.
                let opaque = serde_json::json!({
                    "type": "redacted_thinking",
                    "data": data,
                });
                Ok(vec![ModelEvent::ContinuationState {
                    block_index: *canonical_index,
                    state: ProviderContinuationState::Anthropic(AnthropicContinuation { opaque }),
                }])
            }
            AnthropicBlockState::ToolUse {
                canonical_index,
                call_id,
                tool_id,
                name,
                argument_buffer,
                completed,
            } => {
                if *completed {
                    return Ok(Vec::new());
                }
                // The complete JSON is parsed exactly once, at block stop.
                let parsed = serde_json::from_str(argument_buffer).map_err(|e| {
                    provider_error(format!(
                        "malformed complete tool arguments for {name:?} ({call_id}): {e}"
                    ))
                })?;
                *completed = true;
                Ok(vec![ModelEvent::ToolCallCompleted {
                    block_index: *canonical_index,
                    call: ToolCall {
                        id: call_id.clone(),
                        tool_id: tool_id.clone(),
                        name: name.clone(),
                        arguments: parsed,
                    },
                }])
            }
        }
    }

    /// Handles `message_stop`: emits refusal semantics when the provider
    /// terminated with `stop_reason = refusal`, then the terminal event.
    ///
    /// Provider deltas were already streamed as they arrived; `ModelEvent`
    /// is provisional adapter output and the completed
    /// `AgentMessageBlock` assembly (including any rollback of partial
    /// output on a refusal) is owned by the future Agent Loop, not by M2.
    fn terminal(&mut self) -> Result<Vec<ModelEvent>, ModelError> {
        if self.terminal_emitted {
            return Ok(Vec::new());
        }
        self.terminal_emitted = true;
        let stop_reason = self.stop_reason.clone().ok_or_else(|| {
            provider_error("provider stream reached message_stop without a stop reason".to_owned())
        })?;
        let mut events = Vec::new();
        if is_refusal(Some(&stop_reason))
            && let Some(explanation) = self
                .stop_details
                .as_ref()
                .and_then(|details| details.explanation.clone())
                .filter(|explanation| !explanation.is_empty())
        {
            let block_index = *self
                .refusal_block
                .get_or_insert_with(|| self.blocks.allocate(AnthropicBlockKey::Refusal));
            events.push(ModelEvent::RefusalDelta {
                block_index,
                text: explanation,
            });
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
