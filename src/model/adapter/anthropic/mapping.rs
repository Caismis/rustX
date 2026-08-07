//! Anthropic Messages normalization: errors, usage, finish reasons, and
//! canonical request translation.

use reqwest::{StatusCode, header::HeaderMap};

use crate::message::types::{AgentContentBlock, MessageBlock, UserContentBlock};
use crate::model::adapter::validation::ValidatedTools;
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::finish::ModelFinishReason;
use crate::model::types::{ModelUsage, ReasoningEffort, UsageDetails};
use crate::runtime::continuation::{AnthropicContinuation, ProviderContinuationState};
use crate::runtime::identity::ToolId;

use super::wire::WireUsage;

/// Default `max_tokens` when the canonical request carries none. Anthropic
/// requires the field; rustX policy (M3+) will own the real budget.
pub(crate) const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Maps an Anthropic stop reason to the canonical finish reason.
pub(crate) fn map_finish_reason(stop_reason: Option<&str>) -> ModelFinishReason {
    match stop_reason {
        Some("end_turn" | "stop_sequence") => ModelFinishReason::Stop,
        Some("tool_use") => ModelFinishReason::ToolCalls,
        Some("max_tokens" | "model_context_window_exceeded") => ModelFinishReason::Length,
        Some("refusal") => ModelFinishReason::Refusal,
        // `pause_turn` has continuation semantics and is never mapped to an
        // ordinary stop.
        Some("pause_turn") => ModelFinishReason::Other {
            reason: "pause_turn".to_owned(),
        },
        Some(other) => ModelFinishReason::Other {
            reason: other.to_owned(),
        },
        None => ModelFinishReason::Other {
            reason: "unknown".to_owned(),
        },
    }
}

/// Whether the provider wants partial output discarded (refusal semantics).
pub(crate) fn is_refusal(stop_reason: Option<&str>) -> bool {
    matches!(stop_reason, Some("refusal"))
}

/// Combines usage from `message_start` and the latest cumulative
/// `message_delta` snapshot without summing snapshots.
pub(crate) fn normalize_usage(
    message_start_usage: Option<&WireUsage>,
    latest_delta_usage: Option<&WireUsage>,
) -> ModelUsage {
    let input_tokens = latest_delta_usage
        .and_then(|u| u.input_tokens)
        .or_else(|| message_start_usage.and_then(|u| u.input_tokens))
        .unwrap_or(0);
    let output_tokens = latest_delta_usage
        .and_then(|u| u.output_tokens)
        .unwrap_or(0);
    let cached_input_tokens = latest_delta_usage
        .and_then(|u| u.cache_read_input_tokens)
        .or_else(|| message_start_usage.and_then(|u| u.cache_read_input_tokens));
    let details = cached_input_tokens.map(|cached| UsageDetails {
        reasoning_tokens: None,
        cached_input_tokens: Some(cached),
    });
    ModelUsage {
        input_tokens,
        output_tokens,
        total_tokens: input_tokens + output_tokens,
        details,
    }
}

/// Normalizes an HTTP error into a runtime-owned error.
pub(crate) fn normalize_http_error(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> ModelError {
    let (provider_code, message) = parse_error_body(body);
    let message = message.unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());
    let retry_after_ms = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1000));
    let kind = match status.as_u16() {
        400 => {
            if is_context_window_message(&message, provider_code.as_deref()) {
                ModelErrorKind::ContextWindowExceeded
            } else {
                ModelErrorKind::InvalidRequest
            }
        }
        401 | 403 => ModelErrorKind::Authentication,
        404 => ModelErrorKind::InvalidRequest,
        408 | 409 => ModelErrorKind::Timeout,
        429 => ModelErrorKind::RateLimit,
        _ => ModelErrorKind::ProviderError,
    };
    ModelError {
        kind,
        message,
        retry_after_ms,
        provider_code,
    }
}

fn is_context_window_message(message: &str, provider_code: Option<&str>) -> bool {
    if matches!(
        provider_code,
        Some("prompt_too_long" | "input_too_long" | "context_length_exceeded")
    ) {
        return true;
    }
    let lower = message.to_ascii_lowercase();
    lower.contains("prompt is too long")
        || lower.contains("context length")
        || lower.contains("context window")
        || lower.contains("input is too long")
}

