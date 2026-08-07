//! Opt-in live integration tests for all three protocols.
//!
//! These tests require provider credentials and network access and are
//! therefore `#[ignore]`d: the normal CI command `cargo test --all-targets
//! --all-features` never executes them. Run them explicitly with:
//!
//! ```text
//! OPENAI_API_KEY=... ANTHROPIC_API_KEY=... cargo test --test m2_live -- --ignored
//! ```
//!
//! Model names are read from `RUSTX_OPENAI_CHAT_MODEL`,
//! `RUSTX_OPENAI_RESPONSES_MODEL`, and `RUSTX_ANTHROPIC_MODEL` (the latter
//! explicitly required). The `OpenAI`
//! tests fall back to a conservative default; the Anthropic test requires
//! `RUSTX_ANTHROPIC_MODEL` explicitly, because no single model can be
//! assumed to accept the request configuration (adaptive thinking plus
//! `output_config.effort`) — a missing model is skipped and reported, never
//! silently assumed. Deterministic fixture tests remain the authoritative
//! correctness tests.

use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ModelAdapter, ModelEvent, ModelProtocol,
    ModelRequest, OpenAiAdapterConfig, OpenAiChatCompletionsAdapter, OpenAiResponsesAdapter,
    ReasoningEffort, ResponsesStorageMode,
};

fn env_or(name: &str, default: Option<&str>) -> Option<String> {
    std::env::var(name)
        .ok()
        .or_else(|| default.map(str::to_owned))
}

fn openai_key() -> Option<String> {
    env_or("OPENAI_API_KEY", None)
}

fn anthropic_key() -> Option<String> {
    env_or("ANTHROPIC_API_KEY", None)
}

fn live_request(protocol: ModelProtocol, model: &str) -> ModelRequest {
    use rustx::message::content::TextBlock;
    use rustx::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use rustx::runtime::identity::MessageId;
    ModelRequest {
        model: model.to_owned(),
        protocol,
        messages: vec![MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-live-1"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Reply with exactly the word: hello".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
        })],
        tools: Vec::new(),
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 256,
        continuation: None,
    }
}

/// Runs one live invocation and asserts the required lifecycle: Started,
/// normalized content, and a legitimate terminal event.
async fn run_live(adapter: &dyn ModelAdapter, request: ModelRequest) {
    use futures_util::StreamExt;
    let cancellation = rustx::model::ModelCancellation::new();
    let mut stream = adapter.stream(request, cancellation);
    let mut started = false;
    let mut saw_content = false;
    let mut terminal = None;
    while let Some(event) = stream.next().await {
        match &event {
            ModelEvent::Started => started = true,
            ModelEvent::TextDelta { .. }
            | ModelEvent::ReasoningDelta { .. }
            | ModelEvent::RefusalDelta { .. }
            | ModelEvent::ToolCallStarted { .. } => saw_content = true,
            ModelEvent::Completed { .. } | ModelEvent::Failed { .. } => {
                assert!(terminal.is_none(), "exactly one terminal event");
                terminal = Some(event);
            }
            ModelEvent::UsageUpdate { .. }
            | ModelEvent::ToolCallArgumentsDelta { .. }
            | ModelEvent::ToolCallCompleted { .. }
            | ModelEvent::ContinuationState { .. } => {}
        }
    }
    assert!(started, "lifecycle must start with Started");
    assert!(saw_content, "lifecycle must emit normalized content");
    let terminal = terminal.expect("lifecycle must end with a terminal event");
    match terminal {
        ModelEvent::Completed { .. } => {}
        ModelEvent::Failed { error } => panic!("live invocation failed: {error:?}"),
        _ => unreachable!(),
    }
}

/// Live `OpenAI` Chat Completions.
#[tokio::test]
#[ignore = "requires OPENAI_API_KEY and network access"]
async fn live_openai_chat() {
    let Some(key) = openai_key() else {
        eprintln!("skipping live_openai_chat: OPENAI_API_KEY is not set");
        return;
    };
    let model = env_or("RUSTX_OPENAI_CHAT_MODEL", Some("gpt-5-mini")).expect("model");
    let adapter = OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new(key));
    run_live(
        &adapter,
        live_request(ModelProtocol::OpenAiChatCompletions, &model),
    )
    .await;
}

