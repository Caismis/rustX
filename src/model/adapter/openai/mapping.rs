//! `OpenAI` normalization helpers shared by the Chat Completions and Responses
//! adapters: errors, usage, finish reasons, and tool resolution.

use async_openai::error::OpenAIError;
use async_openai::types::chat::CompletionUsage;

#[cfg(test)]
use async_openai::types::chat::FinishReason;

use crate::model::error::{
    ModelError, ModelErrorKind, ModelRetryDisposition, is_context_window_error,
};
use crate::model::finish::ModelFinishReason;
use crate::model::types::{ModelUsage, UsageDetails};

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
            408 => timeout(failure),
            409 => provider_error(failure),
            429 => rate_limit(failure),
            _ if is_context_window_error(&failure.message, failure.provider_code.as_deref()) => {
                context_window(failure)
            }
            _ => provider_error(failure),
        };
    }
    normalize_sdk_error(error)
}

/// Normalizes one SDK-variant error that carried no HTTP failure of its own.
#[allow(clippy::too_many_lines)] // preserve one exhaustive SDK error normalization boundary
fn normalize_sdk_error(error: OpenAIError) -> ModelError {
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
                retry_disposition: if reqwest_error.is_timeout() || reqwest_error.is_connect() {
                    crate::model::error::ModelRetryDisposition::Transient
                } else {
                    crate::model::error::ModelRetryDisposition::Never
                },
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
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
                400 => context_or_invalid_kind(&message, provider_code.as_deref()),
                401 | 403 => ModelErrorKind::Authentication,
                429 => ModelErrorKind::RateLimit,
                408 => ModelErrorKind::Timeout,
                409 => ModelErrorKind::ProviderError,
                _ if is_context_window_error(&message, provider_code.as_deref()) => {
                    ModelErrorKind::ContextWindowExceeded
                }
                _ => ModelErrorKind::ProviderError,
            };
            let retry_disposition = sdk_status_disposition(
                api_error.status_code.as_u16(),
                provider_code.as_deref(),
                &message,
                &kind,
            );
            ModelError {
                kind,
                message,
                retry_disposition,
                retry_after_ms: None,
                provider_code,
                context_overflow: None,
                malformed_tool_proposal: None,
            }
            .normalized()
        }
        OpenAIError::InvalidArgument(message) => ModelError {
            kind: ModelErrorKind::InvalidRequest,
            message,
            retry_disposition: crate::model::error::ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
        },
        OpenAIError::StreamError(stream_error) => ModelError {
            kind: ModelErrorKind::ProviderError,
            message: stream_error.to_string(),
            retry_disposition: match stream_error.as_ref() {
                async_openai::error::StreamError::EventStream(message)
                    if message.starts_with("Transport error:") =>
                {
                    crate::model::error::ModelRetryDisposition::Transient
                }
                _ => crate::model::error::ModelRetryDisposition::Never,
            },
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
        },
        OpenAIError::Boxed(boxed) => ModelError {
            kind: ModelErrorKind::ProviderError,
            message: boxed.to_string(),
            retry_disposition: boxed
                .downcast_ref::<reqwest::Error>()
                .filter(|error| error.is_timeout() || error.is_connect())
                .map_or(crate::model::error::ModelRetryDisposition::Never, |_| {
                    crate::model::error::ModelRetryDisposition::Transient
                }),
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
        },
        OpenAIError::JSONDeserialize(_, content) => {
            if content == "[DONE]" {
                ModelError {
                    kind: ModelErrorKind::ProviderError,
                    message: "stream terminated by [DONE]".to_owned(),
                    retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                    retry_after_ms: None,
                    provider_code: None,
                    context_overflow: None,
                    malformed_tool_proposal: None,
                }
            } else {
                ModelError {
                    kind: ModelErrorKind::ProviderError,
                    message: format!("malformed provider stream payload: {content}"),
                    retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                    retry_after_ms: None,
                    provider_code: None,
                    context_overflow: None,
                    malformed_tool_proposal: None,
                }
            }
        }
        OpenAIError::FileSaveError(message) | OpenAIError::FileReadError(message) => ModelError {
            kind: ModelErrorKind::Transport,
            message,
            retry_disposition: crate::model::error::ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
        },
    }
}

fn context_or_invalid(message: &str, provider_code: Option<&str>) -> ModelError {
    ModelError {
        kind: if is_context_window_error(message, provider_code) {
            ModelErrorKind::ContextWindowExceeded
        } else {
            ModelErrorKind::InvalidRequest
        },
        message: message.to_owned(),
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: provider_code.map(str::to_owned),
        context_overflow: None,
        malformed_tool_proposal: None,
    }
    .normalized()
}

fn context_or_invalid_kind(message: &str, provider_code: Option<&str>) -> ModelErrorKind {
    if is_context_window_error(message, provider_code) {
        ModelErrorKind::ContextWindowExceeded
    } else {
        ModelErrorKind::InvalidRequest
    }
}

