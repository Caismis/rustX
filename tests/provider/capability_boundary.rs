//! Deterministic capability-boundary tests: canonical content that a
//! protocol cannot represent is rejected before any provider request, and
//! full message histories translate without changing canonical roles.

use crate::common::{describe_events, simple_request, sse_fixture};
use rustx::message::content::{ImageReference, TextBlock};
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, ToolMessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ChatReasoningReplay, ModelAdapter,
    ModelErrorKind, ModelEvent, ModelInputMessage, ModelProtocol, ModelRequest,
    OpenAiAdapterConfig, OpenAiChatCompletionsAdapter, OpenAiResponsesAdapter,
};
use rustx::runtime::identity::MessageId;
use rustx::runtime::identity::{ArtifactId, ToolCallId, ToolId};
use rustx::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

async fn unsupported_rejected(
    adapter: &dyn ModelAdapter,
    request: ModelRequest,
    server: &crate::common::FixtureServer,
) {
    let events = crate::common::collect_events(adapter, request).await;
    assert_eq!(
        events.len(),
        1,
        "rejected before the network: {}",
        describe_events(&events)
    );
    let ModelEvent::Failed { error } = &events[0] else {
        panic!("expected Failed");
    };
    assert_eq!(error.kind, ModelErrorKind::Unsupported);
    assert_eq!(server.attempt_count(), 0, "no provider request was made");
}

fn image_user_request(protocol: ModelProtocol, model: &str) -> ModelRequest {
    let mut request = simple_request(protocol, model, "what is this?");
    request.messages[0] = ModelInputMessage::Canonical(MessageBlock::User(UserMessageBlock {
        id: rustx::runtime::identity::MessageId::new("msg-img"),
        content: vec![UserContentBlock::Image(ImageReference {
            artifact_id: ArtifactId::new("artifact-img-1"),
            alt: None,
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    }));
    request
}

fn history_request(protocol: ModelProtocol, model: &str) -> ModelRequest {
    let mut request = simple_request(protocol, model, "Now continue");
    request.messages = vec![
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-u1"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "List the directory".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }),
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("msg-a1"),
            content: vec![
                AssistantContentBlock::Text(TextBlock {
                    text: "Sure.".to_owned(),
                }),
                AssistantContentBlock::ToolCall(rustx::tools::types::ToolCall {
                    id: ToolCallId::new("call_1"),
                    tool_id: ToolId::new("tool-list"),
                    name: "list_directory".to_owned(),
                    arguments: serde_json::json!({"path": "."}),
                }),
            ],
        }),
        MessageBlock::Tool(ToolMessageBlock {
            occurrence: rustx::message::types::ToolCallOccurrenceRef::new(
                rustx::runtime::identity::MessageId::new("msg-a1"),
                rustx::message::types::ContentBlockIndex::new(1),
            ),
            id: MessageId::new("msg-t1"),
            tool_call_id: ToolCallId::new("call_1"),
            tool_id: ToolId::new("tool-list"),
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Success,
                content: vec![ToolResultContent::Text(TextBlock {
                    text: "[\"a.txt\"]".to_owned(),
                })],
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        }),
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-u2"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Now continue".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }),
    ]
    .into_iter()
    .map(ModelInputMessage::Canonical)
    .collect();
    "Be concise.".clone_into(&mut request.effective_system_prompt);
    request.tools = vec![crate::common::model_tool("list_directory", "tool-list")];
    request
}