#[derive(serde::Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: Option<ErrorDetail>,
}

#[derive(serde::Deserialize)]
struct ErrorDetail {
    #[serde(rename = "type", default)]
    error_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

fn parse_error_body(body: &[u8]) -> (Option<String>, Option<String>) {
    match serde_json::from_slice::<ErrorBody>(body) {
        Ok(ErrorBody { error: Some(detail) }) => (detail.error_type, detail.message),
        Ok(_) | Err(_) => (None, None),
    }
}

/// The request body sent to `/v1/messages`.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct WireRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<WireRequestMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<WireTextBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<WireTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<serde_json::Value>,
    pub stream: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct WireRequestMessage {
    pub role: &'static str,
    pub content: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct WireTextBlock {
    pub r#type: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct WireTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// Translates a canonical request into the Anthropic wire request.
pub(crate) fn translate_request(
    request: &crate::model::types::ModelRequest,
    tools: &ValidatedTools,
) -> Result<WireRequest, ModelError> {
    let thinking = match request.reasoning {
        ReasoningEffort::Minimal => {
            return Err(ModelError {
                kind: ModelErrorKind::Unsupported,
                message: "Anthropic cannot represent ReasoningEffort::Minimal; it is not \
                          remapped to another effort level"
                    .to_owned(),
                retry_after_ms: None,
                provider_code: None,
            });
        }
        ReasoningEffort::Low => serde_json::json!({"type": "adaptive", "display": "low"}),
        ReasoningEffort::Medium => serde_json::json!({"type": "adaptive", "display": "medium"}),
        ReasoningEffort::High => serde_json::json!({"type": "adaptive", "display": "high"}),
    };

    let (system, messages) = translate_messages(request, tools)?;

    let tools: Vec<WireTool> = request
        .tools
        .iter()
        .map(|tool| WireTool {
            name: tool.name.clone(),
            description: Some(tool.description.clone()),
            input_schema: tool.input_schema.clone(),
        })
        .collect();

    Ok(WireRequest {
        model: request.model.clone(),
        max_tokens: request.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        messages,
        system: (!system.is_empty()).then_some(system),
        tools: (!tools.is_empty()).then_some(tools),
        thinking: Some(thinking),
        stream: true,
    })
}

/// Translates the canonical message list into Anthropic wire messages and
/// system blocks.
fn translate_messages(
    request: &crate::model::types::ModelRequest,
    tools: &ValidatedTools,
) -> Result<(Vec<WireTextBlock>, Vec<WireRequestMessage>), ModelError> {
    let mut system: Vec<WireTextBlock> = Vec::new();
    let mut messages: Vec<WireRequestMessage> = Vec::new();
    let mut pending_tool_results: Vec<serde_json::Value> = Vec::new();

    let continuation = match &request.continuation {
        Some(ProviderContinuationState::Anthropic(continuation)) => Some(continuation),
        _ => None,
    };

    for (position, block) in request.messages.iter().enumerate() {
        match block {
            MessageBlock::System(system_block) => {
                for text in &system_block.content {
                    system.push(WireTextBlock {
                        r#type: "text",
                        text: text.text.clone(),
                    });
                }
            }
            MessageBlock::User(user) => {
                flush_tool_results(&mut pending_tool_results, &mut messages);
                let mut content = Vec::new();
                for user_content in &user.content {
                    match user_content {
                        UserContentBlock::Text(text) => {
                            content.push(serde_json::json!({
                                "type": "text",
                                "text": text.text,
                            }));
                        }
                        UserContentBlock::Image(_) | UserContentBlock::File(_) => {
                            return Err(unsupported(
                                "Anthropic cannot represent canonical image/file references                                  without artifact resolution",
                            ));
                        }
                    }
                }
                messages.push(WireRequestMessage {
                    role: "user",
                    content,
                });
            }
            MessageBlock::Agent(agent) => {
                flush_tool_results(&mut pending_tool_results, &mut messages);
                let is_last_agent = request.messages[position + 1..]
                    .iter()
                    .all(|later| !matches!(later, MessageBlock::Agent(_)));
                let content = translate_agent_content(agent, tools, continuation, is_last_agent)?;
                messages.push(WireRequestMessage {
                    role: "assistant",
                    content,
                });
            }
            MessageBlock::Tool(tool_message) => {
                pending_tool_results.push(translate_tool_result(tool_message)?);
            }
        }
    }
    flush_tool_results(&mut pending_tool_results, &mut messages);

    if messages.is_empty() {
        return Err(ModelError {
            kind: ModelErrorKind::InvalidRequest,
            message: "an Anthropic Messages request requires at least one message".to_owned(),
            retry_after_ms: None,
            provider_code: None,
        });
    }
    Ok((system, messages))
}

/// Translates one agent message into Anthropic assistant content blocks.
///
/// Previous thinking blocks are replayed from their rustX-owned opaque
/// provider state; a signed thinking block is never reconstructed from
/// canonical text alone and no signature is ever fabricated.
fn translate_agent_content(
    agent: &crate::message::types::AgentMessageBlock,
    tools: &ValidatedTools,
    continuation: Option<&AnthropicContinuation>,
    is_last_agent: bool,
) -> Result<Vec<serde_json::Value>, ModelError> {
    let mut content = Vec::new();
    let mut reasoning_seen = false;
    let last_reasoning_position = agent
        .content
        .iter()
        .rposition(|block| matches!(block, AgentContentBlock::Reasoning(_)));
    for (position, block) in agent.content.iter().enumerate() {
        match block {
            AgentContentBlock::Text(text) => {
                content.push(serde_json::json!({
                    "type": "text",
                    "text": text.text,
                }));
            }
            AgentContentBlock::Reasoning(reasoning) => {
                reasoning_seen = true;
                let is_boundary =
                    is_last_agent && Some(position) == last_reasoning_position;
                let state = match &reasoning.provider_state {
                    Some(ProviderContinuationState::Anthropic(state)) => {
                        if is_boundary {
                            if let Some(continuation) = continuation {
                                if continuation.opaque != state.opaque {
                                    return Err(invalid_request(
                                        "request continuation state contradicts the provider \
                                         state of the boundary reasoning block",
                                    ));
                                }
                            }
                        }
                        state
                    }
                    Some(ProviderContinuationState::OpenAiResponses(_)) => {
                        return Err(unsupported(
                            "foreign OpenAI Responses continuation state cannot be translated \
                             into Anthropic thinking",
                        ));
                    }
                    None => {
                        if is_boundary {
                            if let Some(continuation) = continuation {
                                // The runtime carried the continuation state
                                // separately; attach it to the boundary block.
                                continuation
                            } else {
                                return Err(unsupported(
                                    "previous Anthropic thinking block has no provider \
                                     continuation state; a signed thinking block cannot be \
                                     reconstructed from canonical text",
                                ));
                            }
                        } else {
                            return Err(unsupported(
                                "previous Anthropic thinking block has no provider \
                                 continuation state; a signed thinking block cannot be \
                                 reconstructed from canonical text",
                            ));
                        }
                    }
                };
                content.push(state.opaque.clone());
            }
            AgentContentBlock::ToolCall(call) => {
                let _ = resolve_tool(tools, &call.name)?;
                content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments,
                }));
            }
            AgentContentBlock::Refusal(_) => {
                return Err(unsupported(
                    "Anthropic cannot represent a previous refusal block; refusing to flatten \
                     refusal into text",
                ));
            }
            AgentContentBlock::Image(_) => {
                return Err(unsupported(
                    "Anthropic cannot represent generated image references",
                ));
            }
        }
    }
    if continuation.is_some() && !reasoning_seen && is_last_agent {
        return Err(invalid_request(
            "continuation state has no coherent preceding agent reasoning boundary",
        ));
    }
    Ok(content)
}

