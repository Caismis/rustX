//! Issue #42: opaque provider request parameters, runtime-protected wire
//! keys, and effective capabilities at the real adapter boundary.
//!
//! Every wire assertion runs against a local deterministic fixture HTTP
//! server and inspects the **exact request body the adapter sent**, so a
//! claim about "what reaches the provider" is never inferred from an
//! intermediate Rust value.

mod common;

use rustx::model::catalog::{ChatMaxTokensField, ChatStreamUsage, Modality, ResponsesStorageMode};
use rustx::model::invocation::{
    RequestParamsLayer, adapter_capabilities, effective_capabilities, finalize_provider_request,
    overlay_shallow, validate_request_params_layer,
};
use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ModelAdapter, ModelCapabilities, ModelCompat,
    ModelErrorKind, ModelEvent, ModelProtocol, ModelRequest, OpenAiAdapterConfig,
    OpenAiChatCompletionsAdapter, OpenAiResponsesAdapter, RequestParams,
};

/// The three protocols under test, with the adapter and success fixture each
/// one needs.
enum Protocol {
    Chat,
    Responses,
    Anthropic,
}

impl Protocol {
    const ALL: [Self; 3] = [Self::Chat, Self::Responses, Self::Anthropic];

    fn model_protocol(&self) -> ModelProtocol {
        match self {
            Self::Chat => ModelProtocol::OpenAiChatCompletions,
            Self::Responses => ModelProtocol::OpenAiResponses,
            Self::Anthropic => ModelProtocol::AnthropicMessages,
        }
    }

    /// The success fixture directory/name of this protocol.
    const fn fixture(&self) -> (&'static str, &'static str) {
        match self {
            Self::Chat => ("openai_chat", "plain_text.sse"),
            Self::Responses => ("openai_responses", "plain_text.sse"),
            Self::Anthropic => ("anthropic", "text.sse"),
        }
    }

    /// A fixture server replying with this protocol's success stream.
    async fn server(&self) -> common::FixtureServer {
        let (dir, name) = self.fixture();
        common::FixtureServer::start(move |_attempt, _head| common::sse_fixture(dir, name)).await
    }

    fn adapter(&self, server: &common::FixtureServer) -> Box<dyn ModelAdapter> {
        match self {
            Self::Chat => Box::new(OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new(
                "fixture-key",
                server.url("/v1"),
            ))),
            Self::Responses => Box::new(OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new(
                "fixture-key",
                server.url("/v1"),
            ))),
            Self::Anthropic => Box::new(AnthropicMessagesAdapter::new(
                AnthropicAdapterConfig::new("fixture-key", server.url("")),
            )),
        }
    }
}

fn params(value: serde_json::Value) -> RequestParams {
    common::request_params(value)
}

/// Sends one request with the given effective request parameters and returns
/// the exact JSON body the provider received.
async fn wire_body(protocol: &Protocol, request_params: RequestParams) -> serde_json::Value {
    let server = protocol.server().await;
    let mut request = common::simple_request(protocol.model_protocol(), "wire-test", "hi");
    request.invocation.request_params = request_params;
    let events = common::collect_events(protocol.adapter(&server).as_ref(), request).await;
    assert!(
        matches!(events.last(), Some(ModelEvent::Completed { .. })),
        "{} must complete: {}",
        server.request_body(0),
        common::describe_events(&events)
    );
    serde_json::from_str(&server.request_body(0)).expect("the provider request body is JSON")
}

/// Arbitrary provider extension keys survive resolution and reach the final
/// wire JSON unchanged, at the request **top level**, on every protocol.
///
/// The set deliberately mixes scalars, a nested object, and an array, and
/// includes keys rustX has never heard of: recognizing a new provider
/// parameter must never require a release.
#[tokio::test]
async fn extension_keys_reach_the_wire_unchanged_on_every_protocol() {
    let extension = serde_json::json!({
        "temperature": 0.7,
        "top_p": 0.95,
        "top_k": 40,
        "min_p": 0.05,
        "repetition_penalty": 1.1,
        "chat_template_kwargs": {"enable_thinking": false},
        "provider": {"order": ["alpha", "beta"], "allow_fallbacks": false},
        "stop": ["END", "STOP"],
        "some_future_provider_knob": "value"
    });
    for protocol in Protocol::ALL {
        let body = wire_body(&protocol, params(extension.clone())).await;
        for (key, value) in extension.as_object().expect("object") {
            assert_eq!(
                &body[key],
                value,
                "{key} must reach the {:?} wire unchanged",
                protocol.model_protocol()
            );
        }
        assert!(
            body.get("extra_body").is_none(),
            "there is no invented extra_body nesting level"
        );
    }
}