/// A user image reference cannot be represented without artifact resolution;
/// all three protocols reject it before the network.
#[tokio::test]
async fn image_references_are_unsupported() {
    let chat_server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let responses_server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let anthropic_server =
        crate::common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse"))
            .await;
    let cases: Vec<(
        &str,
        Box<dyn ModelAdapter>,
        ModelRequest,
        &crate::common::FixtureServer,
    )> = vec![
        (
            "openai_chat",
            Box::new(OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new(
                "k",
                chat_server.url("/v1"),
            ))),
            image_user_request(ModelProtocol::OpenAiChatCompletions, "gpt-test"),
            &chat_server,
        ),
        (
            "openai_responses",
            Box::new(OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new(
                "k",
                responses_server.url("/v1"),
            ))),
            image_user_request(ModelProtocol::OpenAiResponses, "gpt-test"),
            &responses_server,
        ),
        (
            "anthropic",
            Box::new(AnthropicMessagesAdapter::new(AnthropicAdapterConfig::new(
                "k",
                anthropic_server.url(""),
            ))),
            image_user_request(ModelProtocol::AnthropicMessages, "claude-test"),
            &anthropic_server,
        ),
    ];
    for (name, adapter, request, server) in cases {
        unsupported_rejected(&*adapter, request, server).await;
        eprintln!("{name}: image references rejected as Unsupported");
    }
}

/// Chat Completions replays previous reasoning through vLLM's dedicated
/// assistant-message field instead of flattening it into visible text.
#[tokio::test]
async fn chat_replays_previous_reasoning() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let mut request = history_request(ModelProtocol::OpenAiChatCompletions, "gpt-test");
    request.invocation.compat.chat_reasoning_replay = Some(ChatReasoningReplay::Reasoning);
    request.messages.insert(
        2,
        ModelInputMessage::Canonical(MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("msg-r"),
            content: vec![AssistantContentBlock::Reasoning(
                rustx::message::types::ReasoningBlock {
                    text: Some("Think.".to_owned()),
                    provider_state: None,
                },
            )],
        })),
    );
    let adapter =
        OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("k", server.url("/v1")));
    let events = crate::common::collect_events(&adapter, request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let reasoning_message = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["reasoning"] == "Think.")
        .expect("assistant reasoning is replayed");
    assert_eq!(reasoning_message["role"], "assistant");
    assert!(
        reasoning_message["content"]
            .as_array()
            .expect("assistant content array")
            .is_empty(),
        "reasoning is not flattened into visible assistant content"
    );
}

/// The catalog selects the assistant reasoning replay dialect explicitly:
/// vLLM/OpenRouter, preserved-thinking APIs, or providers that require it to
/// be omitted from later ordinary turns.
#[tokio::test]
async fn chat_reasoning_replay_dialects_are_explicit() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let adapter =
        OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("k", server.url("/v1")));
    for (attempt, (dialect, field)) in [
        (ChatReasoningReplay::Reasoning, Some("reasoning")),
        (
            ChatReasoningReplay::ReasoningContent,
            Some("reasoning_content"),
        ),
        (ChatReasoningReplay::Omit, None),
    ]
    .into_iter()
    .enumerate()
    {
        let mut request = history_request(ModelProtocol::OpenAiChatCompletions, "gpt-test");
        request.invocation.compat.chat_reasoning_replay = Some(dialect);
        let assistant_index = request
            .messages
            .iter()
            .position(|message| {
                matches!(
                    message,
                    ModelInputMessage::Canonical(MessageBlock::Assistant(_))
                )
            })
            .expect("assistant history");
        request.messages.insert(
            assistant_index,
            ModelInputMessage::Canonical(MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new(format!("msg-r-{attempt}")),
                content: vec![AssistantContentBlock::Reasoning(
                    rustx::message::types::ReasoningBlock {
                        text: Some("Think.".to_owned()),
                        provider_state: None,
                    },
                )],
            })),
        );
        let events = crate::common::collect_events(&adapter, request).await;
        assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
        let body: serde_json::Value =
            serde_json::from_str(&server.request_body(attempt)).expect("request body is JSON");
        let reasoning_message = body["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .find(|message| message["role"] == "assistant")
            .expect("assistant message");
        if let Some(field) = field {
            assert_eq!(reasoning_message[field], "Think.");
        } else {
            assert!(reasoning_message.get("reasoning").is_none());
            assert!(reasoning_message.get("reasoning_content").is_none());
        }
    }
}

