//! Transport-level deterministic tests: the one-attempt invariant and
//! cancellation propagation through the common adapter interface.
//!
//! No test web framework is used; a raw local HTTP server counts attempts.

use std::time::Duration;

use crate::common::{error_fixture, simple_request, sse_fixture};
use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ModelAdapter, ModelErrorKind, ModelEvent,
    ModelProtocol, ModelRequest, ModelStreamItem, OpenAiAdapterConfig,
    OpenAiChatCompletionsAdapter, OpenAiResponsesAdapter,
};
use rustx::model::{
    CarryoverBlockKind, CarryoverOmissionCounts, ModelInputMessage, RenderedCarryoverRecord,
    RenderedCarryoverText, RenderedUnresolvedOutputCarryover, RequestOnlyModelContext,
    UnresolvedOutputSettlement,
};
use rustx::runtime::CancellationSignal;
use rustx::runtime::identity::PublicationStreamId;

fn openai_chat(server: &crate::common::FixtureServer) -> OpenAiChatCompletionsAdapter {
    OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("test-key", server.url("/v1")))
}

fn openai_responses(server: &crate::common::FixtureServer) -> OpenAiResponsesAdapter {
    OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new("test-key", server.url("/v1")))
}

fn anthropic(server: &crate::common::FixtureServer) -> AnthropicMessagesAdapter {
    AnthropicMessagesAdapter::new(AnthropicAdapterConfig::new("test-key", server.url("")))
}

fn chat_request() -> ModelRequest {
    simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi")
}

fn responses_request() -> ModelRequest {
    simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi")
}

fn anthropic_request() -> ModelRequest {
    simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi")
}

fn request_with_carryover(mut request: ModelRequest) -> ModelRequest {
    request.messages.push(ModelInputMessage::RequestOnly(
        RequestOnlyModelContext::UnresolvedOutputCarryover(RenderedUnresolvedOutputCarryover {
            source_stream_id: PublicationStreamId::new("audit-stream"),
            source_settlement: UnresolvedOutputSettlement::Incomplete,
            records: vec![RenderedCarryoverRecord::Text(RenderedCarryoverText {
                kind: CarryoverBlockKind::Text,
                text: Some("unresolved tail".to_owned()),
                omitted_prefix_bytes: 0,
                omitted_detail_bytes: 0,
            })],
            omitted_blocks: CarryoverOmissionCounts::default(),
        }),
    ));
    request
}

async fn collect(adapter: &dyn ModelAdapter, request: ModelRequest) -> Vec<ModelEvent> {
    crate::common::collect_events(adapter, request).await
}

fn last_error_kind(events: &[ModelEvent]) -> ModelErrorKind {
    let ModelEvent::Failed { error } = events.last().expect("terminal event") else {
        panic!("expected Failed terminal");
    };
    error.kind.clone()
}

