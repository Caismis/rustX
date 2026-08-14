//! `OpenAI` normalization helpers shared by the Chat Completions and Responses
//! adapters: errors, usage, finish reasons, and tool resolution.

use async_openai::error::OpenAIError;
use async_openai::types::chat::CompletionUsage;

#[cfg(test)]
use async_openai::types::chat::FinishReason;

use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::finish::ModelFinishReason;
use crate::model::types::{ModelUsage, UsageDetails};
use crate::runtime::identity::ToolId;

use super::client::http_failure_of;

/// Normalizes an SDK error into a runtime-owned [`ModelError`].
///
/// HTTP failures captured by the no-retry transport carry the status and
/// `Retry-After` header; everything else maps by SDK variant.
pub(crate) fn normalize_error(error: OpenAIError) -> ModelError {
    if let Some(failure) = http_failure_of(&error) {
        return match failure.status.as_u16() {
            400 => context_or_invalid(&failure.message, failure.provider_code.as_deref()),
            401 | 403 => auth(failure),
            404 => invalid_or_unsupported(failure),
            408 | 409 => timeout(failure),
            429 => rate_limit(failure),
            _ => provider_error(failure),
        };
    }
    match error {
        OpenAIError::Reqwest(reqwest_error) => {
            let message = reqwest_error.to_string();
            let kind = if reqwest_error.is_timeout() {
                ModelErrorKind::Timeout
            } else {
                ModelErrorKind::Transport
            };
            ModelError {
                kind,
                message,
                retry_after_ms: None,
                provider_code: None,
            }
        }
        OpenAIError::ApiError(api_error) => {
            let message = api_error.api_error.message.clone();
            let provider_code = api_error
                .api_error
                .code
                .clone()
                .or_else(|| api_error.api_error.r#type.clone());
            let kind = match api_error.status_code.as_u16() {
                400 => context_or_invalid_kind(&message),
                401 | 403 => ModelErrorKind::Authentication,
                429 => ModelErrorKind::RateLimit,
                408 | 409 => ModelErrorKind::Timeout,
                _ => ModelErrorKind::ProviderError,
            };
            ModelError {
                kind,
                message,
                retry_after_ms: None,
                provider_code,
            }
        }
        OpenAIError::InvalidArgument(message) => ModelError {
            kind: ModelErrorKind::InvalidRequest,
            message,
            retry_after_ms: None,
            provider_code: None,
        },
        OpenAIError::StreamError(stream_error) => ModelError {
            kind: ModelErrorKind::ProviderError,
            message: stream_error.to_string(),
            retry_after_ms: None,
            provider_code: None,
        },
        OpenAIError::Boxed(boxed) => ModelError {
            kind: ModelErrorKind::ProviderError,
            message: boxed.to_string(),
            retry_after_ms: None,
            provider_code: None,
        },
        OpenAIError::JSONDeserialize(_, content) => {
            if content == "[DONE]" {
                ModelError {
                    kind: ModelErrorKind::ProviderError,
                    message: "stream terminated by [DONE]".to_owned(),
                    retry_after_ms: None,
                    provider_code: None,
                }
            } else {
                ModelError {
                    kind: ModelErrorKind::ProviderError,
                    message: format!("malformed provider stream payload: {content}"),
                    retry_after_ms: None,
                    provider_code: None,
                }
            }
        }
        OpenAIError::FileSaveError(message) | OpenAIError::FileReadError(message) => ModelError {
            kind: ModelErrorKind::Transport,
            message,
            retry_after_ms: None,
            provider_code: None,
        },
    }
}

/// Whether an `OpenAI` error message denotes a context-window violation.
pub(crate) fn is_context_window_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("context_length_exceeded")
        || lower.contains("maximum context length")
        || lower.contains("context length exceeded")
        || lower.contains("context window")
}

