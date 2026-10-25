//! Structured output admission and exact wire contracts through production adapters.

use rustx::model::structured::{PreparedResponseSchema, ResponseSchema, StructuredOutputStrategy};
use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ModelAdapter, ModelErrorKind, ModelEvent,
    ModelProtocol, ModelRequest, OpenAiAdapterConfig, OpenAiChatCompletionsAdapter,
    OpenAiResponsesAdapter,
};
use serde_json::{Value, json};

const PROTOCOLS: [ModelProtocol; 3] = [
    ModelProtocol::OpenAiChatCompletions,
    ModelProtocol::OpenAiResponses,
    ModelProtocol::AnthropicMessages,
];

fn schema() -> ResponseSchema {
    ResponseSchema::new(
        "classification".to_owned(),
        json!({
            "type": "object",
            "properties": {
                "category": {"type": "string", "enum": ["question", "feedback"]},
                "actionable": {"type": "boolean"}
            },
            "required": ["category", "actionable"],
            "additionalProperties": false
        }),
        4096,
    )
    .expect("portable response schema")
}

fn request(protocol: ModelProtocol, native: bool) -> ModelRequest {
    let mut request =
        crate::common::simple_request(protocol, "structured-fixture", "classify this");
    request.invocation.capabilities.json_schema_output = native;
    request.response_schema = Some(PreparedResponseSchema {
        strategy: if native {
            StructuredOutputStrategy::Native
        } else {
            StructuredOutputStrategy::Prompt
        },
        schema: schema(),
    });
    request
}

fn adapter(
    protocol: ModelProtocol,
    server: &crate::common::FixtureServer,
) -> Box<dyn ModelAdapter> {
    match protocol {
        ModelProtocol::OpenAiChatCompletions => Box::new(OpenAiChatCompletionsAdapter::new(
            OpenAiAdapterConfig::new("fixture-key", server.url("/v1")),
        )),
        ModelProtocol::OpenAiResponses => Box::new(OpenAiResponsesAdapter::new(
            OpenAiAdapterConfig::new("fixture-key", server.url("/v1")),
        )),
        ModelProtocol::AnthropicMessages => Box::new(AnthropicMessagesAdapter::new(
            AnthropicAdapterConfig::new("fixture-key", server.url("")),
        )),
    }
}

async fn server(protocol: ModelProtocol) -> crate::common::FixtureServer {
    crate::common::FixtureServer::start(move |_, _| match protocol {
        ModelProtocol::OpenAiChatCompletions => {
            crate::common::sse_fixture("openai_chat", "plain_text.sse")
        }
        ModelProtocol::OpenAiResponses => {
            crate::common::sse_fixture("openai_responses", "plain_text.sse")
        }
        ModelProtocol::AnthropicMessages => crate::common::sse_fixture("anthropic", "text.sse"),
    })
    .await
}

async fn send(request: ModelRequest, server: &crate::common::FixtureServer) -> Value {
    let index = server.request_bodies().len();
    let events =
        crate::common::collect_events(adapter(request.protocol(), server).as_ref(), request).await;
    assert!(
        matches!(events.last(), Some(ModelEvent::Completed { .. })),
        "{}",
        crate::common::describe_events(&events)
    );
    serde_json::from_str(&server.request_body(index)).expect("provider JSON body")
}

fn sibling_params(protocol: ModelProtocol) -> Value {
    match protocol {
        ModelProtocol::OpenAiChatCompletions => json!({"temperature": 0.2}),
        ModelProtocol::OpenAiResponses => json!({"text": {"verbosity": "low"}}),
        ModelProtocol::AnthropicMessages => json!({"output_config": {"effort": "low"}}),
    }
}

#[tokio::test]
async fn native_schema_has_exact_protocol_shape_and_preserves_configured_siblings() {
    for protocol in PROTOCOLS {
        let server = server(protocol).await;
        let mut structured = request(protocol, true);
        structured.invocation.request_params =
            crate::common::request_params(sibling_params(protocol));
        let mut text = structured.clone();
        text.response_schema = None;
        let mut expected = send(text, &server).await;
        match protocol {
            ModelProtocol::OpenAiChatCompletions => {
                expected["response_format"] = json!({
                    "type": "json_schema",
                    "json_schema": {
                        "name": "classification", "strict": true, "schema": schema().schema()
                    }
                });
            }
            ModelProtocol::OpenAiResponses => {
                expected["text"]["format"] = json!({
                    "type": "json_schema", "name": "classification",
                    "strict": true, "schema": schema().schema()
                });
            }
            ModelProtocol::AnthropicMessages => {
                expected["output_config"]["format"] = json!({
                    "type": "json_schema", "schema": schema().schema()
                });
            }
        }
        assert_eq!(send(structured, &server).await, expected, "{protocol:?}");
        assert_eq!(server.attempt_count(), 2);
    }
}

#[tokio::test]
async fn prompt_strategy_is_an_ordinary_text_wire_request() {
    for protocol in PROTOCOLS {
        let server = server(protocol).await;
        let mut structured = request(protocol, false);
        // Auxiliary owns preparation; the adapter carries the already frozen
        // trusted instruction unchanged and adds no native format parameter.
        structured.effective_system_prompt =
            "Return exactly one JSON object matching the caller's schema.".to_owned();
        structured.invocation.request_params =
            crate::common::request_params(sibling_params(protocol));
        let mut text = structured.clone();
        text.response_schema = None;
        let expected = send(text, &server).await;
        let actual = send(structured, &server).await;
        assert_eq!(actual, expected, "{protocol:?}");
        assert!(actual.get("response_format").is_none());
        assert!(actual["text"].get("format").is_none());
        assert!(actual["output_config"].get("format").is_none());
    }
}

