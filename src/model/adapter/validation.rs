//! Deterministic local capability validation for model requests.
//!
//! This is the runtime/model invocation boundary: everything rustX can know
//! locally is decided here, **before** a provider request is opened, so the
//! provider is never the first validator of a known rustX mismatch. It
//! validates protocol agreement, effective-capability agreement with the
//! canonical content of the request, continuation variant agreement, and
//! tool identity integrity. Provider-specific restrictions that cannot be
//! known locally still surface later as normalized `InvalidRequest` or
//! `Unsupported` provider errors.

use std::collections::BTreeMap;

use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::invocation::validate_content_modalities;
use crate::model::types::{ModelProtocol, ModelRequest};
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::ToolId;

/// Tools validated for one request, resolved by model-facing name.
#[derive(Debug, Clone, Default)]
pub struct ValidatedTools {
    by_name: BTreeMap<String, ToolId>,
}

impl ValidatedTools {
    /// The tool names in deterministic (sorted) order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    /// Resolves a model-facing tool name to its canonical [`ToolId`].
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<&ToolId> {
        self.by_name.get(name)
    }
}

/// Validates everything rustX can know locally about a request for the given
/// protocol.
///
/// Returns the resolved tool table on success, or a normalized
/// [`ModelError`]. Validation failures happen before any provider request.
///
/// # Errors
///
/// Returns `InvalidRequest` when the request protocol does not match the
/// adapter protocol, when the continuation variant does not match the
/// protocol, or when tool identities are empty or ambiguous.
pub fn validate_request(
    request: &ModelRequest,
    protocol: ModelProtocol,
) -> Result<ValidatedTools, ModelError> {
    if request.protocol() != protocol {
        return Err(invalid_request(format!(
            "request protocol {} does not match adapter protocol {}",
            serde_name(request.protocol()),
            serde_name(protocol)
        )));
    }

    // Effective-capability agreement is checked before anything else that
    // could reach the network: content the invocation cannot represent is
    // rejected here, not by the provider.
    validate_content_modalities(&request.messages, &request.invocation.capabilities)?;
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

    validate_continuation(request, protocol)?;
    validate_tools(request)
}

fn validate_continuation(
    request: &ModelRequest,
    protocol: ModelProtocol,
) -> Result<(), ModelError> {
    match (&request.continuation, protocol) {
        (None, _) => Ok(()),
        (Some(_state), ModelProtocol::OpenAiChatCompletions) => Err(invalid_request(
            "OpenAI Chat Completions has no Responses-style continuation; \
             continuation state must be None"
                .to_owned(),
        )),
        (Some(state), ModelProtocol::OpenAiResponses) => {
            if matches!(state, ProviderContinuationState::OpenAiResponses(_)) {
                Ok(())
            } else {
                Err(invalid_request(
                    "continuation state must be OpenAiResponses for the \
                     OpenAiResponses protocol"
                        .to_owned(),
                ))
            }
        }
        (Some(state), ModelProtocol::AnthropicMessages) => {
            if matches!(state, ProviderContinuationState::Anthropic(_)) {
                Ok(())
            } else {
                Err(invalid_request(
                    "continuation state must be Anthropic for the \
                     AnthropicMessages protocol"
                        .to_owned(),
                ))
            }
        }
    }
}

fn validate_tools(request: &ModelRequest) -> Result<ValidatedTools, ModelError> {
    let mut by_name: BTreeMap<String, ToolId> = BTreeMap::new();
    for tool in &request.tools {
        if tool.name.is_empty() {
            return Err(invalid_request(
                "tool definitions must carry a non-empty model-facing name".to_owned(),
            ));
        }
        if tool.id.as_str().is_empty() {
            return Err(invalid_request(format!(
                "tool {:?} must carry a non-empty ToolId",
                tool.name
            )));
        }
        if by_name.insert(tool.name.clone(), tool.id.clone()).is_some() {
            return Err(invalid_request(format!(
                "duplicate model-facing tool name {:?}; tool names must be \
                 unambiguous before a request is sent",
                tool.name
            )));
        }
    }
    Ok(ValidatedTools { by_name })
}