/// Every provider adapter receives the already ordered provider-neutral
/// request-only item and translates it as runtime-authored user context. The
/// adapters do not select, order, load, or consume the Publication Audit.
#[tokio::test]
async fn every_adapter_translates_request_only_carryover_without_canonical_identity() {
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

    let chat_events = collect(
        &openai_chat(&chat_server),
        request_with_carryover(chat_request()),
    )
    .await;
    assert!(matches!(
        chat_events.last(),
        Some(ModelEvent::Completed { .. })
    ));
    let chat_body: serde_json::Value =
        serde_json::from_str(&chat_server.request_body(0)).expect("Chat request JSON");
    let chat_messages = chat_body["messages"].as_array().expect("Chat messages");
    assert_eq!(chat_messages.len(), 2);
    assert_eq!(chat_messages[1]["role"], "user");
    assert!(
        chat_messages[1]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| {
                text.contains("unresolved tail") && text.contains("source_settlement=incomplete")
            })
    );

    let responses_events = collect(
        &openai_responses(&responses_server),
        request_with_carryover(responses_request()),
    )
    .await;
    assert!(matches!(
        responses_events.last(),
        Some(ModelEvent::Completed { .. })
    ));
    let responses_body: serde_json::Value =
        serde_json::from_str(&responses_server.request_body(0)).expect("Responses request JSON");
    let responses_input = responses_body["input"].as_array().expect("Responses input");
    assert_eq!(responses_input.len(), 2);
    assert_eq!(responses_input[1]["role"], "user");
    assert!(
        responses_input[1]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| {
                text.contains("unresolved tail") && text.contains("source_settlement=incomplete")
            })
    );

    let anthropic_events = collect(
        &anthropic(&anthropic_server),
        request_with_carryover(anthropic_request()),
    )
    .await;
    assert!(matches!(
        anthropic_events.last(),
        Some(ModelEvent::Completed { .. })
    ));
    let anthropic_body: serde_json::Value =
        serde_json::from_str(&anthropic_server.request_body(0)).expect("Anthropic request JSON");
    let anthropic_messages = anthropic_body["messages"]
        .as_array()
        .expect("Anthropic messages");
    assert_eq!(anthropic_messages.len(), 2);
    assert_eq!(anthropic_messages[1]["role"], "user");
    assert!(
        anthropic_messages[1]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| {
                text.contains("unresolved tail") && text.contains("source_settlement=incomplete")
            })
    );

    let request = request_with_carryover(chat_request());
    assert!(request.messages.iter().any(|message| {
        matches!(message, ModelInputMessage::RequestOnly(_)) && message.canonical_id().is_none()
    }));
}

/// One `OpenAI` Chat invocation against a simulated retryable 429 performs
/// exactly one HTTP attempt and one normalized Failed terminal event.
#[tokio::test]
async fn openai_chat_one_attempt_on_429() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        let mut reply = error_fixture("openai_429.json");
        reply.status = 429;
        reply.reason = "Too Many Requests";
        reply
    })
    .await;
    let events = collect(&openai_chat(&server), chat_request()).await;
    assert_eq!(
        server.attempt_count(),
        1,
        "exactly one HTTP attempt, no retry"
    );
    assert_eq!(events.len(), 2, "[Started, Failed]");
    assert_eq!(events[0], ModelEvent::Started);
    assert_eq!(last_error_kind(&events), ModelErrorKind::RateLimit);
}

/// The same one-attempt invariant holds for a simulated 5xx.
#[tokio::test]
async fn openai_chat_one_attempt_on_500() {
    let server =
        crate::common::FixtureServer::start(|_attempt, _head| error_fixture("openai_500.json"))
            .await;
    let events = collect(&openai_chat(&server), chat_request()).await;
    assert_eq!(
        server.attempt_count(),
        1,
        "exactly one HTTP attempt, no retry"
    );
    assert_eq!(last_error_kind(&events), ModelErrorKind::ProviderError);
}

/// One `OpenAI` Responses invocation performs exactly one HTTP attempt.
#[tokio::test]
async fn openai_responses_one_attempt_on_429() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        let mut reply = error_fixture("openai_429.json");
        reply.status = 429;
        reply.reason = "Too Many Requests";
        reply
    })
    .await;
    let events = collect(&openai_responses(&server), responses_request()).await;
    assert_eq!(
        server.attempt_count(),
        1,
        "exactly one HTTP attempt, no retry"
    );
    assert_eq!(last_error_kind(&events), ModelErrorKind::RateLimit);
}

/// One Anthropic invocation performs exactly one HTTP attempt.
#[tokio::test]
async fn anthropic_one_attempt_on_429() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        let mut reply = error_fixture("anthropic_429.json");
        reply.status = 429;
        reply.reason = "Too Many Requests";
        reply
    })
    .await;
    let events = collect(&anthropic(&server), anthropic_request()).await;
    assert_eq!(
        server.attempt_count(),
        1,
        "exactly one HTTP attempt, no retry"
    );
    assert_eq!(last_error_kind(&events), ModelErrorKind::RateLimit);
}

