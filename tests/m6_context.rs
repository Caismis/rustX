//! Regression tests for the unified model-visible context boundary.
//!
//! Skill guidance and Agent Status are canonical context facts before these
//! adapters run. The adapters receive one frozen Effective System Prompt and
//! perform protocol translation only.

use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantMessageBlock, InboundKind, MessageBlock, SystemAuthority, SystemMessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::{ModelProtocol, ModelRequest, RequestIdentity, RequestSnapshot};
use rustx::runtime::identity::{AttemptId, CapabilityRevision, MessageId, TurnId};

#[path = "common/mod.rs"]
mod common;

const SYSTEM_TEXT: &str = "You are a helpful agent.";
const SKILL_TEXT: &str = "## Skills\n\n- pdf: Create PDF documents.\n";

fn request(protocol: ModelProtocol) -> ModelRequest {
    ModelRequest {
        invocation: common::invocation(protocol, "m6-test"),
        messages: vec![
            MessageBlock::System(SystemMessageBlock {
                id: MessageId::new("msg-system-1"),
                authority: SystemAuthority::Runtime,
                content: vec![TextBlock {
                    text: SYSTEM_TEXT.to_owned(),
                }],
            }),
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("msg-agent-1"),
                content: vec![rustx::message::types::AssistantContentBlock::Text(
                    TextBlock {
                        text: "earlier turn".to_owned(),
                    },
                )],
            }),
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new("msg-user-1"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "hello".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            }),
        ],
        tools: Vec::new(),
        effective_system_prompt: format!("{SYSTEM_TEXT}\n\n{SKILL_TEXT}"),
        continuation: None,
    }
}

#[tokio::test]
async fn anthropic_receives_only_the_frozen_effective_prompt() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("anthropic", "text.sse")
    })
    .await;
    let adapter = rustx::model::AnthropicMessagesAdapter::new(
        rustx::model::AnthropicAdapterConfig::new("test-key", server.url("")),
    );
    let events = common::collect_events(&adapter, request(ModelProtocol::AnthropicMessages)).await;
    assert!(matches!(
        events.last(),
        Some(rustx::model::ModelEvent::Completed { .. })
    ));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let system = body["system"].as_array().expect("system array");
    assert_eq!(system.len(), 1);
    assert_eq!(system[0]["text"], format!("{SYSTEM_TEXT}\n\n{SKILL_TEXT}"));
    let messages = serde_json::to_string(&body["messages"]).expect("messages serialize");
    assert!(!messages.contains(SKILL_TEXT));
}

#[tokio::test]
async fn chat_completions_translates_the_prompt_without_mutating_user_facts() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let adapter = rustx::model::OpenAiChatCompletionsAdapter::new(
        rustx::model::OpenAiAdapterConfig::new("test-key", server.url("/v1")),
    );
    let events =
        common::collect_events(&adapter, request(ModelProtocol::OpenAiChatCompletions)).await;
    assert!(matches!(
        events.last(),
        Some(rustx::model::ModelEvent::Completed { .. })
    ));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(
        messages[0]["content"],
        format!("{SYSTEM_TEXT}\n\n{SKILL_TEXT}")
    );
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[2]["role"], "user");
    assert!(!messages[2].to_string().contains(SKILL_TEXT));
}

#[tokio::test]
async fn responses_keeps_the_prompt_across_a_continuation_boundary() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let adapter = rustx::model::OpenAiResponsesAdapter::new(
        rustx::model::OpenAiAdapterConfig::new("test-key", server.url("/v1")),
    );
    let mut request = request(ModelProtocol::OpenAiResponses);
    request.continuation = Some(
        rustx::runtime::continuation::ProviderContinuationState::OpenAiResponses(
            rustx::runtime::continuation::OpenAiResponsesContinuation::Stored {
                previous_response_id: "resp_1".to_owned(),
            },
        ),
    );
    let events = common::collect_events(&adapter, request).await;
    assert!(matches!(
        events.last(),
        Some(rustx::model::ModelEvent::Completed { .. })
    ));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert_eq!(body["previous_response_id"], "resp_1");
    assert_eq!(
        body["instructions"],
        format!("{SYSTEM_TEXT}\n\n{SKILL_TEXT}")
    );
}

#[test]
fn effective_prompt_is_part_of_projection_measurement() {
    let engine = rustx::context::ContextEngine::new(
        rustx::context::ContextConfig {
            context_window_tokens: 2_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        std::sync::Arc::new(rustx::context::DefaultTokenEstimator),
    )
    .expect("engine");
    let state = rustx::conversation::ConversationState::new();
    let without = engine
        .build_projection(&state, &[], None, "")
        .expect("projection");
    let with_prompt = engine
        .build_projection(&state, &[], None, SKILL_TEXT)
        .expect("projection");
    assert!(with_prompt.estimated_input.input_tokens > without.estimated_input.input_tokens);
    assert_ne!(with_prompt.fingerprint(), without.fingerprint());
}

#[test]
fn request_snapshot_reconstructs_exactly_after_live_state_changes() {
    let mut conversation = rustx::conversation::ConversationState::new();
    conversation
        .commit(MessageBlock::User(UserMessageBlock {
            id: MessageId::new("historical-user"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "historical input".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }))
        .expect("commit historical message");
    let historical_revision = conversation.revision();
    let snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: AttemptId::new("attempt-snapshot"),
            turn: TurnId::new("turn-1"),
            retry_number: 0,
        },
        historical_revision,
        "frozen effective system prompt".to_owned(),
        common::invocation(ModelProtocol::OpenAiChatCompletions, "frozen-model"),
        128_000,
        None,
        false,
        Vec::new(),
        CapabilityRevision::new(7),
        rustx::context::ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
    );
    let expected = snapshot
        .reconstruct(&conversation)
        .expect("historical request reconstructs");

    conversation
        .commit(MessageBlock::User(UserMessageBlock {
            id: MessageId::new("live-user"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "live state must not leak backward".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }))
        .expect("commit later live message");
    assert_ne!(conversation.revision(), historical_revision);

    let rebuilt = snapshot
        .reconstruct(&conversation)
        .expect("historical request remains reconstructable");
    assert_eq!(rebuilt, expected);
    assert_eq!(rebuilt.messages.len(), 1);
    assert_eq!(rebuilt.messages[0], expected.messages[0]);

    let encoded = serde_json::to_string(&snapshot).expect("serialize snapshot");
    let decoded: RequestSnapshot = serde_json::from_str(&encoded).expect("deserialize snapshot");
    assert_eq!(decoded, snapshot, "the frozen boundary is reconstructable");
}
