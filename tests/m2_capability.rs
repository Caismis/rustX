//! Deterministic capability-boundary tests: canonical content that a
//! protocol cannot represent is rejected before any provider request, and
//! full message histories translate without changing canonical roles.

mod common;

use common::{describe_events, simple_request, sse_fixture};
use rustx::message::content::{ImageReference, TextBlock};
use rustx::message::types::{
    AgentContentBlock, AgentMessageBlock, InboundKind, MessageBlock, SystemAuthority,
    SystemMessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ModelAdapter, ModelErrorKind, ModelEvent,
    ModelProtocol, ModelRequest, OpenAiAdapterConfig, OpenAiChatCompletionsAdapter,
    OpenAiResponsesAdapter, ResponsesStorageMode,
};
use rustx::runtime::identity::MessageId;
use rustx::runtime::identity::{ArtifactId, ToolCallId, ToolId};
use rustx::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

async fn unsupported_rejected(
    adapter: &dyn ModelAdapter,
    request: ModelRequest,
    server: &common::FixtureServer,
) {
    let events = common::collect_events(adapter, request).await;
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
    request.messages[0] = MessageBlock::User(UserMessageBlock {
        id: rustx::runtime::identity::MessageId::new("msg-img"),
        content: vec![UserContentBlock::Image(ImageReference {
            artifact_id: ArtifactId::new("artifact-img-1"),
            alt: None,
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    });
    request
}

fn history_request(protocol: ModelProtocol, model: &str) -> ModelRequest {
    let mut request = simple_request(protocol, model, "Now continue");
    request.messages = vec![
        MessageBlock::System(SystemMessageBlock {
            id: MessageId::new("msg-sys"),
            authority: SystemAuthority::Runtime,
            content: vec![TextBlock {
                text: "Be concise.".to_owned(),
            }],
        }),
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-u1"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "List the directory".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }),
        MessageBlock::Agent(AgentMessageBlock {
            id: MessageId::new("msg-a1"),
            content: vec![
                AgentContentBlock::Text(TextBlock {
                    text: "Sure.".to_owned(),
                }),
                AgentContentBlock::ToolCall(rustx::tools::types::ToolCall {
                    id: ToolCallId::new("call_1"),
                    tool_id: ToolId::new("tool-list"),
                    name: "list_directory".to_owned(),
                    arguments: serde_json::json!({"path": "."}),
                }),
            ],
        }),
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("msg-t1"),
            tool_call_id: ToolCallId::new("call_1"),
            tool_id: ToolId::new("tool-list"),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![ToolResultContent::Text(TextBlock {
                    text: "[\"a.txt\"]".to_owned(),
                })],
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
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
    ];
    request.tools = vec![common::model_tool("list_directory", "tool-list")];
    request
}

/// A user image reference cannot be represented without artifact resolution;
/// all three protocols reject it before the network.
#[tokio::test]
async fn image_references_are_unsupported() {
    let chat_server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let responses_server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let anthropic_server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let cases: Vec<(
        &str,
        Box<dyn ModelAdapter>,
        ModelRequest,
        &common::FixtureServer,
    )> = vec![
        (
            "openai-chat",
            Box::new(OpenAiChatCompletionsAdapter::new(
                OpenAiAdapterConfig::new("k").with_api_base(chat_server.url("/v1")),
            )),
            image_user_request(ModelProtocol::OpenAiChatCompletions, "gpt-test"),
            &chat_server,
        ),
        (
            "openai-responses",
            Box::new(OpenAiResponsesAdapter::new(
                OpenAiAdapterConfig::new("k")
                    .with_api_base(responses_server.url("/v1"))
                    .with_responses_storage(ResponsesStorageMode::Stored),
            )),
            image_user_request(ModelProtocol::OpenAiResponses, "gpt-test"),
            &responses_server,
        ),
        (
            "anthropic",
            Box::new(AnthropicMessagesAdapter::new(
                AnthropicAdapterConfig::new("k").with_api_base(anthropic_server.url("")),
            )),
            image_user_request(ModelProtocol::AnthropicMessages, "claude-test"),
            &anthropic_server,
        ),
    ];
    for (name, adapter, request, server) in cases {
        unsupported_rejected(&*adapter, request, server).await;
        eprintln!("{name}: image references rejected as Unsupported");
    }
}

/// Chat Completions rejects previous reasoning blocks in history instead of
/// flattening them into text.
#[tokio::test]
async fn chat_rejects_previous_reasoning() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let mut request = history_request(ModelProtocol::OpenAiChatCompletions, "gpt-test");
    request.messages.insert(
        2,
        MessageBlock::Agent(AgentMessageBlock {
            id: MessageId::new("msg-r"),
            content: vec![AgentContentBlock::Reasoning(
                rustx::message::types::ReasoningBlock {
                    text: Some("Think.".to_owned()),
                    provider_state: None,
                },
            )],
        }),
    );
    unsupported_rejected(
        &OpenAiChatCompletionsAdapter::new(
            OpenAiAdapterConfig::new("k").with_api_base(server.url("/v1")),
        ),
        request,
        &server,
    )
    .await;
}

/// Chat Completions translates a full canonical history (system, user, agent
/// with tool calls, tool result, user) into provider messages without
/// changing any canonical role.
#[tokio::test]
async fn chat_translates_full_history_roles() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let adapter = OpenAiChatCompletionsAdapter::new(
        OpenAiAdapterConfig::new("k").with_api_base(server.url("/v1")),
    );
    let events = common::collect_events(
        &adapter,
        history_request(ModelProtocol::OpenAiChatCompletions, "gpt-test"),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
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
        "provider roles follow canonical roles without a fifth role"
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
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let adapter = OpenAiResponsesAdapter::new(
        OpenAiAdapterConfig::new("k")
            .with_api_base(server.url("/v1"))
            .with_responses_storage(ResponsesStorageMode::Stored),
    );
    let events = common::collect_events(
        &adapter,
        history_request(ModelProtocol::OpenAiResponses, "gpt-test"),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
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
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let adapter = AnthropicMessagesAdapter::new(
        AnthropicAdapterConfig::new("k").with_api_base(server.url("")),
    );
    let events = common::collect_events(
        &adapter,
        history_request(ModelProtocol::AnthropicMessages, "claude-test"),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
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
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let mut request = history_request(ModelProtocol::OpenAiChatCompletions, "gpt-test");
    let MessageBlock::Tool(tool_message) = &mut request.messages[3] else {
        panic!("tool message expected");
    };
    tool_message.result.content = vec![ToolResultContent::File(
        rustx::message::content::FileReference {
            artifact_id: ArtifactId::new("artifact-file-1"),
            name: Some("report.pdf".to_owned()),
            mime_type: Some("application/pdf".to_owned()),
            description: None,
        },
    )];
    unsupported_rejected(
        &OpenAiChatCompletionsAdapter::new(
            OpenAiAdapterConfig::new("k").with_api_base(server.url("/v1")),
        ),
        request,
        &server,
    )
    .await;
}