fn context_or_invalid(message: &str, provider_code: Option<&str>) -> ModelError {
    ModelError {
        kind: if is_context_window_message(message) {
            ModelErrorKind::ContextWindowExceeded
        } else {
            ModelErrorKind::InvalidRequest
        },
        message: message.to_owned(),
        retry_after_ms: None,
        provider_code: provider_code.map(str::to_owned),
    }
}

fn context_or_invalid_kind(message: &str) -> ModelErrorKind {
    if is_context_window_message(message) {
        ModelErrorKind::ContextWindowExceeded
    } else {
        ModelErrorKind::InvalidRequest
    }
}

fn auth(failure: &super::client::HttpFailure) -> ModelError {
    ModelError {
        kind: ModelErrorKind::Authentication,
        message: failure.message.clone(),
        retry_after_ms: failure.retry_after_ms,
        provider_code: failure.provider_code.clone(),
    }
}

fn invalid_or_unsupported(failure: &super::client::HttpFailure) -> ModelError {
    ModelError {
        kind: ModelErrorKind::InvalidRequest,
        message: failure.message.clone(),
        retry_after_ms: failure.retry_after_ms,
        provider_code: failure.provider_code.clone(),
    }
}

fn timeout(failure: &super::client::HttpFailure) -> ModelError {
    ModelError {
        kind: ModelErrorKind::Timeout,
        message: failure.message.clone(),
        retry_after_ms: failure.retry_after_ms,
        provider_code: failure.provider_code.clone(),
    }
}

fn rate_limit(failure: &super::client::HttpFailure) -> ModelError {
    ModelError {
        kind: ModelErrorKind::RateLimit,
        message: failure.message.clone(),
        retry_after_ms: failure.retry_after_ms,
        provider_code: failure.provider_code.clone(),
    }
}

fn provider_error(failure: &super::client::HttpFailure) -> ModelError {
    ModelError {
        kind: ModelErrorKind::ProviderError,
        message: failure.message.clone(),
        retry_after_ms: failure.retry_after_ms,
        provider_code: failure.provider_code.clone(),
    }
}

/// Maps a Chat Completions finish reason.
pub(crate) fn map_chat_finish_reason(reason: Option<&str>) -> ModelFinishReason {
    match reason {
        Some("stop") => ModelFinishReason::Stop,
        Some("tool_calls") => ModelFinishReason::ToolCalls,
        Some("length") => ModelFinishReason::Length,
        Some("content_filter" | "sensitive") => ModelFinishReason::ContentFilter,
        Some("function_call") => ModelFinishReason::Other {
            reason: "function_call".to_owned(),
        },
        Some(other) => ModelFinishReason::Other {
            reason: other.to_owned(),
        },
        None => ModelFinishReason::Other {
            reason: "unknown".to_owned(),
        },
    }
}

/// Maps an SDK Chat Completions finish reason (used where the typed SDK value
/// is already in hand, for example in unit tests).
#[cfg(test)]
fn map_sdk_chat_finish_reason(reason: FinishReason) -> ModelFinishReason {
    match reason {
        FinishReason::Stop => ModelFinishReason::Stop,
        FinishReason::Length => ModelFinishReason::Length,
        FinishReason::ToolCalls => ModelFinishReason::ToolCalls,
        FinishReason::ContentFilter => ModelFinishReason::ContentFilter,
        FinishReason::FunctionCall => ModelFinishReason::Other {
            reason: "function_call".to_owned(),
        },
    }
}

/// Normalizes Chat Completions usage.
pub(crate) fn normalize_chat_usage(usage: &CompletionUsage) -> ModelUsage {
    let input_tokens = u64::from(usage.prompt_tokens);
    let output_tokens = u64::from(usage.completion_tokens);
    ModelUsage {
        input_tokens,
        output_tokens,
        total_tokens: u64::from(usage.total_tokens),
        details: usage_details(
            usage
                .completion_tokens_details
                .as_ref()
                .and_then(|d| d.reasoning_tokens),
            usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|d| d.cached_tokens),
        ),
    }
}