fn context_window(failure: &super::client::HttpFailure) -> ModelError {
    ModelError {
        kind: ModelErrorKind::ContextWindowExceeded,
        message: failure.message.clone(),
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: failure.retry_after_ms,
        provider_code: failure.provider_code.clone(),
        context_overflow: None,
        malformed_tool_proposal: None,
    }
    .normalized()
}

fn auth(failure: &super::client::HttpFailure) -> ModelError {
    ModelError {
        kind: ModelErrorKind::Authentication,
        message: failure.message.clone(),
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: failure.retry_after_ms,
        provider_code: failure.provider_code.clone(),
        context_overflow: None,
        malformed_tool_proposal: None,
    }
}

fn invalid_or_unsupported(failure: &super::client::HttpFailure) -> ModelError {
    ModelError {
        kind: ModelErrorKind::InvalidRequest,
        message: failure.message.clone(),
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: failure.retry_after_ms,
        provider_code: failure.provider_code.clone(),
        context_overflow: None,
        malformed_tool_proposal: None,
    }
}

fn timeout(failure: &super::client::HttpFailure) -> ModelError {
    ModelError {
        kind: ModelErrorKind::Timeout,
        message: failure.message.clone(),
        retry_disposition: if failure.status == reqwest::StatusCode::REQUEST_TIMEOUT
            || failure
                .provider_code
                .as_deref()
                .is_some_and(|code| code.eq_ignore_ascii_case("timeout"))
        {
            crate::model::error::ModelRetryDisposition::Transient
        } else {
            crate::model::error::ModelRetryDisposition::Never
        },
        retry_after_ms: failure.retry_after_ms,
        provider_code: failure.provider_code.clone(),
        context_overflow: None,
        malformed_tool_proposal: None,
    }
}

fn rate_limit(failure: &super::client::HttpFailure) -> ModelError {
    ModelError {
        kind: ModelErrorKind::RateLimit,
        message: failure.message.clone(),
        retry_disposition: if is_permanent_quota(failure.provider_code.as_deref(), &failure.message)
        {
            crate::model::error::ModelRetryDisposition::Never
        } else {
            crate::model::error::ModelRetryDisposition::Transient
        },
        retry_after_ms: failure.retry_after_ms,
        provider_code: failure.provider_code.clone(),
        context_overflow: None,
        malformed_tool_proposal: None,
    }
}

fn provider_error(failure: &super::client::HttpFailure) -> ModelError {
    ModelError {
        kind: ModelErrorKind::ProviderError,
        message: failure.message.clone(),
        retry_disposition: if !is_permanent_quota(
            failure.provider_code.as_deref(),
            &failure.message,
        ) && (retryable_server_status(failure.status)
            || explicitly_retryable_provider_code(failure.provider_code.as_deref()))
        {
            crate::model::error::ModelRetryDisposition::Transient
        } else {
            crate::model::error::ModelRetryDisposition::Never
        },
        retry_after_ms: failure.retry_after_ms,
        provider_code: failure.provider_code.clone(),
        context_overflow: None,
        malformed_tool_proposal: None,
    }
}

/// Statuses that are explicit provider/server availability evidence. A
/// generic `ProviderError` is not retryable merely because it came from a
/// transport or an HTTP response.
pub(crate) fn retryable_server_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 500 | 502 | 503 | 504 | 529)
}

/// Provider error codes that explicitly describe temporary availability or
/// throttling. Codes are normalized here so the Agent Loop never interprets
/// provider strings itself.
pub(crate) fn explicitly_retryable_provider_code(code: Option<&str>) -> bool {
    code.map(str::to_ascii_lowercase).is_some_and(|code| {
        matches!(
            code.as_str(),
            "rate_limit_exceeded"
                | "rate_limit_error"
                | "upstream_rate_limited"
                | "server_error"
                | "internal_server_error"
                | "internal_error"
                | "service_unavailable"
                | "temporarily_unavailable"
                | "upstream_error"
                | "upstream_timeout"
                | "overloaded"
                | "overloaded_error"
                | "network_error"
                | "insufficient_system_resource"
        )
    })
}

/// Codes and messages that prove a rate-limit response is permanent quota or
/// billing exhaustion rather than temporary throttling.
pub(crate) fn is_permanent_quota(code: Option<&str>, message: &str) -> bool {
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
                | "account_deactivated"
        )
    }) || message.contains("insufficient quota")
        || message.contains("quota exceeded")
        || message.contains("billing")
        || message.contains("payment required")
}