fn translate_tool_result(
    tool: &crate::message::types::ToolMessageBlock,
) -> Result<serde_json::Value, ModelError> {
    let mut content = Vec::new();
    for result_content in &tool.result.content {
        match result_content {
            crate::tools::types::ToolResultContent::Text(text) => {
                content.push(serde_json::json!({
                    "type": "text",
                    "text": text.text,
                }));
            }
            crate::tools::types::ToolResultContent::Json { value } => {
                content.push(serde_json::json!({
                    "type": "text",
                    "text": serde_json::to_string(value).map_err(|e| {
                        unsupported(&format!("tool JSON result is not serializable: {e}"))
                    })?,
                }));
            }
            crate::tools::types::ToolResultContent::File(_)
            | crate::tools::types::ToolResultContent::Image(_) => {
                return Err(unsupported(
                    "Anthropic cannot represent file/image tool results",
                ));
            }
        }
    }
    Ok(serde_json::json!({
        "type": "tool_result",
        "tool_use_id": tool.tool_call_id,
        "content": content,
    }))
}

/// Anthropic requires `tool_result` blocks to form one user message directly
/// after the assistant message; consecutive canonical tool results merge into
/// a single provider user message.
fn flush_tool_results(
    pending: &mut Vec<serde_json::Value>,
    messages: &mut Vec<WireRequestMessage>,
) {
    if pending.is_empty() {
        return;
    }
    let content = std::mem::take(pending);
    messages.push(WireRequestMessage {
        role: "user",
        content,
    });
}