/// Omission is decided while translating canonical content, so an
/// unexposed reasoning block does not need a fabricated text value or
/// provider-state conversion.
#[tokio::test]
async fn chat_omit_ignores_unavailable_reasoning() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let mut request = history_request(ModelProtocol::OpenAiChatCompletions, "gpt-test");
    request.invocation.compat.chat_reasoning_replay = Some(ChatReasoningReplay::Omit);
    request.messages.insert(
        2,
        ModelInputMessage::Canonical(MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("msg-r-omit-unavailable"),
            content: vec![
                AssistantContentBlock::Reasoning(rustx::message::types::ReasoningBlock {
                    text: None,
                    provider_state: Some(
                        rustx::runtime::continuation::ProviderContinuationState::Anthropic(
                            rustx::runtime::continuation::AnthropicContinuation {
                                opaque: serde_json::json!({"signature": "opaque"}),
                            },
                        ),
                    ),
                }),
                AssistantContentBlock::Text(TextBlock {
                    text: "Visible answer.".to_owned(),
                }),
            ],
        })),
    );

    let events = crate::common::collect_events(
        &OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("k", server.url("/v1"))),
        request,
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));

    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let message = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["content"][0]["text"] == "Visible answer.")
        .expect("visible assistant content is preserved");
    assert!(message.get("reasoning").is_none());
    assert!(message.get("reasoning_content").is_none());
}

/// Omission ignores every historical reasoning block, even when one
/// assistant message contains more than the Chat dialect can represent.
#[tokio::test]
async fn chat_omit_ignores_multiple_reasoning_blocks() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let mut request = history_request(ModelProtocol::OpenAiChatCompletions, "gpt-test");
    request.invocation.compat.chat_reasoning_replay = Some(ChatReasoningReplay::Omit);
    request.messages.insert(
        2,
        ModelInputMessage::Canonical(MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("msg-r-omit-multiple"),
            content: vec![
                AssistantContentBlock::Reasoning(rustx::message::types::ReasoningBlock {
                    text: Some("first".to_owned()),
                    provider_state: None,
                }),
                AssistantContentBlock::Reasoning(rustx::message::types::ReasoningBlock {
                    text: None,
                    provider_state: None,
                }),
                AssistantContentBlock::Text(TextBlock {
                    text: "Keep this text.".to_owned(),
                }),
                AssistantContentBlock::ToolCall(rustx::tools::types::ToolCall {
                    id: ToolCallId::new("call-omit"),
                    tool_id: ToolId::new("tool-list"),
                    name: "list_directory".to_owned(),
                    arguments: serde_json::json!({"path": "."}),
                }),
            ],
        })),
    );

    let events = crate::common::collect_events(
        &OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("k", server.url("/v1"))),
        request,
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));

    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let message = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["content"][0]["text"] == "Keep this text.")
        .expect("visible assistant content is preserved");
    assert!(message.get("reasoning").is_none());
    assert!(message.get("reasoning_content").is_none());
    assert_eq!(message["tool_calls"][0]["id"], "call-omit");
}

/// Replay modes fail before network execution when the canonical reasoning
/// text was not exposed by the provider.
#[tokio::test]
async fn chat_replay_modes_reject_unavailable_reasoning_text() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    for dialect in [
        ChatReasoningReplay::Reasoning,
        ChatReasoningReplay::ReasoningContent,
    ] {
        let mut request = history_request(ModelProtocol::OpenAiChatCompletions, "gpt-test");
        request.invocation.compat.chat_reasoning_replay = Some(dialect);
        request.messages.insert(
            2,
            ModelInputMessage::Canonical(MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new(format!("msg-r-unavailable-{dialect:?}")),
                content: vec![AssistantContentBlock::Reasoning(
                    rustx::message::types::ReasoningBlock {
                        text: None,
                        provider_state: None,
                    },
                )],
            })),
        );
        unsupported_rejected(
            &OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("k", server.url("/v1"))),
            request,
            &server,
        )
        .await;
    }
}

