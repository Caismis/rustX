//! Anthropic Messages normalization: errors, usage, finish reasons, and
//! canonical request translation.

use reqwest::{StatusCode, header::HeaderMap};

use crate::message::types::{AssistantContentBlock, MessageBlock, UserContentBlock};
use crate::model::adapter::validation::ValidatedTools;
use crate::model::error::{
    ModelError, ModelErrorKind, ModelRetryDisposition, is_context_window_error,
};
use crate::model::finish::ModelFinishReason;
use crate::model::invocation::finalize_provider_request;
use crate::model::types::{ModelProtocol, ModelUsage, UsageDetails};
use crate::runtime::continuation::{AnthropicContinuation, ProviderContinuationState};
use crate::runtime::identity::ToolId;

use super::wire::WireUsage;

/// Maps an Anthropic stop reason to the canonical finish reason.
pub(crate) fn map_finish_reason(stop_reason: Option<&str>) -> ModelFinishReason {
    match stop_reason {
        Some("end_turn" | "stop_sequence") => ModelFinishReason::Stop,
        Some("tool_use") => ModelFinishReason::ToolCalls,
        Some("max_tokens") => ModelFinishReason::Length,
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
/// `message_delta` snapshot without summing snapshots over time.
///
/// Canonical effective input consumption accounts for every provider
/// input-token category that contributes to total provider input:
///
/// ```text
/// canonical input_tokens
///     = input_tokens
///       + cache_creation_input_tokens
///       + cache_read_input_tokens
/// ```
///
/// where reported, per the current Messages API usage semantics.
/// `cache_read_input_tokens` is additionally reported as
/// [`UsageDetails::cached_input_tokens`]; it is never double counted.
pub(crate) fn normalize_usage(
    message_start_usage: Option<&WireUsage>,
    latest_delta_usage: Option<&WireUsage>,
) -> ModelUsage {
    let input_base = latest_delta_usage
        .and_then(|u| u.input_tokens)
        .or_else(|| message_start_usage.and_then(|u| u.input_tokens))
        .unwrap_or(0);
    let cache_creation = latest_delta_usage
        .and_then(|u| u.cache_creation_input_tokens)
        .or_else(|| message_start_usage.and_then(|u| u.cache_creation_input_tokens))
        .unwrap_or(0);
    let cache_read = latest_delta_usage
        .and_then(|u| u.cache_read_input_tokens)
        .or_else(|| message_start_usage.and_then(|u| u.cache_read_input_tokens));
    let output_tokens = latest_delta_usage
        .and_then(|u| u.output_tokens)
        .or_else(|| message_start_usage.and_then(|u| u.output_tokens))
        .unwrap_or(0);
    let reasoning_tokens = latest_delta_usage
        .and_then(|u| {
            u.output_tokens_details
                .as_ref()
                .and_then(|d| d.thinking_tokens)
        })
        .or_else(|| {
            message_start_usage
                .and_then(|u| u.output_tokens_details.as_ref())
                .and_then(|d| d.thinking_tokens)
        });
    let input_tokens = input_base + cache_creation + cache_read.unwrap_or(0);
    let details = (reasoning_tokens.is_some() || cache_read.is_some()).then_some(UsageDetails {
        reasoning_tokens,
        cached_input_tokens: cache_read,
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
        400 | 413 => {
            if is_context_window_error(&message, provider_code.as_deref()) {
                ModelErrorKind::ContextWindowExceeded
            } else {
                ModelErrorKind::InvalidRequest
            }
        }
        401 | 403 => ModelErrorKind::Authentication,
        404 => ModelErrorKind::InvalidRequest,
        408 => ModelErrorKind::Timeout,
        409 => ModelErrorKind::ProviderError,
        429 => ModelErrorKind::RateLimit,
        _ if is_context_window_error(&message, provider_code.as_deref()) => {
            ModelErrorKind::ContextWindowExceeded
        }
        _ => ModelErrorKind::ProviderError,
    };
    let retry_disposition =
        http_retry_disposition(status, provider_code.as_deref(), &message, &kind);
    ModelError {
        kind,
        message,
        retry_disposition,
        retry_after_ms,
        provider_code,
        context_overflow: None,
    }
    .normalized()
}

fn http_retry_disposition(
    status: StatusCode,
    provider_code: Option<&str>,
    message: &str,
    kind: &ModelErrorKind,
) -> ModelRetryDisposition {
    match kind {
        ModelErrorKind::RateLimit => {
            if is_permanent_quota(provider_code, message) || status != StatusCode::TOO_MANY_REQUESTS
            {
                ModelRetryDisposition::Never
            } else {
                ModelRetryDisposition::Transient
            }
        }
        ModelErrorKind::Timeout => (status == StatusCode::REQUEST_TIMEOUT)
            .then_some(())
            .map_or(ModelRetryDisposition::Never, |()| {
                ModelRetryDisposition::Transient
            }),
        ModelErrorKind::ProviderError => {
            if !is_permanent_quota(provider_code, message)
                && (retryable_server_status(status) || explicitly_retryable_code(provider_code))
            {
                ModelRetryDisposition::Transient
            } else {
                ModelRetryDisposition::Never
            }
        }
        ModelErrorKind::InvalidRequest
        | ModelErrorKind::Authentication
        | ModelErrorKind::Unsupported
        | ModelErrorKind::ContextWindowExceeded
        | ModelErrorKind::Cancelled
        | ModelErrorKind::Transport => ModelRetryDisposition::Never,
    }
}

fn retryable_server_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 500 | 502 | 503 | 504 | 529)
}

fn explicitly_retryable_code(code: Option<&str>) -> bool {
    code.map(str::to_ascii_lowercase).is_some_and(|code| {
        matches!(
            code.as_str(),
            "rate_limit_error"
                | "rate_limit_exceeded"
                | "overloaded_error"
                | "server_error"
                | "internal_server_error"
                | "service_unavailable"
                | "temporarily_unavailable"
                | "upstream_error"
        )
    })
}

fn is_permanent_quota(code: Option<&str>, message: &str) -> bool {
    let code = code.map(str::to_ascii_lowercase);
    let message = message.to_ascii_lowercase();
    code.as_deref().is_some_and(|code| {
        matches!(
            code,
            "insufficient_quota"
                | "quota_exceeded"
                | "billing_hard_limit_reached"
                | "billing_not_active"
                | "payment_required"
        )
    }) || message.contains("insufficient quota")
        || message.contains("quota exceeded")
        || message.contains("billing")
        || message.contains("payment required")
}

/// Classifies structured Anthropic stream error evidence. A wire error that
/// merely resembles cancellation is not runtime-owned cancellation and is
/// never converted to `ModelErrorKind::Cancelled` by this helper.
pub(crate) fn stream_retry_disposition(
    error_type: Option<&str>,
    message: Option<&str>,
) -> ModelRetryDisposition {
    let message = message.unwrap_or_default();
    if is_permanent_quota(error_type, message) {
        return ModelRetryDisposition::Never;
    }
    if matches!(error_type, Some("rate_limit_error" | "rate_limit_exceeded")) {
        return ModelRetryDisposition::Transient;
    }
    if explicitly_retryable_code(error_type) {
        return ModelRetryDisposition::Transient;
    }
    ModelRetryDisposition::Never
}

fn parse_error_body(body: &[u8]) -> (Option<String>, Option<String>) {
    let Ok(root) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (None, None);
    };
    let detail = root
        .get("error")
        .filter(|error| error.is_object())
        .unwrap_or(&root);
    let provider_code = detail
        .get("code")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            detail
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            detail
                .get("code")
                .and_then(serde_json::Value::as_u64)
                .map(|code| code.to_string())
        });
    let message = detail
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    (provider_code, message)
}