/// Live `OpenAI` Responses (Stored mode).
#[tokio::test]
#[ignore = "requires OPENAI_API_KEY and network access"]
async fn live_openai_responses() {
    let Some(key) = openai_key() else {
        eprintln!("skipping live_openai_responses: OPENAI_API_KEY is not set");
        return;
    };
    let model = env_or("RUSTX_OPENAI_RESPONSES_MODEL", Some("gpt-5-mini")).expect("model");
    let adapter = OpenAiResponsesAdapter::new(
        OpenAiAdapterConfig::new(key).with_responses_storage(ResponsesStorageMode::Stored),
    );
    run_live(
        &adapter,
        live_request(ModelProtocol::OpenAiResponses, &model),
    )
    .await;
}

/// Live `OpenAI` Responses (Stateless mode).
#[tokio::test]
#[ignore = "requires OPENAI_API_KEY and network access"]
async fn live_openai_responses_stateless() {
    let Some(key) = openai_key() else {
        eprintln!("skipping live_openai_responses_stateless: OPENAI_API_KEY is not set");
        return;
    };
    let model = env_or("RUSTX_OPENAI_RESPONSES_MODEL", Some("gpt-5-mini")).expect("model");
    let adapter = OpenAiResponsesAdapter::new(
        OpenAiAdapterConfig::new(key).with_responses_storage(ResponsesStorageMode::Stateless),
    );
    run_live(
        &adapter,
        live_request(ModelProtocol::OpenAiResponses, &model),
    )
    .await;
}

/// Live `Anthropic` Messages.
///
/// The model must be supplied explicitly via `RUSTX_ANTHROPIC_MODEL`: the
/// request shape (adaptive thinking + `output_config.effort`) is not
/// supported by every model, so no model is silently assumed. When the
/// variable is missing the test skips and reports that clearly; it never
/// claims to have passed.
#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY, RUSTX_ANTHROPIC_MODEL, and network access"]
async fn live_anthropic_messages() {
    let Some(key) = anthropic_key() else {
        eprintln!("skipping live_anthropic_messages: ANTHROPIC_API_KEY is not set");
        return;
    };
    let Some(model) = env_or("RUSTX_ANTHROPIC_MODEL", None) else {
        eprintln!(
            "skipping live_anthropic_messages: RUSTX_ANTHROPIC_MODEL is not set; \
             no model is silently assumed for the adaptive-thinking request shape"
        );
        return;
    };
    let adapter = AnthropicMessagesAdapter::new(AnthropicAdapterConfig::new(key));
    run_live(
        &adapter,
        live_request(ModelProtocol::AnthropicMessages, &model),
    )
    .await;
}

/// Live tool-call check for `OpenAI` Chat Completions (reasonably stable).
#[tokio::test]
#[ignore = "requires OPENAI_API_KEY and network access"]
async fn live_openai_chat_tool_call() {
    use futures_util::StreamExt;
    use rustx::message::content::TextBlock;
    use rustx::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use rustx::runtime::identity::{MessageId, ToolId};
    use rustx::tools::types::{ToolDefinition, ToolExecutionMode, ToolOrigin, ToolReplayPolicy};

    let Some(key) = openai_key() else {
        eprintln!("skipping live_openai_chat_tool_call: OPENAI_API_KEY is not set");
        return;
    };
    let model = env_or("RUSTX_OPENAI_CHAT_MODEL", Some("gpt-5-mini")).expect("model");
    let request = ModelRequest {
        model,
        protocol: ModelProtocol::OpenAiChatCompletions,
        messages: vec![MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-live-2"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Use the get_weather tool for location 'Berlin' and report its result."
                    .to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
        })],
        tools: vec![ToolDefinition {
            id: ToolId::new("tool-weather"),
            name: "get_weather".to_owned(),
            description: "Get the current weather for a location".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"location": {"type": "string"}},
                "required": ["location"],
            }),
            execution_mode: ToolExecutionMode::Sequential,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        }],
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 256,
        continuation: None,
    };
    let adapter = OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new(key));
    let cancellation = rustx::model::ModelCancellation::new();
    let mut stream = adapter.stream(request, cancellation);
    let mut saw_tool_call = false;
    let mut terminal = None;
    while let Some(event) = stream.next().await {
        if matches!(event, ModelEvent::ToolCallStarted { .. }) {
            saw_tool_call = true;
        }
        if matches!(
            event,
            ModelEvent::Completed { .. } | ModelEvent::Failed { .. }
        ) {
            terminal = Some(event);
        }
    }
    assert!(saw_tool_call, "the model was expected to emit a tool call");
    assert!(matches!(terminal, Some(ModelEvent::Completed { .. })));
}