/// Replay modes fail explicitly rather than silently selecting, merging, or
/// dropping one of multiple canonical reasoning blocks.
#[tokio::test]
async fn chat_replay_modes_reject_multiple_reasoning_blocks() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    for dialect in [
        ChatReasoningReplay::Reasoning,
        ChatReasoningReplay::ReasoningContent,
    ] {
        let mut request = history_request(ModelProtocol::OpenAiChatCompletions, "gpt-test");
        request.invocation.compat.chat_reasoning_replay = Some(dialect);
        request.messages.insert(
            2,
            ModelInputMessage::Canonical(MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new(format!("msg-r-multiple-{dialect:?}")),
                content: vec![
                    AssistantContentBlock::Reasoning(rustx::message::types::ReasoningBlock {
                        text: Some("first".to_owned()),
                        provider_state: None,
                    }),
                    AssistantContentBlock::Reasoning(rustx::message::types::ReasoningBlock {
                        text: Some("second".to_owned()),
                        provider_state: None,
                    }),
                ],
            })),
        );
        unsupported_rejected(
            &OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("k", server.url("/v1"))),
            request,
            &server,
        )
        .await;
    }
}

/// Chat Completions prepends the effective System authority and translates a
/// full canonical history (User, Assistant with tool calls, Tool, User) into
/// provider messages without changing any conversational role.
#[tokio::test]
async fn chat_translates_full_history_roles() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let adapter =
        OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("k", server.url("/v1")));
    let events = crate::common::collect_events(
        &adapter,
        history_request(ModelProtocol::OpenAiChatCompletions, "gpt-test"),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    assert!(!server.request_body(0).contains("assistant_message_id"));
    assert!(!server.request_body(0).contains("occurrence"));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let messages = body["messages"].as_array().expect("messages");
    let roles: Vec<&str> = messages
        .iter()
        .map(|message| message["role"].as_str().expect("role"))
        .collect();
    assert_eq!(
        roles,
        vec!["system", "user", "assistant", "tool", "user"],
        "effective System authority precedes the three canonical conversational roles"
    );
    let assistant = &messages[2];
    assert_eq!(
        assistant["tool_calls"][0]["function"]["name"],
        "list_directory"
    );
    assert_eq!(
        assistant["tool_calls"][0]["id"], "call_1",
        "provider call id remains the ToolCallId"
    );
    let tool_message = &messages[3];
    assert_eq!(tool_message["tool_call_id"], "call_1");
    assert_eq!(tool_message["content"][0]["text"], "[\"a.txt\"]");
}

/// Responses translates the same full history into input items.
#[tokio::test]
async fn responses_translates_full_history() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let adapter = OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new("k", server.url("/v1")));
    let events = crate::common::collect_events(
        &adapter,
        history_request(ModelProtocol::OpenAiResponses, "gpt-test"),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    assert!(!server.request_body(0).contains("assistant_message_id"));
    assert!(!server.request_body(0).contains("occurrence"));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert!(
        body["instructions"].as_str().is_some(),
        "system becomes instructions"
    );
    let input = body["input"].as_array().expect("input items");
    let item_types: Vec<&str> = input
        .iter()
        .map(|item| item["type"].as_str().expect("type"))
        .collect();
    assert_eq!(
        item_types,
        vec![
            "message",
            "message",
            "function_call",
            "function_call_output",
            "message"
        ],
        "canonical roles map to Responses input item types"
    );
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[1]["role"], "assistant");
    assert_eq!(input[2]["name"], "list_directory");
    assert_eq!(input[2]["call_id"], "call_1");
    assert_eq!(input[3]["call_id"], "call_1");
}