/// Nested objects are replaced atomically at the top level, never
/// deep-merged, and arrays are atomic values.
///
/// This is asserted on the *resolution* primitive and again on the wire, so
/// the guarantee is not merely a property of one adapter.
#[tokio::test]
async fn overlay_is_shallow_and_atomic_end_to_end() {
    let mut base = params(serde_json::json!({
        "routing": {"a": 1, "b": 2},
        "stop": ["a", "b", "c"],
        "keep": true
    }));
    overlay_shallow(
        &mut base,
        &params(serde_json::json!({"routing": {"c": 3}, "stop": ["z"]})),
    );
    assert_eq!(
        serde_json::Value::Object(base.clone()),
        serde_json::json!({"routing": {"c": 3}, "stop": ["z"], "keep": true}),
        "a nested object is replaced whole and an array is atomic"
    );

    let body = wire_body(&Protocol::Chat, base).await;
    assert_eq!(body["routing"], serde_json::json!({"c": 3}));
    assert_eq!(body["stop"], serde_json::json!(["z"]));
    assert_eq!(body["keep"], serde_json::json!(true));
}

/// The runtime-owned structural fields survive an aggressive extension
/// overlay: the effective parameters are applied *after* translation and can
/// never displace them.
#[tokio::test]
async fn runtime_owned_fields_survive_the_overlay() {
    let body = wire_body(
        &Protocol::Responses,
        params(serde_json::json!({"temperature": 0.1, "reasoning": {"effort": "high"}})),
    )
    .await;
    assert_eq!(body["model"], "wire-test");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_output_tokens"], 512);
    assert!(body["input"].is_array());
    // The provider-owned reasoning object is *not* protected: a profile owns
    // it, and it arrives exactly as configured.
    assert_eq!(body["reasoning"], serde_json::json!({"effort": "high"}));
}

/// A protected-key collision fails deterministically at **every**
/// configuration layer, and the failure names the layer.
#[test]
fn protected_key_collisions_fail_at_every_layer() {
    let layers = [
        RequestParamsLayer::ModelDefaults,
        RequestParamsLayer::ReasoningProfile,
        RequestParamsLayer::SessionOverrides,
        RequestParamsLayer::SummaryOverrides,
    ];
    let cases = [
        (ModelProtocol::OpenAiChatCompletions, "messages"),
        (ModelProtocol::OpenAiChatCompletions, "max_tokens"),
        (
            ModelProtocol::OpenAiChatCompletions,
            "max_completion_tokens",
        ),
        (ModelProtocol::OpenAiChatCompletions, "stream_options"),
        (ModelProtocol::OpenAiResponses, "store"),
        (ModelProtocol::OpenAiResponses, "previous_response_id"),
        (ModelProtocol::OpenAiResponses, "include"),
        (ModelProtocol::OpenAiResponses, "instructions"),
        (ModelProtocol::AnthropicMessages, "system"),
        (ModelProtocol::AnthropicMessages, "max_tokens"),
    ];
    for layer in layers {
        for (protocol, key) in cases {
            let value = params(serde_json::json!({key: "hijack"}));
            let error = validate_request_params_layer(&value, protocol, layer)
                .expect_err("a protected key must be rejected");
            assert_eq!(error.key, key);
            assert_eq!(error.layer, layer);
        }
    }
}

/// Defence in depth: even if an invalid internal state produced effective
/// parameters carrying a protected key, final request construction refuses
/// rather than overwriting the runtime field.
#[tokio::test]
async fn final_construction_refuses_a_protected_key() {
    for protocol in Protocol::ALL {
        let key = match protocol.model_protocol() {
            ModelProtocol::OpenAiChatCompletions | ModelProtocol::AnthropicMessages => "messages",
            ModelProtocol::OpenAiResponses => "input",
        };
        let error = finalize_provider_request(
            serde_json::json!({"model": "m", key: []}),
            &params(serde_json::json!({key: "hijack"})),
            protocol.model_protocol(),
        )
        .expect_err("a protected key must be refused at construction");
        assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
        assert!(error.message.contains(key));
    }
}