/// The runtime-owned structural part of the request body sent to
/// `/v1/messages`.
///
/// Only fields rustX owns semantically appear here. Provider sampling and
/// reasoning fields (`thinking`, `output_config`, `temperature`, …) are
/// *not* modelled: they arrive as the request's opaque effective
/// `requestParams` and are shallow-overlaid onto the serialized object.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct WireRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<WireRequestMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<WireTextBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<WireTool>>,
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

/// Translates a canonical request into the final Anthropic request JSON.
///
/// Canonical translation produces the typed runtime-owned [`WireRequest`];
/// it is then serialized to a JSON object and shallow-overlaid with the
/// request's effective opaque `requestParams` under the protected-key
/// contract. No `thinking` or `output_config` field is ever synthesized:
/// whatever the selected reasoning profile configured is exactly what
/// reaches the wire, and a model whose profile configures nothing sends
/// nothing.
pub(crate) fn translate_request(
    request: &crate::model::types::ModelRequest,
    tools: &ValidatedTools,
) -> Result<serde_json::Value, ModelError> {
    let (system, messages) = translate_messages(request, tools)?;

    if !request.tools.is_empty() && !request.invocation.capabilities.tool_calls {
        return Err(ModelError {
            kind: ModelErrorKind::Unsupported,
            message: "the effective model capabilities do not include tool calls; \
                      tool definitions are never sent to a text-only model"
                .to_owned(),
            retry_disposition: crate::model::error::ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
        });
    }
    let tools = translate_tools(&request.tools);

    let wire = WireRequest {
        model: request.model().to_owned(),
        max_tokens: request.max_output_tokens(),
        messages,
        system: (!system.is_empty()).then_some(system),
        tools: (!tools.is_empty()).then_some(tools),
        stream: true,
    };
    let value = serde_json::to_value(&wire).map_err(|error| ModelError {
        kind: ModelErrorKind::InvalidRequest,
        message: format!("failed to serialize the Anthropic request: {error}"),
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
    })?;
    finalize_provider_request(
        value,
        request.request_params(),
        ModelProtocol::AnthropicMessages,
    )
}