/// Anthropic translates the full history into user/assistant messages, with
/// consecutive tool results merged into one user message.
#[tokio::test]
async fn anthropic_translates_full_history() {
    let server =
        crate::common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse"))
            .await;
    let adapter = AnthropicMessagesAdapter::new(AnthropicAdapterConfig::new("k", server.url("")));
    let events = crate::common::collect_events(
        &adapter,
        history_request(ModelProtocol::AnthropicMessages, "claude-test"),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    assert!(!server.request_body(0).contains("assistant_message_id"));
    assert!(!server.request_body(0).contains("occurrence"));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert_eq!(body["system"][0]["text"], "Be concise.");
    let messages = body["messages"].as_array().expect("messages");
    let roles: Vec<&str> = messages
        .iter()
        .map(|message| message["role"].as_str().expect("role"))
        .collect();
    assert_eq!(roles, vec!["user", "assistant", "user", "user"]);
    let assistant = &messages[1];
    assert_eq!(assistant["content"][0]["type"], "text");
    assert_eq!(assistant["content"][1]["type"], "tool_use");
    assert_eq!(assistant["content"][1]["id"], "call_1");
    assert_eq!(assistant["content"][1]["name"], "list_directory");
    let tool_result_user = &messages[2];
    assert_eq!(tool_result_user["content"][0]["type"], "tool_result");
    assert_eq!(tool_result_user["content"][0]["tool_use_id"], "call_1");
}

/// A tool result with a file reference cannot be represented; it is rejected
/// before the network.
#[tokio::test]
async fn file_tool_results_are_unsupported() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let mut request = history_request(ModelProtocol::OpenAiChatCompletions, "gpt-test");
    let tool_message = request
        .messages
        .iter_mut()
        .find_map(|message| match message {
            ModelInputMessage::Canonical(MessageBlock::Tool(tool_message)) => Some(tool_message),
            ModelInputMessage::Canonical(MessageBlock::User(_) | MessageBlock::Assistant(_))
            | ModelInputMessage::RequestOnly(_) => None,
        })
        .expect("tool message expected");
    tool_message.result.content = vec![ToolResultContent::File(
        rustx::message::content::FileReference {
            artifact_id: ArtifactId::new("artifact-file-1"),
            name: Some("report.pdf".to_owned()),
            mime_type: Some("application/pdf".to_owned()),
            description: None,
        },
    )];
    unsupported_rejected(
        &OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("k", server.url("/v1"))),
        request,
        &server,
    )
    .await;
}

#[tokio::test]
async fn anthropic_resolved_user_image_and_unsupported_placements() {
    use base64::Engine;
    use rustx::model::catalog::Modality;
    let server =
        crate::common::FixtureServer::start(|_, _| sse_fixture("anthropic", "text.sse")).await;
    let adapter = AnthropicMessagesAdapter::new(AnthropicAdapterConfig::new("k", server.url("")));
    let mut bytes = Vec::new();
    {
        let encoder = png::Encoder::new(&mut bytes, 1, 1);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[128]).unwrap();
    }
    let mut request = image_user_request(ModelProtocol::AnthropicMessages, "claude-test");
    request
        .invocation
        .capabilities
        .input_modalities
        .insert(Modality::Image);
    request
        .images
        .insert(ArtifactId::new("artifact-img-1"), bytes.clone());
    let events = crate::common::collect_events(&adapter, request.clone()).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value = serde_json::from_str(&server.request_body(0)).unwrap();
    assert_eq!(
        body["messages"][0]["content"][0]["source"]["data"],
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    );
    assert!(
        !serde_json::to_string(&request)
            .unwrap()
            .contains(&base64::engine::general_purpose::STANDARD.encode(&bytes)),
        "ephemeral data never enters serialized canonical request evidence"
    );
    let before = server.attempt_count();
    let mut text = request.clone();
    text.invocation
        .capabilities
        .input_modalities
        .remove(&Modality::Image);
    let mut assistant = request.clone();
    assistant.messages[0] =
        ModelInputMessage::Canonical(MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("assistant-image"),
            content: vec![AssistantContentBlock::Image(ImageReference {
                artifact_id: ArtifactId::new("artifact-img-1"),
                alt: None,
            })],
        }));
    let mut corrupt = request.clone();
    corrupt
        .images
        .insert(ArtifactId::new("artifact-img-1"), b"invalid".to_vec());
    let mut unresolved = request;
    unresolved.images.clear();
    for invalid in [text, assistant, corrupt, unresolved] {
        let events = crate::common::collect_events(&adapter, invalid).await;
        assert!(matches!(events.first(), Some(ModelEvent::Failed { .. })));
        assert_eq!(
            server.attempt_count(),
            before,
            "invalid request fails before network I/O"
        );
    }
}