/// One Anthropic invocation against a simulated 5xx performs exactly one
/// attempt.
#[tokio::test]
async fn anthropic_one_attempt_on_500() {
    let server =
        crate::common::FixtureServer::start(|_attempt, _head| error_fixture("anthropic_500.json"))
            .await;
    let events = collect(&anthropic(&server), anthropic_request()).await;
    assert_eq!(
        server.attempt_count(),
        1,
        "exactly one HTTP attempt, no retry"
    );
    assert_eq!(last_error_kind(&events), ModelErrorKind::ProviderError);
}

/// Cancelling before any network request must not create a provider request:
/// the terminal event is Failed(Cancelled) without Started.
#[tokio::test]
async fn cancellation_before_network_creates_no_request() {
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
            "openai-chat",
            Box::new(openai_chat(&chat_server)),
            chat_request(),
            &chat_server,
        ),
        (
            "openai-responses",
            Box::new(openai_responses(&responses_server)),
            responses_request(),
            &responses_server,
        ),
        (
            "anthropic",
            Box::new(anthropic(&anthropic_server)),
            anthropic_request(),
            &anthropic_server,
        ),
    ];
    for (name, adapter, request, server) in cases {
        let cancellation = CancellationSignal::new();
        cancellation.cancel();
        let mut stream = adapter.stream(request, cancellation);
        let events: Vec<ModelEvent> = {
            use futures_util::StreamExt;
            let mut collected = Vec::new();
            while let Some(item) = stream.next().await {
                if let ModelStreamItem::Event(event) = item {
                    collected.push(event);
                }
            }
            collected
        };
        assert_eq!(events.len(), 1, "{name}: single Failed(Cancelled)");
        assert_eq!(
            last_error_kind(&events),
            ModelErrorKind::Cancelled,
            "{name}"
        );
        assert_eq!(
            server.attempt_count(),
            0,
            "{name}: no provider request on pre-network cancellation"
        );
    }
}

/// An in-flight `OpenAI` Chat stream is cancelled through the common adapter
/// interface: the underlying stream stops, no further deltas are emitted, and
/// the terminal event is Failed(Cancelled).
///
/// `Started` is emitted as the provider request attempt begins, before the
/// connection is made; cancellation therefore happens only after a provider
/// delta proves the stream is actually in flight.
#[tokio::test]
async fn cancellation_in_flight_openai_chat() {
    use futures_util::StreamExt;
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        crate::common::FixtureReply::chunked(
            200,
            "OK",
            "text/event-stream",
            vec![
                (0, b"data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n".to_vec()),
                (60_000, b"data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\n".to_vec()),
            ],
        )
    })
    .await;
    let cancellation = CancellationSignal::new();
    let mut stream = openai_chat(&server).stream(chat_request(), cancellation.clone());
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("first event within timeout");
    assert_eq!(first, Some(ModelStreamItem::Event(ModelEvent::Started)));
    let second = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("text delta within timeout");
    assert!(matches!(
        second,
        Some(ModelStreamItem::Event(ModelEvent::TextDelta { .. }))
    ));
    // Cancel while the provider stream is actually in flight.
    cancellation.cancel();
    let usage_or_terminal = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("terminal within timeout");
    let terminal = match usage_or_terminal {
        Some(ModelStreamItem::Event(ModelEvent::UsageUpdate { usage })) => {
            assert_eq!(usage.input_tokens, 5);
            tokio::time::timeout(Duration::from_secs(5), stream.next())
                .await
                .expect("terminal after usage update")
        }
        other => other,
    };
    assert!(matches!(
        terminal,
        Some(ModelStreamItem::Event(ModelEvent::Failed { error }))
            if error.kind == ModelErrorKind::Cancelled
    ));
    // The stream terminates; nothing is emitted after the terminal event.
    let after = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("stream ends promptly");
    assert_eq!(after, None);
    assert_eq!(server.attempt_count(), 1, "one attempt, no retry on cancel");
}