fn invalid_request(message: String) -> ModelError {
    ModelError {
        kind: ModelErrorKind::InvalidRequest,
        message,
        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
    }
}

fn serde_name(protocol: ModelProtocol) -> String {
    serde_json::to_string(&protocol).unwrap_or_else(|_| format!("{protocol:?}"))
}

#[cfg(test)]
mod tests {
    use super::validate_request;
    use crate::model::error::ModelErrorKind;
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::continuation::{
        AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
    };
    use crate::runtime::identity::ToolId;
    use crate::tools::types::ModelToolDefinition;

    fn request() -> ModelRequest {
        ModelRequest {
            invocation: crate::model::invocation::ModelInvocationConfig {
                model: "m".to_owned(),
                protocol: ModelProtocol::OpenAiResponses,
                max_output_tokens: 512,
                request_params: crate::model::invocation::RequestParams::new(),
                capabilities: crate::model::catalog::ModelCapabilities::text_only(true, true),
                compat: crate::model::catalog::ModelCompat::default(),
            },
            messages: Vec::new(),
            tools: Vec::new(),
            effective_system_prompt: String::new(),
            continuation: None,
        }
    }

    fn tool(name: &str, id: &str) -> ModelToolDefinition {
        ModelToolDefinition {
            id: ToolId::new(id),
            name: name.to_owned(),
            description: "d".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    #[test]
    fn protocol_mismatch_is_invalid_request() {
        let mut r = request();
        r.invocation.protocol = ModelProtocol::AnthropicMessages;
        let error = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
    }

    #[test]
    fn chat_completions_rejects_any_continuation() {
        for state in [
            ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stored {
                previous_response_id: "resp_1".to_owned(),
            }),
            ProviderContinuationState::Anthropic(AnthropicContinuation {
                opaque: serde_json::json!({}),
            }),
        ] {
            let mut r = request();
            r.invocation.protocol = ModelProtocol::OpenAiChatCompletions;
            r.continuation = Some(state);
            let error =
                validate_request(&r, ModelProtocol::OpenAiChatCompletions).expect_err("must fail");
            assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
        }
    }

    #[test]
    fn responses_rejects_non_responses_continuation() {
        let mut r = request();
        r.continuation = Some(ProviderContinuationState::Anthropic(
            AnthropicContinuation {
                opaque: serde_json::json!({"signature": "s"}),
            },
        ));
        let error = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
    }

    #[test]
    fn anthropic_rejects_non_anthropic_continuation() {
        let mut r = request();
        r.invocation.protocol = ModelProtocol::AnthropicMessages;
        r.continuation = Some(ProviderContinuationState::OpenAiResponses(
            OpenAiResponsesContinuation::Stateless { items: Vec::new() },
        ));
        let error = validate_request(&r, ModelProtocol::AnthropicMessages).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
    }

    #[test]
    fn duplicate_tool_names_are_rejected() {
        let mut r = request();
        r.tools = vec![tool("dup", "tool-a"), tool("dup", "tool-b")];
        let error = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
        assert!(error.message.contains("dup"));
    }

    #[test]
    fn empty_tool_name_is_rejected() {
        let mut r = request();
        r.tools = vec![tool("", "tool-a")];
        let error = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
    }

    #[test]
    fn empty_tool_id_is_rejected() {
        let mut r = request();
        r.tools = vec![tool("name", "")];
        let error = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
    }

    #[test]
    fn valid_tools_resolve_by_name() {
        let mut r = request();
        r.tools = vec![tool("list", "tool-list"), tool("read", "tool-read")];
        let tools = validate_request(&r, ModelProtocol::OpenAiResponses).expect("must pass");
        assert_eq!(tools.resolve("list").map(ToolId::as_str), Some("tool-list"));
        assert_eq!(tools.resolve("read").map(ToolId::as_str), Some("tool-read"));
        assert_eq!(tools.resolve("missing"), None);
        assert_eq!(
            tools.names().collect::<Vec<_>>(),
            vec!["list", "read"],
            "names resolve in deterministic sorted order"
        );
    }
}
