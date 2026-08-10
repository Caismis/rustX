//! Deterministic local capability validation for model requests.
//!
//! M2 does not invent a mutable provider model catalog and performs no
//! network capability discovery. It validates what rustX can know locally
//! from the canonical request itself: protocol agreement, continuation
//! variant agreement, and tool identity integrity. Provider-specific
//! restrictions that cannot be known locally surface later as normalized
//! `InvalidRequest` or `Unsupported` provider errors.

use std::collections::BTreeMap;

use crate::message::types::{InboundKind, MessageBlock};
use crate::model::error::{ModelError, ModelErrorKind};
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
    if request.protocol != protocol {
        return Err(invalid_request(format!(
            "request protocol {} does not match adapter protocol {}",
            serde_name(request.protocol),
            serde_name(protocol)
        )));
    }

    validate_continuation(request, protocol)?;
    validate_agent_status(request)?;
    validate_tools(request)
}

/// Validates a request's ephemeral Agent Status attachment before any
/// provider I/O.
///
/// A malformed attachment is an invalid request: the target message must
/// exist exactly once in the request's canonical messages, must be a
/// `MessageBlock::User`, and must be an ordinary inbound
/// [`InboundKind::Message`]. A compaction summary is user-role history and
/// can never be the status target.
fn validate_agent_status(request: &ModelRequest) -> Result<(), ModelError> {
    let Some(status) = &request.agent_status else {
        return Ok(());
    };
    let occurrences = request
        .messages
        .iter()
        .filter(|message| message_id_of(message) == status.target_message_id)
        .count();
    if occurrences == 0 {
        return Err(invalid_request(format!(
            "Agent Status targets message {} which is not present in the request messages",
            status.target_message_id
        )));
    }
    if occurrences > 1 {
        return Err(invalid_request(format!(
            "Agent Status target message {} appears more than once in the request messages",
            status.target_message_id
        )));
    }
    let Some(MessageBlock::User(target)) = request
        .messages
        .iter()
        .find(|message| message_id_of(message) == status.target_message_id)
    else {
        return Err(invalid_request(format!(
            "Agent Status target message {} is not a user-role message",
            status.target_message_id
        )));
    };
    if target.kind != InboundKind::Message {
        return Err(invalid_request(format!(
            "Agent Status target message {} is not an ordinary inbound message; \
             a compaction summary is never a fresh inbound target",
            status.target_message_id
        )));
    }
    Ok(())
}

fn message_id_of(message: &MessageBlock) -> crate::runtime::identity::MessageId {
    match message {
        MessageBlock::System(system) => system.id.clone(),
        MessageBlock::User(user) => user.id.clone(),
        MessageBlock::Agent(agent) => agent.id.clone(),
        MessageBlock::Tool(tool) => tool.id.clone(),
    }
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
        retry_after_ms: None,
        provider_code: None,
    }
}

fn serde_name(protocol: ModelProtocol) -> String {
    serde_json::to_string(&protocol).unwrap_or_else(|_| format!("{protocol:?}"))
}

#[cfg(test)]
mod tests {
    use super::validate_request;
    use crate::message::types::InboundKind;
    use crate::model::error::ModelErrorKind;
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::continuation::{
        AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
    };
    use crate::runtime::identity::ToolId;
    use crate::tools::types::ModelToolDefinition;

    fn request() -> ModelRequest {
        ModelRequest {
            model: "m".to_owned(),
            protocol: ModelProtocol::OpenAiResponses,
            messages: Vec::new(),
            tools: Vec::new(),
            agent_status: None,
            reasoning: crate::model::types::ReasoningEffort::Medium,
            max_output_tokens: 512,
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
        r.protocol = ModelProtocol::AnthropicMessages;
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
            r.protocol = ModelProtocol::OpenAiChatCompletions;
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
        r.protocol = ModelProtocol::AnthropicMessages;
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

    fn user_message(id: &str, kind: InboundKind) -> crate::message::types::UserMessageBlock {
        use crate::message::content::TextBlock;
        use crate::message::types::{UserContentBlock, UserMessageBlock, UserSource};
        use crate::runtime::identity::MessageId;
        UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "inbound".to_owned(),
            })],
            source: UserSource::Human,
            kind,
            timestamp: Some(
                chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                    .expect("fixed timestamp")
                    .with_timezone(&chrono::Utc),
            ),
        }
    }

    fn attachment(target: &str) -> crate::model::types::AgentStatusAttachment {
        crate::model::types::AgentStatusAttachment {
            target_message_id: crate::runtime::identity::MessageId::new(target),
            rendered: "<system-reminder>Current time: fixed</system-reminder>".to_owned(),
        }
    }

    #[test]
    fn agent_status_target_must_exist_exactly_once_as_ordinary_user_message() {
        let mut r = request();
        r.agent_status = Some(attachment("msg-inbound-1"));
        let missing = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("missing");
        assert_eq!(missing.kind, ModelErrorKind::InvalidRequest);
        assert!(missing.message.contains("not present"));

        r.messages = vec![
            crate::message::types::MessageBlock::User(user_message(
                "msg-inbound-1",
                InboundKind::Message,
            )),
            crate::message::types::MessageBlock::User(user_message(
                "msg-inbound-2",
                InboundKind::Message,
            )),
        ];
        let ok = validate_request(&r, ModelProtocol::OpenAiResponses).expect("must pass");
        assert_eq!(ok.resolve("missing"), None);

        r.messages
            .push(crate::message::types::MessageBlock::User(user_message(
                "msg-inbound-1",
                InboundKind::Message,
            )));
        let duplicate = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("dup");
        assert_eq!(duplicate.kind, ModelErrorKind::InvalidRequest);
        assert!(duplicate.message.contains("more than once"));
    }

    #[test]
    fn agent_status_target_rejects_non_user_and_summary_targets() {
        use crate::message::types::{
            AgentMessageBlock, MessageBlock, SystemAuthority, SystemMessageBlock,
        };
        let mut r = request();
        r.agent_status = Some(attachment("msg-agent-1"));
        r.messages = vec![MessageBlock::Agent(AgentMessageBlock {
            id: crate::runtime::identity::MessageId::new("msg-agent-1"),
            content: Vec::new(),
        })];
        let not_user = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("agent");
        assert_eq!(not_user.kind, ModelErrorKind::InvalidRequest);
        assert!(not_user.message.contains("not a user-role message"));

        r.agent_status = Some(attachment("msg-summary-1"));
        r.messages = vec![MessageBlock::User(user_message(
            "msg-summary-1",
            InboundKind::CompactionSummary,
        ))];
        let summary = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("summary");
        assert_eq!(summary.kind, ModelErrorKind::InvalidRequest);
        assert!(summary.message.contains("compaction summary"));

        r.agent_status = Some(attachment("msg-sys-1"));
        r.messages = vec![MessageBlock::System(SystemMessageBlock {
            id: crate::runtime::identity::MessageId::new("msg-sys-1"),
            authority: SystemAuthority::Platform,
            content: Vec::new(),
        })];
        let system = validate_request(&r, ModelProtocol::OpenAiResponses).expect_err("system");
        assert_eq!(system.kind, ModelErrorKind::InvalidRequest);
    }
}