/// Builds optional usage details, mapping only semantically equivalent
/// provider metrics into the canonical fields.
fn usage_details(
    reasoning_tokens: Option<u32>,
    cached_input_tokens: Option<u32>,
) -> Option<UsageDetails> {
    let details = UsageDetails {
        reasoning_tokens: reasoning_tokens.map(u64::from),
        cached_input_tokens: cached_input_tokens.map(u64::from),
    };
    (details.reasoning_tokens.is_some() || details.cached_input_tokens.is_some()).then_some(details)
}

/// Resolves a provider function name to the canonical tool identity, failing
/// explicitly when the name is unknown.
pub(crate) fn resolve_tool(
    tools: &crate::model::adapter::validation::ValidatedTools,
    name: &str,
) -> Result<ToolId, ModelError> {
    match tools.resolve(name) {
        Some(tool_id) => Ok(tool_id.clone()),
        None => Err(ModelError {
            kind: ModelErrorKind::InvalidRequest,
            message: format!("model called unknown tool name {name:?}"),
            retry_after_ms: None,
            provider_code: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_context_window_message, map_chat_finish_reason, map_sdk_chat_finish_reason,
        normalize_error,
    };
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::finish::ModelFinishReason;
    use async_openai::types::chat::FinishReason;

    #[test]
    fn chat_finish_reason_mapping_is_exhaustive() {
        for (raw, expected) in [
            (Some("stop"), ModelFinishReason::Stop),
            (Some("tool_calls"), ModelFinishReason::ToolCalls),
            (Some("length"), ModelFinishReason::Length),
            (Some("content_filter"), ModelFinishReason::ContentFilter),
            (Some("sensitive"), ModelFinishReason::ContentFilter),
            (
                Some("function_call"),
                ModelFinishReason::Other {
                    reason: "function_call".to_owned(),
                },
            ),
            (
                Some("mystery_reason"),
                ModelFinishReason::Other {
                    reason: "mystery_reason".to_owned(),
                },
            ),
            (
                None,
                ModelFinishReason::Other {
                    reason: "unknown".to_owned(),
                },
            ),
        ] {
            assert_eq!(map_chat_finish_reason(raw), expected);
        }
        for (raw, expected) in [
            (FinishReason::Stop, ModelFinishReason::Stop),
            (FinishReason::ToolCalls, ModelFinishReason::ToolCalls),
            (FinishReason::Length, ModelFinishReason::Length),
            (
                FinishReason::ContentFilter,
                ModelFinishReason::ContentFilter,
            ),
            (
                FinishReason::FunctionCall,
                ModelFinishReason::Other {
                    reason: "function_call".to_owned(),
                },
            ),
        ] {
            assert_eq!(map_sdk_chat_finish_reason(raw), expected);
        }
    }

    #[test]
    fn context_window_messages_are_detected() {
        for message in [
            "This model's maximum context length is 128000 tokens...",
            "context_length_exceeded: you can retrieve...",
            "context window is full",
        ] {
            assert!(is_context_window_message(message), "{message}");
        }
        for message in ["invalid api key", "missing required field"] {
            assert!(!is_context_window_message(message), "{message}");
        }
    }

    #[test]
    fn sdk_api_errors_normalize_by_status() {
        let error = async_openai::error::OpenAIError::InvalidArgument("bad builder".to_owned());
        let normalized = normalize_error(error);
        assert_eq!(normalized.kind, ModelErrorKind::InvalidRequest);
        assert!(normalized.message.contains("bad builder"));
    }

    #[tokio::test]
    async fn transport_errors_map_to_transport_or_timeout() {
        let error = async_openai::error::OpenAIError::Reqwest(
            reqwest::Client::new()
                .get("http://127.0.0.1:1/")
                .send()
                .await
                .expect_err("connection must fail"),
        );
        let normalized = normalize_error(error);
        assert_eq!(normalized.kind, ModelErrorKind::Transport);
        let _: &ModelError = &normalized;
    }
}