#[tokio::test]
async fn plain_text_wire_is_identical_with_either_capability() {
    for protocol in PROTOCOLS {
        let server = server(protocol).await;
        let mut text = request(protocol, false);
        text.response_schema = None;
        text.invocation.request_params = crate::common::request_params(sibling_params(protocol));
        let expected = send(text.clone(), &server).await;
        text.invocation.capabilities.json_schema_output = true;
        assert_eq!(send(text, &server).await, expected, "{protocol:?}");
        assert!(expected.get("response_format").is_none());
        assert!(expected["text"].get("format").is_none());
        assert!(expected["output_config"].get("format").is_none());
    }
}

#[tokio::test]
async fn conflicting_format_is_rejected_before_network_for_both_strategies() {
    for protocol in PROTOCOLS {
        let server = server(protocol).await;
        for native in [false, true] {
            for format in [
                Value::Null,
                json!({"type": "json_object"}),
                json!({"type": "text"}),
            ] {
                let (params, path) = match protocol {
                    ModelProtocol::OpenAiChatCompletions => {
                        (json!({"response_format": format}), "response_format")
                    }
                    ModelProtocol::OpenAiResponses => (
                        json!({"text": {"verbosity": "low", "format": format}}),
                        "text.format",
                    ),
                    ModelProtocol::AnthropicMessages => (
                        json!({"output_config": {"effort": "low", "format": format}}),
                        "output_config.format",
                    ),
                };
                let mut request = request(protocol, native);
                request.invocation.request_params = crate::common::request_params(params);
                let events =
                    crate::common::collect_events(adapter(protocol, &server).as_ref(), request)
                        .await;
                let [ModelEvent::Failed { error }] = events.as_slice() else {
                    panic!(
                        "collision must fail before Started: {}",
                        crate::common::describe_events(&events)
                    );
                };
                assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
                assert!(error.message.contains(path));
            }
        }
        assert_eq!(server.attempt_count(), 0);
    }
}

#[tokio::test]
async fn invalid_format_parent_is_rejected_before_network() {
    for (protocol, parent) in [
        (ModelProtocol::OpenAiResponses, "text"),
        (ModelProtocol::AnthropicMessages, "output_config"),
    ] {
        let server = server(protocol).await;
        for native in [false, true] {
            for value in [Value::Null, json!([]), json!("invalid")] {
                let mut request = request(protocol, native);
                request.invocation.request_params =
                    crate::common::request_params(json!({parent: value}));
                let events =
                    crate::common::collect_events(adapter(protocol, &server).as_ref(), request)
                        .await;
                let [ModelEvent::Failed { error }] = events.as_slice() else {
                    panic!("invalid parent must fail before Started");
                };
                assert_eq!(error.kind, ModelErrorKind::InvalidRequest);
                assert!(error.message.contains(parent));
            }
        }
        assert_eq!(server.attempt_count(), 0);
    }
}

#[tokio::test]
async fn structured_strategy_must_match_frozen_effective_capability() {
    for protocol in PROTOCOLS {
        let server = server(protocol).await;
        for native in [false, true] {
            let mut request = request(protocol, native);
            request.invocation.capabilities.json_schema_output = !native;
            let events =
                crate::common::collect_events(adapter(protocol, &server).as_ref(), request).await;
            let [ModelEvent::Failed { error }] = events.as_slice() else {
                panic!("capability mismatch must fail before Started");
            };
            assert_eq!(error.kind, ModelErrorKind::Unsupported);
            assert_eq!(
                error.retry_disposition,
                rustx::model::ModelRetryDisposition::Never
            );
        }
        assert_eq!(server.attempt_count(), 0);
    }
}

#[tokio::test]
async fn provider_rejection_of_native_format_never_falls_back_or_retries() {
    for protocol in PROTOCOLS {
        let server = crate::common::FixtureServer::start(|_, _| {
            crate::common::FixtureReply::json_error(
                400,
                "Bad Request",
                &json!({
                    "type": "error",
                    "error": {
                        "type": "invalid_request_error",
                        "code": "unsupported_response_format",
                        "message": "json_schema output format is not supported"
                    }
                }),
            )
        })
        .await;
        let events = crate::common::collect_events(
            adapter(protocol, &server).as_ref(),
            request(protocol, true),
        )
        .await;
        let Some(ModelEvent::Failed { error }) = events.last() else {
            panic!("native format rejection must fail");
        };
        assert!(matches!(
            error.kind,
            ModelErrorKind::InvalidRequest | ModelErrorKind::Unsupported
        ));
        assert_eq!(
            error.retry_disposition,
            rustx::model::ModelRetryDisposition::Never
        );
        assert_eq!(
            server.attempt_count(),
            1,
            "no hidden prompt fallback request"
        );
        let body: Value = serde_json::from_str(&server.request_body(0)).expect("native wire body");
        let format = match protocol {
            ModelProtocol::OpenAiChatCompletions => &body["response_format"],
            ModelProtocol::OpenAiResponses => &body["text"]["format"],
            ModelProtocol::AnthropicMessages => &body["output_config"]["format"],
        };
        assert_eq!(format["type"], "json_schema");
    }
}
