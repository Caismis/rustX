//! M6 deterministic tests: the Skill catalog attachment in model context
//! and trusted system wire placement.
//!
//! Every provider test runs against the local fixture server: no provider
//! network access.

use rustx::message::content::TextBlock;
use rustx::message::types::{
    AgentMessageBlock, InboundKind, MessageBlock, SystemAuthority, SystemMessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::types::{ModelProtocol, ModelRequest, SkillCatalogAttachment};
use rustx::runtime::identity::MessageId;

#[path = "common/mod.rs"]
mod common;

const CATALOG: &str = "## Skills\n\n- pdf: Create, edit, inspect, and transform PDF documents.\n";
const SYSTEM_TEXT: &str = "You are a helpful agent.";

/// A canonical request with trusted system content, one agent turn, and the
/// Skill catalog attachment.
fn request(protocol: ModelProtocol, with_catalog: bool) -> ModelRequest {
    ModelRequest {
        model: "m6-test".to_owned(),
        protocol,
        messages: vec![
            MessageBlock::System(SystemMessageBlock {
                id: MessageId::new("msg-system-1"),
                authority: SystemAuthority::Runtime,
                content: vec![TextBlock {
                    text: SYSTEM_TEXT.to_owned(),
                }],
            }),
            MessageBlock::Agent(AgentMessageBlock {
                id: MessageId::new("msg-agent-1"),
                content: vec![rustx::message::types::AgentContentBlock::Text(TextBlock {
                    text: "earlier turn".to_owned(),
                })],
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
        agent_status: None,
        skill_catalog: with_catalog.then(|| SkillCatalogAttachment {
            rendered: CATALOG.to_owned(),
        }),
        reasoning: rustx::model::ReasoningEffort::Medium,
        max_output_tokens: 512,
        continuation: None,
    }
}

/// The Anthropic adapter places the catalog in the top-level `system`
/// content along with canonical trusted system content.
#[tokio::test]
async fn anthropic_places_the_catalog_in_top_level_system_content() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("anthropic", "text.sse")
    })
    .await;
    let adapter = rustx::model::AnthropicMessagesAdapter::new(
        rustx::model::AnthropicAdapterConfig::new("test-key").with_api_base(server.url("")),
    );
    let events = common::collect_events(&adapter, request(ModelProtocol::AnthropicMessages, true))
        .await;
    assert!(matches!(events.last(), Some(rustx::model::ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let system = body
        .get("system")
        .expect("top-level system content")
        .as_array()
        .expect("system array");
    let texts: Vec<&str> = system
        .iter()
        .map(|block| block.get("text").expect("text").as_str().expect("string"))
        .collect();
    assert_eq!(
        texts,
        vec![SYSTEM_TEXT, CATALOG],
        "the catalog follows the canonical trusted system content"
    );
    // The catalog never appears inside a user message.
    let messages_text = serde_json::to_string(body.get("messages").expect("messages"))
        .expect("serialize");
    assert!(
        !messages_text.contains(CATALOG),
        "the catalog is never attached to a user message"
    );
}

/// Without an active Skill set the catalog attachment is absent entirely
/// and the Anthropic system content carries only the canonical blocks.
#[tokio::test]
async fn anthropic_omits_the_catalog_when_no_skill_is_active() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("anthropic", "text.sse")
    })
    .await;
    let adapter = rustx::model::AnthropicMessagesAdapter::new(
        rustx::model::AnthropicAdapterConfig::new("test-key").with_api_base(server.url("")),
    );
    let events = common::collect_events(&adapter, request(ModelProtocol::AnthropicMessages, false))
        .await;
    assert!(matches!(events.last(), Some(rustx::model::ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let system = body.get("system").expect("system").as_array().expect("array");
    let texts: Vec<&str> = system
        .iter()
        .map(|block| block.get("text").expect("text").as_str().expect("string"))
        .collect();
    assert_eq!(texts, vec![SYSTEM_TEXT]);
}

/// The OpenAI Chat Completions adapter translates the catalog through the
/// system-level message mechanism, never attached to a user message.
#[tokio::test]
async fn chat_completions_places_the_catalog_in_a_system_message() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let adapter = rustx::model::OpenAiChatCompletionsAdapter::new(
        rustx::model::OpenAiAdapterConfig::new("test-key").with_api_base(server.url("/v1")),
    );
    let events =
        common::collect_events(&adapter, request(ModelProtocol::OpenAiChatCompletions, true))
            .await;
    assert!(matches!(events.last(), Some(rustx::model::ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let messages = body.get("messages").expect("messages").as_array().expect("array");
    let system_messages: Vec<&str> = messages
        .iter()
        .filter(|message| message.get("role") == Some(&serde_json::json!("system")))
        .map(|message| message.get("content").expect("content").as_str().expect("text"))
        .collect();
    assert_eq!(
        system_messages,
        vec![SYSTEM_TEXT, CATALOG],
        "the catalog is a system message after the canonical system messages"
    );
    let user_messages_text = serde_json::to_string(
        &messages
            .iter()
            .filter(|message| message.get("role") == Some(&serde_json::json!("user")))
            .collect::<Vec<_>>(),
    )
    .expect("serialize");
    assert!(
        !user_messages_text.contains(CATALOG),
        "the catalog is never attached to a user message"
    );
}

/// The OpenAI Responses adapter combines the catalog with the canonical
/// system instructions in the trusted `instructions` channel.
#[tokio::test]
async fn responses_places_the_catalog_in_the_instructions_channel() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let adapter = rustx::model::OpenAiResponsesAdapter::new(
        rustx::model::OpenAiAdapterConfig::new("test-key")
            .with_api_base(server.url("/v1"))
            .with_responses_storage(rustx::model::ResponsesStorageMode::Stored),
    );
    let events =
        common::collect_events(&adapter, request(ModelProtocol::OpenAiResponses, true)).await;
    assert!(matches!(events.last(), Some(rustx::model::ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let instructions = body
        .get("instructions")
        .expect("instructions channel")
        .as_str()
        .expect("string");
    assert!(
        instructions.contains(SYSTEM_TEXT) && instructions.contains(CATALOG),
        "the instructions channel combines canonical system instructions with the catalog"
    );
}

/// A Responses stored continuation still sends the catalog: the catalog is
/// rebuilt from the request attachment on every request and never
/// disappears because canonical history before the continuation boundary
/// was sliced away.
#[tokio::test]
async fn responses_continuation_keeps_sending_the_catalog() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        common::sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let adapter = rustx::model::OpenAiResponsesAdapter::new(
        rustx::model::OpenAiAdapterConfig::new("test-key")
            .with_api_base(server.url("/v1"))
            .with_responses_storage(rustx::model::ResponsesStorageMode::Stored),
    );
    let mut request = request(ModelProtocol::OpenAiResponses, true);
    request.continuation = Some(rustx::runtime::continuation::ProviderContinuationState::OpenAiResponses(
        rustx::runtime::continuation::OpenAiResponsesContinuation::Stored {
            previous_response_id: "resp_1".to_owned(),
        },
    ));
    let events = common::collect_events(&adapter, request).await;
    assert!(matches!(events.last(), Some(rustx::model::ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert_eq!(
        body.get("previous_response_id"),
        Some(&serde_json::json!("resp_1")),
        "the stored continuation is sent"
    );
    // The input items are only the continuation tail (system and agent
    // history are sliced away)...
    let input_text = serde_json::to_string(body.get("input").expect("input")).expect("serialize");
    assert!(
        !input_text.contains(SYSTEM_TEXT),
        "canonical system history is sliced away in the tail"
    );
    // ...but the catalog survives in the instructions channel.
    let instructions = body
        .get("instructions")
        .expect("instructions channel")
        .as_str()
        .expect("string");
    assert!(
        instructions.contains(CATALOG),
        "the catalog is re-sent with every continuation request even though          the canonical system history was sliced away"
    );
}

/// A large Skill catalog contributes to `CannotFit`: the hard-fit
/// calculation includes the exact catalog attachment on both sides of the
/// compaction progress comparison.
#[test]
fn large_catalog_contributes_to_cannot_fit() {
    use rustx::context::{ContextConfig, ContextEngine, ContextRuntime};
    let estimator: std::sync::Arc<dyn rustx::context::TokenEstimator> =
        std::sync::Arc::new(rustx::context::DefaultTokenEstimator);
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 2000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator.clone(),
    )
    .expect("engine");
    let history = vec![MessageBlock::User(UserMessageBlock {
        id: MessageId::new("msg-u1"),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "hello".to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })];
    let catalog = SkillCatalogAttachment {
        rendered: "## Skills\n".repeat(5000),
    };
    let with_catalog = engine
        .build_projection(&history, None, &[], None, None, Some(&catalog))
        .expect("projection");
    let soft_limit = engine.soft_input_limit(512).expect("soft limit");
    assert!(
        with_catalog.estimated_input.input_tokens > soft_limit,
        "a large catalog must exceed the soft limit"
    );
    assert!(
        engine.should_compact(&with_catalog, 512).expect("threshold"),
        "a large catalog must trigger compaction"
    );
    assert!(
        !engine
            .fits_under_soft_limit(&with_catalog, 512)
            .expect("fit check"),
        "a large catalog can contribute to CannotFit"
    );
    let recent = estimator.estimate_conversation_input(&with_catalog);
    let without_catalog = engine
        .build_projection(&history, None, &[], None, None, None)
        .expect("projection");
    assert_eq!(
        recent,
        estimator.estimate_conversation_input(&without_catalog),
        "the catalog never counts toward keep_recent_tokens"
    );

    // Compaction planning carries the exact attachment: a plan built with
    // the catalog cannot silently drop it.
    // A plan with the huge catalog cannot fit: the catalog participates in
    // the hard-fit calculation and can therefore contribute to CannotFit.
    let plan_error = engine
        .plan_compaction(
            &history,
            None,
            &with_catalog,
            &[],
            512,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect_err("the huge catalog cannot fit");
    assert_eq!(plan_error.kind, rustx::context::ContextErrorKind::CannotFit);

    // With a moderate catalog the plan succeeds and carries the exact
    // attachment, so compaction planning and application use the same
    // catalog snapshot on both sides of the progress comparison.
    let moderate = engine
        .build_projection(&history, None, &[], None, None, Some(&catalog))
        .expect("projection");
    let _ = moderate;
    let moderate_catalog = SkillCatalogAttachment {
        rendered: "## Skills\n".to_owned(),
    };
    let with_moderate = engine
        .build_projection(&history, None, &[], None, None, Some(&moderate_catalog))
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &with_moderate,
            &[],
            512,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.skill_catalog, Some(moderate_catalog));
    let _ = ContextRuntime::new;
}