/// The Chat Completions compat metadata selects exactly one max-token
/// spelling, and the other spelling never appears.
///
/// Because both spellings are protected, a request parameter can never add
/// the contradictory second field.
#[tokio::test]
async fn chat_compat_selects_one_max_token_spelling() {
    for (field, present, absent) in [
        (
            ChatMaxTokensField::MaxCompletionTokens,
            "max_completion_tokens",
            "max_tokens",
        ),
        (
            ChatMaxTokensField::MaxTokens,
            "max_tokens",
            "max_completion_tokens",
        ),
    ] {
        let server = common::FixtureServer::start(|_attempt, _head| {
            common::sse_fixture("openai_chat", "plain_text.sse")
        })
        .await;
        let mut request =
            common::simple_request(ModelProtocol::OpenAiChatCompletions, "wire-test", "hi");
        request.invocation.compat.chat_max_tokens_field = field;
        let adapter = OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new(
            "fixture-key",
            server.url("/v1"),
        ));
        let events = common::collect_events(&adapter, request).await;
        assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
        let body: serde_json::Value =
            serde_json::from_str(&server.request_body(0)).expect("JSON body");
        assert_eq!(body[present], 512, "{field:?} writes {present}");
        assert!(
            body.get(absent).is_none(),
            "{field:?} must never also write {absent}: {body}"
        );
    }
}

/// The Chat Completions compat metadata can suppress stream options for a
/// service that rejects them.
#[tokio::test]
async fn chat_compat_can_suppress_stream_usage_options() {
    for (usage, expect_options) in [
        (ChatStreamUsage::Supported, true),
        (ChatStreamUsage::Unsupported, false),
    ] {
        let server = common::FixtureServer::start(|_attempt, _head| {
            common::sse_fixture("openai_chat", "plain_text.sse")
        })
        .await;
        let mut request =
            common::simple_request(ModelProtocol::OpenAiChatCompletions, "wire-test", "hi");
        request.invocation.compat.chat_stream_usage = usage;
        let adapter = OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new(
            "fixture-key",
            server.url("/v1"),
        ));
        let events = common::collect_events(&adapter, request).await;
        assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
        let body: serde_json::Value =
            serde_json::from_str(&server.request_body(0)).expect("JSON body");
        assert_eq!(
            body.get("stream_options").is_some(),
            expect_options,
            "{usage:?} -> {body}"
        );
    }
}

/// The Responses compat metadata owns the storage/continuation structure,
/// including the `include` value that makes stateless replay possible.
#[tokio::test]
async fn responses_compat_owns_storage_structure() {
    for (mode, stored) in [
        (ResponsesStorageMode::Stored, true),
        (ResponsesStorageMode::Stateless, false),
    ] {
        let server = common::FixtureServer::start(|_attempt, _head| {
            common::sse_fixture("openai_responses", "plain_text.sse")
        })
        .await;
        let mut request = common::simple_request(ModelProtocol::OpenAiResponses, "wire-test", "hi");
        request.invocation.compat.responses_storage = mode;
        let adapter =
            OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new("fixture-key", server.url("/v1")));
        let events = common::collect_events(&adapter, request).await;
        assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
        let body: serde_json::Value =
            serde_json::from_str(&server.request_body(0)).expect("JSON body");
        assert_eq!(body["store"], stored);
        assert_eq!(
            body.get("include").is_some(),
            !stored,
            "stateless replay requires the encrypted-reasoning include value: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Effective capabilities
// ---------------------------------------------------------------------------

/// A raw catalog claim is intersected with the adapter/protocol capability
/// and the current runtime capability; image input is never advertised
/// because no adapter can transmit it yet, and canonical file input resolves
/// as unsupported.
#[test]
fn effective_capabilities_intersect_the_raw_claim() {
    let generous = ModelCapabilities {
        input_modalities: [Modality::Text, Modality::Image, Modality::File]
            .into_iter()
            .collect(),
        output_modalities: [Modality::Text, Modality::Image].into_iter().collect(),
        tool_calls: true,
        reasoning: true,
    };
    for protocol in [
        ModelProtocol::OpenAiChatCompletions,
        ModelProtocol::OpenAiResponses,
        ModelProtocol::AnthropicMessages,
    ] {
        let effective = effective_capabilities(&generous, protocol);
        assert!(effective.input_modalities.contains(&Modality::Text));
        assert!(
            !effective.input_modalities.contains(&Modality::Image),
            "image input must not be advertised while no adapter can transmit it"
        );
        assert!(
            !effective.input_modalities.contains(&Modality::File),
            "canonical file input must resolve as unsupported"
        );
        assert_eq!(
            effective,
            adapter_capabilities(protocol).intersect(&generous)
        );
    }

    // The intersection can only narrow: a model that claims no tool calls
    // stays without tool calls.
    let restricted = ModelCapabilities::text_only(false, false);
    let effective = effective_capabilities(&restricted, ModelProtocol::OpenAiChatCompletions);
    assert!(!effective.tool_calls);
    assert!(!effective.reasoning);
}

/// Unsupported canonical input is rejected **before** the provider request
/// attempt: the fixture server's attempt counter never increments.
#[tokio::test]
async fn unsupported_input_is_rejected_before_any_provider_request() {
    use rustx::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use rustx::runtime::identity::MessageId;

    for protocol in Protocol::ALL {
        let server = protocol.server().await;
        let mut request = common::simple_request(protocol.model_protocol(), "wire-test", "hi");
        request.messages = vec![MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-image-1"),
            content: vec![UserContentBlock::Image(
                rustx::message::content::ImageReference {
                    artifact_id: rustx::runtime::identity::ArtifactId::new("artifact-1"),
                    alt: None,
                },
            )],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })];
        let events = common::collect_events(protocol.adapter(&server).as_ref(), request).await;
        assert_eq!(
            events.len(),
            1,
            "a rejected request emits only its terminal"
        );
        match events.first() {
            Some(ModelEvent::Failed { error }) => {
                assert_eq!(error.kind, ModelErrorKind::Unsupported);
            }
            other => panic!("expected a terminal Failed, got {other:?}"),
        }
        assert_eq!(
            server.attempt_count(),
            0,
            "the provider must never be the first validator of a known mismatch"
        );
    }
}