/// Every Image-capable protocol preserves canonical placement, part ordering,
/// exact bytes and parallel call correlation. Invalid inputs never reach HTTP.
#[tokio::test]
async fn all_protocols_transport_user_and_ordered_tool_images() {
    use base64::Engine;
    use rustx::model::catalog::Modality;
    for (protocol, fixture) in [
        (ModelProtocol::OpenAiChatCompletions, "openai_chat"),
        (ModelProtocol::OpenAiResponses, "openai_responses"),
        (ModelProtocol::AnthropicMessages, "anthropic"),
    ] {
        let server = crate::common::FixtureServer::start(move |_, _| {
            sse_fixture(
                fixture,
                if fixture == "anthropic" {
                    "text.sse"
                } else {
                    "plain_text.sse"
                },
            )
        })
        .await;
        let adapter: Box<dyn ModelAdapter> = match protocol {
            ModelProtocol::OpenAiChatCompletions => Box::new(OpenAiChatCompletionsAdapter::new(
                OpenAiAdapterConfig::new("k", server.url("/v1")),
            )),
            ModelProtocol::OpenAiResponses => Box::new(OpenAiResponsesAdapter::new(
                OpenAiAdapterConfig::new("k", server.url("/v1")),
            )),
            ModelProtocol::AnthropicMessages => Box::new(AnthropicMessagesAdapter::new(
                AnthropicAdapterConfig::new("k", server.url("")),
            )),
        };
        let mut bytes = Vec::new();
        png::Encoder::new(&mut bytes, 1, 1)
            .write_header()
            .unwrap()
            .write_image_data(&[128])
            .unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let image = ImageReference {
            artifact_id: ArtifactId::new("artifact-img-1"),
            alt: None,
        };
        let mut request = history_request(protocol, "explicit-image-model");
        request
            .invocation
            .capabilities
            .input_modalities
            .insert(Modality::Image);
        request.images.insert(image.artifact_id.clone(), bytes);
        if let ModelInputMessage::Canonical(MessageBlock::User(user)) = &mut request.messages[0] {
            user.content.push(UserContentBlock::Image(image.clone()));
        }
        if let ModelInputMessage::Canonical(MessageBlock::Assistant(assistant)) =
            &mut request.messages[1]
        {
            let AssistantContentBlock::ToolCall(mut call) = assistant.content[1].clone() else {
                panic!()
            };
            call.id = ToolCallId::new("call_2");
            assistant
                .content
                .push(AssistantContentBlock::ToolCall(call));
        }
        if let ModelInputMessage::Canonical(MessageBlock::Tool(tool)) = &mut request.messages[2] {
            tool.result.content = vec![
                ToolResultContent::Text(TextBlock { text: "A".into() }),
                ToolResultContent::Image(image.clone()),
                ToolResultContent::Text(TextBlock { text: "B".into() }),
                ToolResultContent::Image(image),
            ];
        }
        let mut second = request.messages[2].clone();
        if let ModelInputMessage::Canonical(MessageBlock::Tool(tool)) = &mut second {
            tool.id = MessageId::new("msg-t2");
            tool.tool_call_id = ToolCallId::new("call_2");
            tool.occurrence = rustx::message::types::ToolCallOccurrenceRef::new(
                MessageId::new("msg-a1"),
                rustx::message::types::ContentBlockIndex::new(2),
            );
        }
        request.messages.insert(3, second);
        let canonical = request.messages.clone();
        for image_enabled in [true, false, true] {
            let mut projected = request.clone();
            if !image_enabled {
                projected
                    .invocation
                    .capabilities
                    .input_modalities
                    .remove(&Modality::Image);
                projected.images.clear();
            }
            rustx::model::images::project(&mut projected.messages, image_enabled);
            let before = server.attempt_count();
            let events = crate::common::collect_events(adapter.as_ref(), projected).await;
            assert!(
                matches!(events.last(), Some(ModelEvent::Completed { .. })),
                "{protocol:?}: {events:?}"
            );
            let body: serde_json::Value =
                serde_json::from_str(&server.request_body(usize::try_from(before).unwrap()))
                    .unwrap();
            let serialized = body.to_string();
            assert_eq!(
                serialized.matches(&encoded).count(),
                if image_enabled { 5 } else { 0 }
            );
            if !image_enabled {
                assert!(serialized.contains("Image artifact"));
                continue;
            }
            match protocol {
                ModelProtocol::OpenAiChatCompletions => {
                    let messages = body["messages"].as_array().unwrap();
                    assert_eq!(messages.iter().filter(|m| m["role"] == "user").count(), 2);
                    for id in ["call_1", "call_2"] {
                        let tool = messages.iter().find(|m| m["tool_call_id"] == id).unwrap();
                        assert_eq!(tool["role"], "tool");
                        assert_eq!(tool["content"][0]["text"], "A");
                        assert_eq!(
                            tool["content"][1]["image_url"]["url"],
                            format!("data:image/png;base64,{encoded}")
                        );
                        assert_eq!(tool["content"][2]["text"], "B");
                        assert_eq!(tool["content"][3]["type"], "image_url");
                    }
                }
                ModelProtocol::OpenAiResponses => {
                    let items = body["input"].as_array().unwrap();
                    assert_eq!(items.iter().filter(|m| m["role"] == "user").count(), 2);
                    for id in ["call_1", "call_2"] {
                        let tool = items
                            .iter()
                            .find(|m| m["type"] == "function_call_output" && m["call_id"] == id)
                            .unwrap();
                        assert_eq!(tool["output"][0]["text"], "A");
                        assert_eq!(
                            tool["output"][1]["image_url"],
                            format!("data:image/png;base64,{encoded}")
                        );
                        assert_eq!(tool["output"][2]["text"], "B");
                        assert_eq!(tool["output"][3]["type"], "input_image");
                    }
                }
                ModelProtocol::AnthropicMessages => {
                    let tools: Vec<_> = body["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .flat_map(|m| m["content"].as_array().unwrap())
                        .filter(|p| p["type"] == "tool_result")
                        .collect();
                    assert_eq!(tools.len(), 2);
                    for (tool, id) in tools.iter().zip(["call_1", "call_2"]) {
                        assert_eq!(tool["tool_use_id"], id);
                        assert_eq!(tool["content"][0]["text"], "A");
                        assert_eq!(tool["content"][1]["source"]["data"], encoded);
                        assert_eq!(tool["content"][2]["text"], "B");
                        assert_eq!(tool["content"][3]["type"], "image");
                    }
                }
            }
        }
        assert_eq!(request.messages, canonical);
        let before = server.attempt_count();
        let mut missing = request.clone();
        missing.images.clear();
        let mut corrupt = request.clone();
        corrupt
            .images
            .values_mut()
            .for_each(|bytes| *bytes = b"bad PNG".to_vec());
        let mut oversized = request.clone();
        oversized
            .messages
            .extend(std::iter::repeat_n(request.messages[2].clone(), 7));
        for invalid in [missing, corrupt, oversized] {
            let events = crate::common::collect_events(adapter.as_ref(), invalid).await;
            assert!(matches!(events.first(), Some(ModelEvent::Failed { .. })));
            assert_eq!(server.attempt_count(), before);
        }
    }
}