/// An in-flight `Anthropic` stream is cancelled the same way; the cancel
/// happens only after a provider text delta proves the stream is in flight.
#[tokio::test]
async fn cancellation_in_flight_anthropic() {
    use futures_util::StreamExt;
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        crate::common::FixtureReply::chunked(
            200,
            "OK",
            "text/event-stream",
            vec![
                (0, b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"m\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n\n".to_vec()),
                (0, b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n".to_vec()),
                (0, b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n".to_vec()),
                (60_000, b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\n".to_vec()),
            ],
        )
    })
    .await;
    let cancellation = CancellationSignal::new();
    let mut stream = anthropic(&server).stream(anthropic_request(), cancellation.clone());
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("first event within timeout");
    assert_eq!(first, Some(ModelStreamItem::Event(ModelEvent::Started)));
    let delta = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("text delta within timeout");
    assert!(matches!(
        delta,
        Some(ModelStreamItem::Event(ModelEvent::TextDelta { .. }))
    ));
    cancellation.cancel();
    let usage_or_terminal = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("terminal within timeout");
    let terminal = match usage_or_terminal {
        Some(ModelStreamItem::Event(ModelEvent::UsageUpdate { usage })) => {
            assert_eq!(usage.input_tokens, 5);
            tokio::time::timeout(Duration::from_secs(5), stream.next())
                .await
                .expect("terminal after usage update")
        }
        other => other,
    };
    assert!(matches!(
        terminal,
        Some(ModelStreamItem::Event(ModelEvent::Failed { error }))
            if error.kind == ModelErrorKind::Cancelled
    ));
    let after = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("stream ends promptly");
    assert_eq!(after, None);
    assert_eq!(server.attempt_count(), 1);
}

/// An in-flight `OpenAI` Responses stream is cancelled the same way; the
/// cancel happens only after a provider text delta proves the stream is in
/// flight.
#[tokio::test]
async fn cancellation_in_flight_openai_responses() {
    use futures_util::StreamExt;
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        crate::common::FixtureReply::chunked(
            200,
            "OK",
            "text/event-stream",
            vec![
                (0, b"data: {\"type\":\"response.output_item.added\",\"sequence_number\":1,\"output_index\":0,\"item\":{\"id\":\"msg_1\",\"type\":\"message\",\"status\":\"in_progress\",\"role\":\"assistant\",\"content\":[]}}\n\n".to_vec()),
                (0, b"data: {\"type\":\"response.output_text.delta\",\"sequence_number\":2,\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\n".to_vec()),
                (60_000, b"data: {\"type\":\"response.output_text.delta\",\"sequence_number\":3,\"item_id\":\"msg_1\",\"output_index\":0,\"content_index\":0,\"delta\":\" world\"}\n\n".to_vec()),
            ],
        )
    })
    .await;
    let cancellation = CancellationSignal::new();
    let mut stream = openai_responses(&server).stream(responses_request(), cancellation.clone());
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("first event within timeout");
    assert_eq!(first, Some(ModelStreamItem::Event(ModelEvent::Started)));
    let delta = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("text delta within timeout");
    assert!(matches!(
        delta,
        Some(ModelStreamItem::Event(ModelEvent::TextDelta { .. }))
    ));
    cancellation.cancel();
    let terminal = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("terminal within timeout");
    assert!(matches!(
        terminal,
        Some(ModelStreamItem::Event(ModelEvent::Failed { error }))
            if error.kind == ModelErrorKind::Cancelled
    ));
    let after = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("stream ends promptly");
    assert_eq!(after, None);
    assert_eq!(server.attempt_count(), 1);
}