/// A model without effective tool-call capability stays usable as a text
/// model, and tool definitions are refused rather than silently sent.
#[tokio::test]
async fn a_text_only_model_is_usable_and_never_receives_tools() {
    for protocol in Protocol::ALL {
        // A plain text request succeeds.
        let server = protocol.server().await;
        let mut request = common::simple_request(protocol.model_protocol(), "wire-test", "hi");
        request.invocation.capabilities = ModelCapabilities::text_only(false, false);
        let events = common::collect_events(protocol.adapter(&server).as_ref(), request).await;
        assert!(
            matches!(events.last(), Some(ModelEvent::Completed { .. })),
            "a text-only model remains usable"
        );
        let body: serde_json::Value =
            serde_json::from_str(&server.request_body(0)).expect("JSON body");
        assert!(body.get("tools").is_none(), "no tool definitions are sent");

        // Supplying tool definitions to it is refused before the network.
        let server = protocol.server().await;
        let mut request = common::simple_request(protocol.model_protocol(), "wire-test", "hi");
        request.invocation.capabilities = ModelCapabilities::text_only(false, false);
        request.tools = vec![common::model_tool("list_directory", "tool-list")];
        let events = common::collect_events(protocol.adapter(&server).as_ref(), request).await;
        assert_eq!(events.len(), 1);
        match events.first() {
            Some(ModelEvent::Failed { error }) => {
                assert_eq!(error.kind, ModelErrorKind::Unsupported);
            }
            other => panic!("expected a terminal Failed, got {other:?}"),
        }
        assert_eq!(server.attempt_count(), 0);
    }
}

/// The invocation configuration carries no credential material by
/// construction: it is a serializable provider-neutral value, and the
/// compat/capability pieces round-trip exactly.
#[test]
fn the_invocation_configuration_is_credential_free_and_serializable() {
    let invocation = rustx::model::ModelInvocationConfig {
        model: "wire-test".to_owned(),
        protocol: ModelProtocol::AnthropicMessages,
        max_output_tokens: 256,
        request_params: params(serde_json::json!({"temperature": 0.2})),
        capabilities: ModelCapabilities::text_only(true, true),
        compat: ModelCompat::default(),
    };
    let json = serde_json::to_string(&invocation).expect("serialize");
    assert!(!json.contains("api_key") && !json.contains("apiKey"));
    let decoded: rustx::model::ModelInvocationConfig =
        serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, invocation);

    let request = ModelRequest {
        invocation,
        messages: Vec::new(),
        tools: Vec::new(),
        agent_status: None,
        skill_catalog: None,
        continuation: None,
    };
    assert_eq!(request.max_output_tokens(), 256);
    assert_eq!(request.model(), "wire-test");
}
