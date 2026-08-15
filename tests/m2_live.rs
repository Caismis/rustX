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
//! Provider endpoints are read from `RUSTX_OPENAI_BASE_URL` and
//! `RUSTX_ANTHROPIC_BASE_URL`: no adapter can infer an official endpoint, so
//! a live run states the endpoint explicitly just like the catalog does.
//!
//! Model names are read from `RUSTX_OPENAI_CHAT_MODEL`,
//! `RUSTX_OPENAI_RESPONSES_MODEL`, and `RUSTX_ANTHROPIC_MODEL` (the latter
//! explicitly required). The `OpenAI` tests fall back to a conservative
//! default; the Anthropic test requires `RUSTX_ANTHROPIC_MODEL` explicitly,
//! because no single model can be assumed available — a missing model is
//! skipped and reported, never silently assumed. Deterministic fixture tests
//! remain the authoritative correctness tests.
//!
//! A live request carries **no** request parameters, so any provider field
//! that reaches the wire came from canonical translation and not from a
//! configured overlay.

use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ModelAdapter, ModelCapabilities, ModelCompat,
    ModelEvent, ModelInvocationConfig, ModelProtocol, ModelRequest, OpenAiAdapterConfig,
    OpenAiChatCompletionsAdapter, OpenAiResponsesAdapter, RequestParams, ResponsesStorageMode,
};

/// The explicit `OpenAI`-compatible endpoint of a live run.
fn openai_base_url() -> String {
    env_or("RUSTX_OPENAI_BASE_URL", Some("https://api.openai.com/v1")).expect("base url")
}

/// The explicit Anthropic-compatible endpoint of a live run.
fn anthropic_base_url() -> String {
    env_or(
        "RUSTX_ANTHROPIC_BASE_URL",
        Some("https://api.anthropic.com"),
    )
    .expect("base url")
}

/// The live invocation configuration: no request parameters, so anything a
/// live run observes on the wire came from translation, not configuration.
fn live_invocation(
    protocol: ModelProtocol,
    model: &str,
    storage: ResponsesStorageMode,
) -> ModelInvocationConfig {
    ModelInvocationConfig {
        model: model.to_owned(),
        protocol,
        max_output_tokens: 256,
        request_params: RequestParams::new(),
        capabilities: ModelCapabilities::text_only(true, true),
        compat: ModelCompat {
            responses_storage: storage,
            ..ModelCompat::default()
        },
    }
}

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
    live_request_with_storage(protocol, model, ResponsesStorageMode::Stored)
}

/// A live request with an explicit Responses storage/continuation mode.
fn live_request_with_storage(
    protocol: ModelProtocol,
    model: &str,
    storage: ResponsesStorageMode,
) -> ModelRequest {
    use rustx::message::content::TextBlock;
    use rustx::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use rustx::runtime::identity::MessageId;
    ModelRequest {
        invocation: live_invocation(protocol, model, storage),
        messages: vec![MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-live-1"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Reply with exactly the word: hello".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })],
        tools: Vec::new(),
        effective_system_prompt: String::new(),
        continuation: None,
    }
}

/// Runs one live invocation and asserts the required lifecycle: Started,
/// normalized content, and a legitimate terminal event.
async fn run_live(adapter: &dyn ModelAdapter, request: ModelRequest) {
    use futures_util::StreamExt;
    let cancellation = rustx::runtime::CancellationSignal::new();
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
    let adapter =
        OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new(key, openai_base_url()));
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
    let adapter = OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new(key, openai_base_url()));
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
    let adapter = OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new(key, openai_base_url()));
    run_live(
        &adapter,
        live_request_with_storage(
            ModelProtocol::OpenAiResponses,
            &model,
            ResponsesStorageMode::Stateless,
        ),
    )
    .await;
}

/// Live `Anthropic` Messages.
///
/// The model must be supplied explicitly via `RUSTX_ANTHROPIC_MODEL`: no
/// model is silently assumed. When the variable is missing the test skips
/// and reports that clearly; it never claims to have passed.
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
    let adapter =
        AnthropicMessagesAdapter::new(AnthropicAdapterConfig::new(key, anthropic_base_url()));
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
    use rustx::tools::types::ModelToolDefinition;

    let Some(key) = openai_key() else {
        eprintln!("skipping live_openai_chat_tool_call: OPENAI_API_KEY is not set");
        return;
    };
    let model = env_or("RUSTX_OPENAI_CHAT_MODEL", Some("gpt-5-mini")).expect("model");
    let request = ModelRequest {
        invocation: live_invocation(
            ModelProtocol::OpenAiChatCompletions,
            &model,
            ResponsesStorageMode::Stored,
        ),
        messages: vec![MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-live-2"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Use the get_weather tool for location 'Berlin' and report its result."
                    .to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })],
        tools: vec![ModelToolDefinition {
            id: ToolId::new("tool-weather"),
            name: "get_weather".to_owned(),
            description: "Get the current weather for a location".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"location": {"type": "string"}},
                "required": ["location"],
            }),
        }],
        effective_system_prompt: String::new(),
        continuation: None,
    };
    let adapter =
        OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new(key, openai_base_url()));
    let cancellation = rustx::runtime::CancellationSignal::new();
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