/// Cancellation while the provider has accepted the connection but delays the
/// response headers terminates the invocation promptly with exactly one
/// provider attempt, no retry, and no later events.
#[tokio::test]
async fn cancellation_while_headers_delayed_anthropic() {
    use futures_util::StreamExt;
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "text.sse").with_header_delay(60_000)
    })
    .await;
    let cancellation = CancellationSignal::new();
    let mut stream = anthropic(&server).stream(anthropic_request(), cancellation.clone());
    assert_eq!(
        stream.next().await,
        Some(ModelStreamItem::Event(ModelEvent::Started))
    );
    // Drive the stream in the background so the connection attempt happens.
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            if let ModelStreamItem::Event(event) = item {
                events.push(event);
            }
        }
        events
    });
    // Wait until the provider has accepted the connection (attempt counted),
    // then cancel while the response headers are still delayed.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while server.attempt_count() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "connection never opened"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cancellation.cancel();
    let events = tokio::time::timeout(Duration::from_secs(5), collector)
        .await
        .expect("collection finished promptly")
        .expect("collector did not panic");
    // The test itself consumed Started; the collector sees exactly the
    // terminal Failed(Cancelled) and nothing after it.
    assert_eq!(
        events,
        vec![ModelEvent::Failed {
            error: rustx::model::ModelError {
                kind: ModelErrorKind::Cancelled,
                message: "model invocation cancelled".to_owned(),
                retry_disposition: rustx::model::error::ModelRetryDisposition::Never,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                generation: None,
            },
        }],
        "lifecycle is Started then Failed(Cancelled), nothing after"
    );
    assert_eq!(
        server.attempt_count(),
        1,
        "exactly one provider attempt, no retry"
    );
}

/// The same before-headers cancellation for `OpenAI` Chat.
#[tokio::test]
async fn cancellation_while_headers_delayed_openai_chat() {
    use futures_util::StreamExt;
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse").with_header_delay(60_000)
    })
    .await;
    let cancellation = CancellationSignal::new();
    let mut stream = openai_chat(&server).stream(chat_request(), cancellation.clone());
    assert_eq!(
        stream.next().await,
        Some(ModelStreamItem::Event(ModelEvent::Started))
    );
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            if let ModelStreamItem::Event(event) = item {
                events.push(event);
            }
        }
        events
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while server.attempt_count() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "connection never opened"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cancellation.cancel();
    let events = tokio::time::timeout(Duration::from_secs(5), collector)
        .await
        .expect("collection finished promptly")
        .expect("collector did not panic");
    // The test itself consumed Started; the collector sees exactly the
    // terminal Failed(Cancelled) and nothing after it.
    assert_eq!(
        events,
        vec![ModelEvent::Failed {
            error: rustx::model::ModelError {
                kind: ModelErrorKind::Cancelled,
                message: "model invocation cancelled".to_owned(),
                retry_disposition: rustx::model::error::ModelRetryDisposition::Never,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                generation: None,
            },
        }],
        "lifecycle is Started then Failed(Cancelled), nothing after"
    );
    assert_eq!(
        server.attempt_count(),
        1,
        "exactly one provider attempt, no retry"
    );
}

/// The same before-headers cancellation for `OpenAI` Responses.
#[tokio::test]
async fn cancellation_while_headers_delayed_openai_responses() {
    use futures_util::StreamExt;
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse").with_header_delay(60_000)
    })
    .await;
    let cancellation = CancellationSignal::new();
    let mut stream = openai_responses(&server).stream(responses_request(), cancellation.clone());
    assert_eq!(
        stream.next().await,
        Some(ModelStreamItem::Event(ModelEvent::Started))
    );
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            if let ModelStreamItem::Event(event) = item {
                events.push(event);
            }
        }
        events
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while server.attempt_count() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "connection never opened"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cancellation.cancel();
    let events = tokio::time::timeout(Duration::from_secs(5), collector)
        .await
        .expect("collection finished promptly")
        .expect("collector did not panic");
    // The test itself consumed Started; the collector sees exactly the
    // terminal Failed(Cancelled) and nothing after it.
    assert_eq!(
        events,
        vec![ModelEvent::Failed {
            error: rustx::model::ModelError {
                kind: ModelErrorKind::Cancelled,
                message: "model invocation cancelled".to_owned(),
                retry_disposition: rustx::model::error::ModelRetryDisposition::Never,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
                malformed_tool_proposal: None,
                generation: None,
            },
        }],
        "lifecycle is Started then Failed(Cancelled), nothing after"
    );
    assert_eq!(
        server.attempt_count(),
        1,
        "exactly one provider attempt, no retry"
    );
}