fn translate_tools(tools: &[crate::tools::types::ModelToolDefinition]) -> Vec<WireTool> {
    tools
        .iter()
        .map(|tool| WireTool {
            name: tool.name.clone(),
            description: Some(tool.description.clone()),
            input_schema: tool.input_schema.clone(),
        })
        .collect()
}

/// Translates the canonical message list into Anthropic wire messages and
/// system blocks.
fn translate_messages(
    request: &crate::model::types::ModelRequest,
    tools: &ValidatedTools,
) -> Result<(Vec<WireTextBlock>, Vec<WireRequestMessage>), ModelError> {
    let system: Vec<WireTextBlock> = if request.effective_system_prompt.is_empty() {
        Vec::new()
    } else {
        vec![WireTextBlock {
            r#type: "text",
            text: request.effective_system_prompt.clone(),
        }]
    };
    let mut messages: Vec<WireRequestMessage> = Vec::new();
    let mut pending_tool_results: Vec<serde_json::Value> = Vec::new();

    let continuation = match &request.continuation {
        Some(ProviderContinuationState::Anthropic(continuation)) => Some(continuation),
        _ => None,
    };

    for (position, block) in request.messages.iter().enumerate() {
        match block {
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
            MessageBlock::Assistant(assistant) => {
                flush_tool_results(&mut pending_tool_results, &mut messages);
                let is_last_assistant = request.messages[position + 1..]
                    .iter()
                    .all(|later| !matches!(later, MessageBlock::Assistant(_)));
                let content =
                    translate_assistant_content(assistant, tools, continuation, is_last_assistant)?;
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
            retry_disposition: crate::model::error::ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
        });
    }
    Ok((system, messages))
}