fn sdk_status_disposition(
    status: u16,
    provider_code: Option<&str>,
    message: &str,
    kind: &ModelErrorKind,
) -> ModelRetryDisposition {
    match kind {
        ModelErrorKind::RateLimit => {
            if is_permanent_quota(provider_code, message) || status != 429 {
                ModelRetryDisposition::Never
            } else {
                ModelRetryDisposition::Transient
            }
        }
        ModelErrorKind::Timeout => {
            if status == 408
                || provider_code.is_some_and(|code| code.eq_ignore_ascii_case("timeout"))
            {
                ModelRetryDisposition::Transient
            } else {
                ModelRetryDisposition::Never
            }
        }
        ModelErrorKind::ProviderError => {
            if !is_permanent_quota(provider_code, message)
                && (retryable_server_status(
                    reqwest::StatusCode::from_u16(status).unwrap_or_default(),
                ) || explicitly_retryable_provider_code(provider_code))
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
        | ModelErrorKind::Transport
        // A malformed tool proposal is a generation defect with its own
        // bounded Agent-Loop recovery; it never joins the transient budget.
        | ModelErrorKind::MalformedToolProposal => ModelRetryDisposition::Never,
    }
}

/// Normalizes in-band OpenAI-compatible stream error evidence. The adapter
/// receives structured fields from the wire event; a provider-looking
/// cancellation code is not runtime cancellation and stays non-retryable.
pub(crate) fn stream_retry_disposition(
    error_type: Option<&str>,
    provider_code: Option<&str>,
    numeric_code: Option<u64>,
    message: &str,
) -> ModelRetryDisposition {
    if is_permanent_quota(provider_code.or(error_type), message) {
        return ModelRetryDisposition::Never;
    }
    if matches!(numeric_code, Some(408))
        || error_type.is_some_and(|code| code.eq_ignore_ascii_case("timeout"))
        || provider_code.is_some_and(|code| code.eq_ignore_ascii_case("timeout"))
        || matches!(error_type, Some("408"))
        || matches!(provider_code, Some("408"))
    {
        return ModelRetryDisposition::Transient;
    }
    if numeric_code == Some(429)
        || matches!(
            error_type,
            Some("429" | "rate_limit_exceeded" | "rate_limit_error")
        )
        || matches!(
            provider_code,
            Some("429" | "rate_limit_exceeded" | "rate_limit_error")
        )
    {
        return ModelRetryDisposition::Transient;
    }
    if numeric_code.is_some_and(|code| matches!(code, 500 | 502 | 503 | 504 | 529))
        || explicitly_retryable_provider_code(provider_code)
        || explicitly_retryable_provider_code(error_type)
    {
        ModelRetryDisposition::Transient
    } else {
        ModelRetryDisposition::Never
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

#[cfg(test)]
mod tests {
    use super::{
        explicitly_retryable_provider_code, is_permanent_quota, map_chat_finish_reason,
        map_sdk_chat_finish_reason, normalize_error, provider_error, rate_limit,
        stream_retry_disposition,
    };
    use crate::model::adapter::openai::client::HttpFailure;
    use crate::model::error::{
        ModelError, ModelErrorKind, ModelRetryDisposition, is_context_window_error,
    };
    use crate::model::finish::ModelFinishReason;
    use async_openai::types::chat::FinishReason;

    fn http_failure(status: u16, provider_code: Option<&str>, message: &str) -> HttpFailure {
        HttpFailure {
            status: reqwest::StatusCode::from_u16(status).expect("valid test status"),
            retry_after_ms: None,
            message: message.to_owned(),
            provider_code: provider_code.map(str::to_owned),
        }
    }

    #[test]
    fn provider_retry_classification_requires_retryable_evidence() {
        assert_eq!(
            provider_error(&http_failure(500, None, "server unavailable")).retry_disposition,
            ModelRetryDisposition::Transient
        );
        assert_eq!(
            provider_error(&http_failure(409, None, "conflict")).retry_disposition,
            ModelRetryDisposition::Never
        );
        assert_eq!(
            rate_limit(&http_failure(429, Some("rate_limit_exceeded"), "slow down"))
                .retry_disposition,
            ModelRetryDisposition::Transient
        );
        assert_eq!(
            rate_limit(&http_failure(
                429,
                Some("insufficient_quota"),
                "quota exhausted"
            ))
            .retry_disposition,
            ModelRetryDisposition::Never
        );
        assert_eq!(
            super::timeout(&http_failure(408, None, "request timed out")).retry_disposition,
            ModelRetryDisposition::Transient
        );
        assert_eq!(
            super::context_window(&http_failure(
                400,
                Some("context_length_exceeded"),
                "context window exceeded",
            ))
            .retry_disposition,
            ModelRetryDisposition::Never
        );
        assert!(explicitly_retryable_provider_code(Some("overloaded")));
        assert!(is_permanent_quota(
            Some("billing_hard_limit_reached"),
            "billing"
        ));
    }

    #[test]
    fn provider_looking_cancellation_is_not_runtime_cancellation() {
        assert_eq!(
            stream_retry_disposition(Some("cancelled"), Some("cancelled"), None, "cancelled"),
            ModelRetryDisposition::Never
        );
        assert_ne!(
            provider_error(&http_failure(500, Some("cancelled"), "cancelled")).kind,
            ModelErrorKind::Cancelled
        );
    }

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
            assert!(is_context_window_error(message, None), "{message}");
        }
        for message in ["invalid api key", "missing required field"] {
            assert!(!is_context_window_error(message, None), "{message}");
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