fn invalid_request(message: &str) -> ModelError {
    ModelError {
        kind: ModelErrorKind::InvalidRequest,
        message: message.to_owned(),
        retry_after_ms: None,
        provider_code: None,
    }
}

fn unsupported(message: &str) -> ModelError {
    ModelError {
        kind: ModelErrorKind::Unsupported,
        message: message.to_owned(),
        retry_after_ms: None,
        provider_code: None,
    }
}

/// Tool name resolution kept available to the messages module.
pub(crate) fn resolve_tool(
    tools: &ValidatedTools,
    name: &str,
) -> Result<ToolId, ModelError> {
    tools.resolve(name).cloned().ok_or_else(|| {
        invalid_request(&format!("model called unknown tool name {name:?}"))
    })
}

#[cfg(test)]
mod tests {
    use super::map_finish_reason;
    use crate::model::finish::ModelFinishReason;

    #[test]
    fn finish_reason_mapping_is_exhaustive() {
        for (raw, expected) in [
            (Some("end_turn"), ModelFinishReason::Stop),
            (Some("stop_sequence"), ModelFinishReason::Stop),
            (Some("tool_use"), ModelFinishReason::ToolCalls),
            (Some("max_tokens"), ModelFinishReason::Length),
            (Some("model_context_window_exceeded"), ModelFinishReason::Length),
            (Some("refusal"), ModelFinishReason::Refusal),
            (
                Some("pause_turn"),
                ModelFinishReason::Other {
                    reason: "pause_turn".to_owned(),
                },
            ),
            (
                Some("future_reason"),
                ModelFinishReason::Other {
                    reason: "future_reason".to_owned(),
                },
            ),
            (
                None,
                ModelFinishReason::Other {
                    reason: "unknown".to_owned(),
                },
            ),
        ] {
            assert_eq!(map_finish_reason(raw), expected);
        }
    }

    #[test]
    fn pause_turn_is_not_an_ordinary_stop() {
        assert!(matches!(
            map_finish_reason(Some("pause_turn")),
            ModelFinishReason::Other { .. }
        ));
    }

    #[test]
    fn model_context_window_exceeded_is_length() {
        assert_eq!(
            map_finish_reason(Some("model_context_window_exceeded")),
            ModelFinishReason::Length
        );
    }

    /// Cumulative usage snapshots are combined, not summed.
    #[test]
    fn usage_combines_cumulative_snapshots() {
        use super::{normalize_usage, WireUsage};
        let start = WireUsage {
            input_tokens: Some(100),
            output_tokens: Some(1),
            cache_read_input_tokens: Some(10),
            cache_creation_input_tokens: Some(0),
        };
        let delta = WireUsage {
            input_tokens: Some(100),
            output_tokens: Some(42),
            cache_read_input_tokens: Some(12),
            cache_creation_input_tokens: Some(0),
        };
        let usage = normalize_usage(Some(&start), Some(&delta));
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 42);
        assert_eq!(usage.total_tokens, 142);
        assert_eq!(
            usage.details.as_ref().and_then(|d| d.cached_input_tokens),
            Some(12)
        );
    }
}