/// Translates one canonical Assistant message into Anthropic assistant
/// content blocks.
///
/// Previous thinking blocks are replayed from their rustX-owned opaque
/// provider state; a signed thinking block is never reconstructed from
/// canonical text alone and no signature is ever fabricated.
fn translate_assistant_content(
    assistant: &crate::message::types::AssistantMessageBlock,
    tools: &ValidatedTools,
    continuation: Option<&AnthropicContinuation>,
    is_last_assistant: bool,
) -> Result<Vec<serde_json::Value>, ModelError> {
    let mut content = Vec::new();
    let mut reasoning_seen = false;
    let last_reasoning_position = assistant
        .content
        .iter()
        .rposition(|block| matches!(block, AssistantContentBlock::Reasoning(_)));
    for (position, block) in assistant.content.iter().enumerate() {
        match block {
            AssistantContentBlock::Text(text) => {
                content.push(serde_json::json!({
                    "type": "text",
                    "text": text.text,
                }));
            }
            AssistantContentBlock::Reasoning(reasoning) => {
                reasoning_seen = true;
                let is_boundary = is_last_assistant && Some(position) == last_reasoning_position;
                let state = match &reasoning.provider_state {
                    Some(ProviderContinuationState::Anthropic(state)) => {
                        if is_boundary
                            && let Some(continuation) = continuation
                            && continuation.opaque != state.opaque
                        {
                            return Err(invalid_request(
                                "request continuation state contradicts the provider \
                                 state of the boundary reasoning block",
                            ));
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
            AssistantContentBlock::ToolCall(call) => {
                let _ = resolve_tool(tools, &call.name)?;
                content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments,
                }));
            }
            AssistantContentBlock::Refusal(_) => {
                return Err(unsupported(
                    "Anthropic cannot represent a previous refusal block; refusing to flatten \
                     refusal into text",
                ));
            }
            AssistantContentBlock::Image(_) => {
                return Err(unsupported(
                    "Anthropic cannot represent generated image references",
                ));
            }
        }
    }
    if continuation.is_some() && !reasoning_seen && is_last_assistant {
        return Err(invalid_request(
            "continuation state has no coherent preceding Assistant reasoning boundary",
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
    if let Some(status) = tool.result.status.model_facing_text() {
        content.push(serde_json::json!({
            "type": "text",
            "text": status,
        }));
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
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
    }
}

fn unsupported(message: &str) -> ModelError {
    ModelError {
        kind: ModelErrorKind::Unsupported,
        message: message.to_owned(),
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
    }
}

/// Tool name resolution kept available to the messages module.
pub(crate) fn resolve_tool(tools: &ValidatedTools, name: &str) -> Result<ToolId, ModelError> {
    tools
        .resolve(name)
        .cloned()
        .ok_or_else(|| invalid_request(&format!("model called unknown tool name {name:?}")))
}

#[cfg(test)]
mod questionnaire_schema_tests {
    use super::translate_tools;
    use crate::tools::types::ModelToolDefinition;
    use serde_json::json;

    #[test]
    fn anthropic_messages_preserves_the_nested_questionnaire_schema() {
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
        let encoded = translate_tools(&[ModelToolDefinition {
            id: crate::runtime::identity::ToolId::new("tool-ask-user"),
            name: "ask_user".to_owned(),
            description: "structured questionnaire".to_owned(),
            input_schema: schema.clone(),
        }]);
        assert_eq!(encoded[0].name, "ask_user");
        assert_eq!(encoded[0].input_schema, schema);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        map_finish_reason, normalize_http_error, stream_retry_disposition, translate_tool_result,
    };
    use crate::message::types::ToolMessageBlock;
    use crate::model::error::{ModelErrorKind, ModelRetryDisposition};
    use crate::model::finish::ModelFinishReason;
    use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
    use crate::runtime::types::CancellationReason;
    use crate::tools::types::{ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus};

    /// The adapter — not the agent loop — recovers the provider's own
    /// measurement of a rejected oversized request, and it crosses the
    /// model boundary as typed data.
    #[test]
    fn http_context_overflow_carries_the_typed_provider_measurement() {
        let body = br#"{"error":{"type":"invalid_request_error","message":"prompt is too long: 213462 tokens > 200000 maximum"}}"#;
        let error = normalize_http_error(
            reqwest::StatusCode::BAD_REQUEST,
            &reqwest::header::HeaderMap::new(),
            body,
        );
        assert_eq!(error.kind, ModelErrorKind::ContextWindowExceeded);
        let report = error.context_overflow.expect("typed overflow report");
        assert_eq!(report.reported_input_tokens, Some(213_462));
    }

    /// An error that is not a context overflow carries no overflow report,
    /// so no consumer can mistake an unrelated diagnostic for a budget.
    #[test]
    fn other_http_errors_carry_no_overflow_report() {
        let body =
            br#"{"error":{"type":"rate_limit_error","message":"slow down, 429000 requests"}}"#;
        let error = normalize_http_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &reqwest::header::HeaderMap::new(),
            body,
        );
        assert_eq!(error.kind, ModelErrorKind::RateLimit);
        assert_eq!(error.context_overflow, None);
    }

    #[test]
    fn provider_retry_classification_requires_retryable_evidence() {
        let headers = reqwest::header::HeaderMap::new();
        let server = normalize_http_error(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            &headers,
            br#"{"error":{"type":"api_error","message":"temporarily unavailable"}}"#,
        );
        assert_eq!(server.retry_disposition, ModelRetryDisposition::Transient);

        let conflict = normalize_http_error(
            reqwest::StatusCode::CONFLICT,
            &headers,
            br#"{"error":{"type":"api_error","message":"conflict"}}"#,
        );
        assert_eq!(conflict.retry_disposition, ModelRetryDisposition::Never);

        let throttled = normalize_http_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            br#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#,
        );
        assert_eq!(
            throttled.retry_disposition,
            ModelRetryDisposition::Transient
        );

        let quota = normalize_http_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            br#"{"error":{"type":"rate_limit_error","message":"billing hard limit reached"}}"#,
        );
        assert_eq!(quota.retry_disposition, ModelRetryDisposition::Never);

        let invalid = normalize_http_error(
            reqwest::StatusCode::BAD_REQUEST,
            &headers,
            br#"{"error":{"type":"invalid_request_error","message":"bad input"}}"#,
        );
        assert_eq!(invalid.retry_disposition, ModelRetryDisposition::Never);
    }

    #[test]
    fn issue136_anthropic_translation_consumes_typed_cancellation_status() {
        let message = ToolMessageBlock {
            id: MessageId::new("tool-result-1"),
            tool_call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-1"),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Cancelled {
                    reason: CancellationReason::ParentCancelled,
                    phase: ToolCancellationPhase::DuringExecution,
                },
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            },
        };
        let encoded = translate_tool_result(&message).expect("translate cancelled result");
        assert_eq!(
            encoded["content"][0]["text"],
            "Tool call was cancelled (reason: parent_cancelled). Execution had already started, but cancellation occurred before normal completion. Partial side effects may have occurred."
        );
    }

    #[test]
    fn provider_looking_cancellation_is_not_runtime_cancellation() {
        assert_eq!(
            stream_retry_disposition(Some("request_cancelled"), Some("cancelled")),
            ModelRetryDisposition::Never
        );
    }

    #[test]
    fn finish_reason_mapping_is_exhaustive() {
        for (raw, expected) in [
            (Some("end_turn"), ModelFinishReason::Stop),
            (Some("stop_sequence"), ModelFinishReason::Stop),
            (Some("tool_use"), ModelFinishReason::ToolCalls),
            (Some("max_tokens"), ModelFinishReason::Length),
            (
                Some("model_context_window_exceeded"),
                ModelFinishReason::Other {
                    reason: "model_context_window_exceeded".to_owned(),
                },
            ),
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
    fn model_context_window_exceeded_is_not_output_length() {
        assert!(matches!(
            map_finish_reason(Some("model_context_window_exceeded")),
            ModelFinishReason::Other { .. }
        ));
    }

    /// Cumulative usage snapshots are combined, not summed; effective input
    /// consumption includes every provider input-token category.
    #[test]
    fn usage_combines_cumulative_snapshots() {
        use super::{WireUsage, normalize_usage};
        use crate::model::adapter::anthropic::wire::WireOutputTokensDetails;
        let start = WireUsage {
            input_tokens: Some(100),
            output_tokens: Some(1),
            cache_read_input_tokens: Some(10),
            cache_creation_input_tokens: Some(0),
            output_tokens_details: None,
        };
        let delta = WireUsage {
            input_tokens: Some(100),
            output_tokens: Some(42),
            cache_read_input_tokens: Some(12),
            cache_creation_input_tokens: Some(8),
            output_tokens_details: Some(WireOutputTokensDetails {
                thinking_tokens: Some(7),
            }),
        };
        let usage = normalize_usage(Some(&start), Some(&delta));
        assert_eq!(usage.input_tokens, 120, "100 base + 8 creation + 12 read");
        assert_eq!(usage.output_tokens, 42);
        assert_eq!(usage.total_tokens, 162);
        let details = usage.details.expect("details present");
        assert_eq!(details.cached_input_tokens, Some(12));
        assert_eq!(details.reasoning_tokens, Some(7));
    }

    /// Cache categories are never double counted and are not summed across
    /// snapshots: the latest cumulative snapshot wins.
    #[test]
    fn usage_never_sums_snapshots() {
        use super::{WireUsage, normalize_usage};
        let start = WireUsage {
            input_tokens: Some(40),
            output_tokens: Some(1),
            cache_read_input_tokens: Some(4),
            cache_creation_input_tokens: Some(1),
            output_tokens_details: None,
        };
        let delta = WireUsage {
            input_tokens: Some(40),
            output_tokens: Some(9),
            cache_read_input_tokens: Some(5),
            cache_creation_input_tokens: Some(2),
            output_tokens_details: None,
        };
        let usage = normalize_usage(Some(&start), Some(&delta));
        assert_eq!(usage.input_tokens, 47, "40 base + 2 creation + 5 read");
        assert_eq!(usage.total_tokens, 56);
        assert_eq!(
            usage.details.as_ref().and_then(|d| d.cached_input_tokens),
            Some(5)
        );
    }

    /// Thinking-token details map when reported; absent counts are never
    /// invented.
    #[test]
    fn usage_maps_thinking_tokens_only_when_reported() {
        use super::{WireUsage, normalize_usage};
        let plain = WireUsage {
            input_tokens: Some(10),
            output_tokens: Some(3),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
            output_tokens_details: None,
        };
        let usage = normalize_usage(Some(&plain), None);
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.total_tokens, 13);
        assert!(usage.details.is_none(), "no details without provider data");
    }
}
