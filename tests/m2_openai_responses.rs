//! Deterministic `OpenAI` Responses adapter tests.
//!
//! Covers stream normalization, Stored and Stateless continuation, opaque
//! state preservation, finish-reason derivation, and rejection of
//! provider-hosted output items. All tests run against a local fixture server.

mod common;

use common::{
    collect_events, describe_events, error_fixture, model_tool, simple_request, sse_fixture,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{AgentContentBlock, ContentBlockIndex, MessageBlock};
use rustx::model::finish::ModelFinishReason;
use rustx::model::{
    ModelErrorKind, ModelEvent, ModelProtocol, ModelRequest, ModelUsage, OpenAiAdapterConfig,
    OpenAiResponsesAdapter, ResponsesStorageMode, UsageDetails,
};
use rustx::runtime::continuation::{OpenAiResponsesContinuation, ProviderContinuationState};
use rustx::runtime::identity::ToolCallId;
use rustx::tools::types::ToolCall;

fn adapter(server: &common::FixtureServer) -> OpenAiResponsesAdapter {
    OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new("test-key", server.url("/v1")))
}

/// Selects the provider storage/continuation mode of one request.
///
/// Storage mode is per-model structural compat metadata carried by the
/// request's invocation configuration, not adapter configuration: one
/// adapter serves every Responses model of its provider.
fn with_storage(mut request: ModelRequest, storage: ResponsesStorageMode) -> ModelRequest {
    request.invocation.compat.responses_storage = storage;
    request
}

fn request_with_tools(prompt: &str) -> ModelRequest {
    let mut request = simple_request(ModelProtocol::OpenAiResponses, "gpt-test", prompt);
    request.tools = vec![
        model_tool("list_directory", "tool-list"),
        model_tool("bash", "tool-bash"),
    ];
    request
}

fn assert_terminal_failed(events: &[ModelEvent], kind: &ModelErrorKind) {
    let terminal = events
        .last()
        .unwrap_or_else(|| panic!("expected a terminal event, got none"));
    let ModelEvent::Failed { error } = terminal else {
        panic!("expected Failed terminal, got {terminal:?}");
    };
    assert_eq!(&error.kind, kind, "unexpected error: {}", error.message);
}

/// Plain output text normalizes with a state-only continuation reasoning
/// block placed after provider output, before Completed.
#[tokio::test]
async fn plain_text_normalizes() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "Say hello"),
            ResponsesStorageMode::Stored,
        ),
    )
    .await;
    assert_eq!(events[0], ModelEvent::Started);
    assert_eq!(
        events[1],
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "Hello".to_owned(),
        }
    );
    assert_eq!(
        events[2],
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: " world".to_owned(),
        }
    );
    assert!(matches!(
        events[3],
        ModelEvent::ContinuationState {
            block_index,
            state: ProviderContinuationState::OpenAiResponses(
                OpenAiResponsesContinuation::Stored {
                    previous_response_id: ref id,
                }
            ),
        } if block_index == ContentBlockIndex::new(1) && id == "resp_1"
    ));
    assert_eq!(
        events[4],
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: Some(ModelUsage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
                details: Some(UsageDetails {
                    reasoning_tokens: Some(1),
                    cached_input_tokens: Some(4),
                }),
            }),
        }
    );
    assert_eq!(events.len(), 5, "{}", describe_events(&events));
}

/// Reasoning summary and reasoning text merge into one canonical reasoning
/// block per provider reasoning item.
#[tokio::test]
async fn reasoning_merges_into_one_block() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "reasoning.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "Think"),
            ResponsesStorageMode::Stored,
        ),
    )
    .await;
    let reasoning_deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ReasoningDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        reasoning_deltas,
        vec!["I need to", " think first.", "Detailed reasoning hidden."],
        "{}",
        describe_events(&events)
    );
    let blocks: std::collections::BTreeSet<u32> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ReasoningDelta { block_index, .. } => Some(block_index.get()),
            _ => None,
        })
        .collect();
    assert_eq!(blocks.len(), 1, "one canonical reasoning block per item");
}

/// Reasoning followed by text keeps separate canonical blocks.
#[tokio::test]
async fn reasoning_then_text_stays_separate() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "reasoning_then_text.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "Think"),
            ResponsesStorageMode::Stored,
        ),
    )
    .await;
    assert!(matches!(events[1], ModelEvent::ReasoningDelta { .. }));
    assert!(matches!(events[2], ModelEvent::TextDelta { .. }));
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            ..
        })
    ));
}

/// Reasoning plus a function call: tool call identity from the provider
/// `call_id`, arguments parsed once at completion.
#[tokio::test]
async fn reasoning_then_function_call_normalizes() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "reasoning_then_function_call.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            request_with_tools("Use the tool"),
            ResponsesStorageMode::Stored,
        ),
    )
    .await;
    let starts: Vec<&rustx::tools::types::ToolCallStart> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ToolCallStarted { call, .. } => Some(call),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].id, ToolCallId::new("call_7"));
    assert_eq!(starts[0].name, "list_directory");
    assert_eq!(starts[0].tool_id.as_str(), "tool-list");
    let completed: Vec<&ToolCall> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ToolCallCompleted { call, .. } => Some(call),
            _ => None,
        })
        .collect();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].arguments, serde_json::json!({"path": "."}));
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            ..
        })
    ));
}

/// Multiple output blocks (text, refusal, function call) allocate stable
/// independent canonical indexes.
#[tokio::test]
async fn multiple_output_blocks_keep_stable_indexes() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "multiple_output_blocks.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(request_with_tools("Mixed"), ResponsesStorageMode::Stored),
    )
    .await;
    assert!(matches!(events[1], ModelEvent::TextDelta { .. }));
    assert!(matches!(events[2], ModelEvent::RefusalDelta { .. }));
    assert!(matches!(events[3], ModelEvent::RefusalDelta { .. }));
    assert!(matches!(events[4], ModelEvent::ToolCallStarted { .. }));
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            ..
        })
    ));
}

/// A refusal-only response derives Refusal from the completed response.
#[tokio::test]
async fn refusal_derives_refusal_finish() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "refusal.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "No"),
            ResponsesStorageMode::Stored,
        ),
    )
    .await;
    assert!(matches!(events[1], ModelEvent::RefusalDelta { .. }));
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Refusal,
            ..
        })
    ));
}

/// Incomplete responses map their incomplete reason to Length or
/// `ContentFilter` and still emit continuation state before `Completed`.
#[tokio::test]
async fn incomplete_responses_map_reasons() {
    for (fixture, expected) in [
        ("incomplete_max_output.sse", ModelFinishReason::Length),
        (
            "incomplete_content_filter.sse",
            ModelFinishReason::ContentFilter,
        ),
    ] {
        let fixture = fixture.to_owned();
        let fixture_for_server = fixture.clone();
        let server = common::FixtureServer::start(move |_attempt, _head| {
            sse_fixture("openai_responses", &fixture_for_server)
        })
        .await;
        let events = collect_events(
            &adapter(&server),
            with_storage(
                simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi"),
                ResponsesStorageMode::Stored,
            ),
        )
        .await;
        assert!(
            matches!(
                events.last(),
                Some(ModelEvent::Completed { finish_reason, .. })
                    if *finish_reason == expected
            ),
            "fixture {fixture}: {}",
            describe_events(&events)
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ModelEvent::ContinuationState { .. }))
        );
    }
}

/// A failed response is a normalized Failed, never a fake finish reason.
#[tokio::test]
async fn failed_response_is_a_failure() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "failed.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi"),
            ResponsesStorageMode::Stored,
        ),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
}

/// A mid-stream error event fails the invocation.
#[tokio::test]
async fn stream_error_event_fails() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "stream_error.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi"),
            ResponsesStorageMode::Stored,
        ),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
}

/// A stream ending without a terminal response event is a normalized failure.
#[tokio::test]
async fn interrupted_stream_fails() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "interrupted.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi"),
            ResponsesStorageMode::Stored,
        ),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
}

/// Provider-hosted output items are never reinterpreted as rustX tools.
#[tokio::test]
async fn provider_hosted_output_item_is_unsupported() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "unsupported_item.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "Search"),
            ResponsesStorageMode::Stored,
        ),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::Unsupported);
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, ModelEvent::ToolCallStarted { .. }))
    );
}

/// A reasoning item whose text appears only in the done event is emitted once
/// (finalize without replay).
#[tokio::test]
async fn reasoning_done_only_is_emitted_once() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "reasoning_done_only.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "Think"),
            ResponsesStorageMode::Stored,
        ),
    )
    .await;
    let reasoning: Vec<&String> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ReasoningDelta { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning, vec!["Only in done."]);
}

/// Stateless mode: the fresh request sets store=false, requests encrypted
/// reasoning, and the completed response preserves the exact provider output
/// items, including opaque encrypted reasoning content and unknown fields.
#[tokio::test]
async fn stateless_fresh_request_preserves_opaque_items() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "stateless_encrypted.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi"),
            ResponsesStorageMode::Stateless,
        ),
    )
    .await;
    let ModelEvent::ContinuationState { state, .. } = &events[events.len() - 2] else {
        panic!("expected continuation state before Completed");
    };
    let ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {
        items,
    }) = state
    else {
        panic!("expected stateless continuation");
    };
    assert_eq!(items.len(), 2, "both output items preserved");
    let reasoning = &items[0];
    assert_eq!(reasoning["type"], "reasoning");
    assert_eq!(
        reasoning["encrypted_content"],
        "opaque-encrypted-reasoning-blob"
    );
    // Unknown provider fields survive losslessly.
    assert_eq!(reasoning["extra_future_field"]["nested"][0], 1);
    assert_eq!(items[1]["type"], "message");
    // Semantic JSON round-trip of the canonical state loses nothing.
    let json = serde_json::to_string(state).expect("serialize state");
    let decoded: ProviderContinuationState =
        serde_json::from_str(&json).expect("deserialize state");
    assert_eq!(
        decoded, *state,
        "stateless opaque JSON round-trips losslessly"
    );
    // The request itself used store=false and requested encrypted reasoning.
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert_eq!(body["store"], false);
    assert_eq!(
        body["include"],
        serde_json::json!(["reasoning.encrypted_content"])
    );
}

/// Stateless continuation replays the preserved provider-native items first,
/// then only the canonical context after the continuation boundary, without
/// duplicating the previous generation.
#[tokio::test]
async fn stateless_continuation_replays_items_without_duplication() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let mut request = simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "Continue");
    // The canonical history ends with the previous agent generation followed
    // by the new user prompt.
    request.messages.insert(
        0,
        MessageBlock::Agent(rustx::message::types::AgentMessageBlock {
            id: rustx::runtime::identity::MessageId::new("msg-prev"),
            content: vec![AgentContentBlock::Text(TextBlock {
                text: "Previous answer.".to_owned(),
            })],
        }),
    );
    request.continuation = Some(ProviderContinuationState::OpenAiResponses(
        OpenAiResponsesContinuation::Stateless {
            items: vec![
                serde_json::json!({
                    "type": "reasoning",
                    "id": "rs_prev",
                    "summary": [{"type": "summary_text", "text": "Prior reasoning."}],
                    "encrypted_content": "opaque-blob",
                }),
                serde_json::json!({
                    "type": "message",
                    "role": "assistant",
                    "id": "msg_prev",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": "Previous answer.", "annotations": []}],
                }),
            ],
        },
    ));
    let events = collect_events(
        &adapter(&server),
        with_storage(request, ResponsesStorageMode::Stateless),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let input = body["input"].as_array().expect("input array");
    assert_eq!(input.len(), 3, "items + tail user prompt, no duplication");
    assert_eq!(input[0]["type"], "reasoning");
    assert_eq!(input[0]["encrypted_content"], "opaque-blob");
    assert_eq!(input[1]["type"], "message");
    assert_eq!(input[1]["content"][0]["text"], "Previous answer.");
    assert_eq!(input[2]["type"], "message");
    assert_eq!(input[2]["role"], "user");
    assert_eq!(input[2]["content"][0]["text"], "Continue");
    assert!(body.get("previous_response_id").is_none());
}

/// Stored mode: a fresh request enables provider storage and preserves the
/// response id for later continuation.
#[tokio::test]
async fn stored_mode_preserves_response_id() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "stored_completed.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        with_storage(
            simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi"),
            ResponsesStorageMode::Stored,
        ),
    )
    .await;
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert_eq!(body["store"], true);
    let ModelEvent::ContinuationState { state, .. } = &events[events.len() - 2] else {
        panic!("expected continuation state before Completed");
    };
    assert!(matches!(
        state,
        ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stored {
            previous_response_id,
        }) if previous_response_id == "resp_stored"
    ));
}

/// Stored continuation sets `previous_response_id` and sends only the canonical
/// context after the continued response boundary.
#[tokio::test]
async fn stored_continuation_sends_only_tail_context() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "stored_completed.sse")
    })
    .await;
    let mut request = simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "Continue");
    request.messages.insert(
        0,
        MessageBlock::Agent(rustx::message::types::AgentMessageBlock {
            id: rustx::runtime::identity::MessageId::new("msg-prev"),
            content: vec![AgentContentBlock::Text(TextBlock {
                text: "Old generation.".to_owned(),
            })],
        }),
    );
    request.continuation = Some(ProviderContinuationState::OpenAiResponses(
        OpenAiResponsesContinuation::Stored {
            previous_response_id: "resp_prev".to_owned(),
        },
    ));
    let events = collect_events(
        &adapter(&server),
        with_storage(request, ResponsesStorageMode::Stored),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert_eq!(body["previous_response_id"], "resp_prev");
    let input = body["input"].as_array().expect("input array");
    assert_eq!(
        input.len(),
        1,
        "only the tail user prompt after the boundary"
    );
    assert_eq!(input[0]["content"][0]["text"], "Continue");
}

/// Storage mode and continuation variant contradictions are rejected before
/// the network.
#[tokio::test]
async fn storage_mode_continuation_contradiction_is_rejected() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    for (mode, continuation) in [
        (
            ResponsesStorageMode::Stored,
            ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {
                items: Vec::new(),
            }),
        ),
        (
            ResponsesStorageMode::Stateless,
            ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stored {
                previous_response_id: "resp_1".to_owned(),
            }),
        ),
    ] {
        let mut request = simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi");
        request.messages.insert(
            0,
            MessageBlock::Agent(rustx::message::types::AgentMessageBlock {
                id: rustx::runtime::identity::MessageId::new("msg-prev"),
                content: vec![AgentContentBlock::Text(TextBlock {
                    text: "Old.".to_owned(),
                })],
            }),
        );
        request.continuation = Some(continuation);
        let events = collect_events(&adapter(&server), with_storage(request, mode)).await;
        assert_eq!(events.len(), 1, "terminal Failed without Started");
        assert_terminal_failed(&events, &ModelErrorKind::InvalidRequest);
        assert_eq!(server.attempt_count(), 0, "no provider request");
    }
}

/// A continuation request without a preceding agent boundary fails
/// explicitly instead of guessing.
#[tokio::test]
async fn continuation_without_boundary_fails() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let mut request = simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi");
    request.continuation = Some(ProviderContinuationState::OpenAiResponses(
        OpenAiResponsesContinuation::Stored {
            previous_response_id: "resp_1".to_owned(),
        },
    ));
    let events = collect_events(
        &adapter(&server),
        with_storage(request, ResponsesStorageMode::Stored),
    )
    .await;
    assert_eq!(events.len(), 1);
    assert_terminal_failed(&events, &ModelErrorKind::InvalidRequest);
    assert_eq!(server.attempt_count(), 0);
}

/// HTTP errors normalize for the Responses protocol as well.
#[tokio::test]
async fn http_errors_normalize() {
    let cases: Vec<(&str, u16, &str, ModelErrorKind)> = vec![
        (
            "openai_429.json",
            429,
            "Too Many Requests",
            ModelErrorKind::RateLimit,
        ),
        (
            "openai_401.json",
            401,
            "Unauthorized",
            ModelErrorKind::Authentication,
        ),
        (
            "openai_400_invalid.json",
            400,
            "Bad Request",
            ModelErrorKind::InvalidRequest,
        ),
        (
            "openai_400_context.json",
            400,
            "Bad Request",
            ModelErrorKind::ContextWindowExceeded,
        ),
        (
            "openai_500.json",
            500,
            "Internal Server Error",
            ModelErrorKind::ProviderError,
        ),
    ];
    for (fixture, status, reason, expected_kind) in cases {
        let fixture = fixture.to_owned();
        let fixture_for_server = fixture.clone();
        let server = common::FixtureServer::start(move |_attempt, _head| {
            let mut reply = error_fixture(&fixture_for_server);
            reply.status = status;
            reply.reason = reason;
            reply
        })
        .await;
        let events = collect_events(
            &adapter(&server),
            with_storage(
                simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi"),
                ResponsesStorageMode::Stored,
            ),
        )
        .await;
        assert_eq!(events.len(), 2, "fixture {fixture}");
        assert_terminal_failed(&events, &expected_kind);
    }
}

/// Canonical reasoning text without lossless provider-native state cannot be
/// reconstructed as an `OpenAI` Responses reasoning item: the request fails
/// before the network instead of fabricating provider ids, summary structure,
/// or encrypted content.
#[tokio::test]
async fn reasoning_without_provider_state_is_unsupported() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let mut request = simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi");
    request.messages.insert(
        0,
        MessageBlock::Agent(rustx::message::types::AgentMessageBlock {
            id: rustx::runtime::identity::MessageId::new("msg-reasoning"),
            content: vec![AgentContentBlock::Reasoning(
                rustx::message::types::ReasoningBlock {
                    text: Some("Visible reasoning text.".to_owned()),
                    provider_state: None,
                },
            )],
        }),
    );
    let events = collect_events(
        &adapter(&server),
        with_storage(request, ResponsesStorageMode::Stored),
    )
    .await;
    assert_eq!(events.len(), 1, "rejected before the network");
    assert_terminal_failed(&events, &ModelErrorKind::Unsupported);
    assert_eq!(server.attempt_count(), 0);
}

/// Reasoning with preserved provider-native stateless items replays those
/// items verbatim instead of fabricating a summary item.
#[tokio::test]
async fn reasoning_with_provider_state_replays_items_verbatim() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let mut request = simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "hi");
    let preserved = vec![serde_json::json!({
        "type": "reasoning",
        "id": "rs_preserved",
        "summary": [{"type": "summary_text", "text": "Preserved."}],
        "encrypted_content": "opaque-blob",
        "extra": {"kept": true},
    })];
    request.messages.insert(
        0,
        MessageBlock::Agent(rustx::message::types::AgentMessageBlock {
            id: rustx::runtime::identity::MessageId::new("msg-reasoning"),
            content: vec![AgentContentBlock::Reasoning(
                rustx::message::types::ReasoningBlock {
                    text: None,
                    provider_state: Some(ProviderContinuationState::OpenAiResponses(
                        OpenAiResponsesContinuation::Stateless {
                            items: preserved.clone(),
                        },
                    )),
                },
            )],
        }),
    );
    let events = collect_events(
        &adapter(&server),
        with_storage(request, ResponsesStorageMode::Stored),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let input = body["input"].as_array().expect("input items");
    let reasoning = input
        .iter()
        .find(|item| item["type"] == "reasoning")
        .expect("reasoning item present");
    assert_eq!(
        reasoning, &preserved[0],
        "the preserved provider-native item replays verbatim"
    );
    let reasoning_items: Vec<&serde_json::Value> = input
        .iter()
        .filter(|item| item["type"] == "reasoning")
        .collect();
    assert_eq!(
        reasoning_items.len(),
        1,
        "exactly the preserved item, no fabricated duplicate"
    );
}

/// The serialized fresh request carries only model-facing tool fields, and no
/// reasoning field is synthesized when none is configured.
#[tokio::test]
async fn request_serialization_is_model_facing_only() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "plain_text.sse")
    })
    .await;
    let request = request_with_tools("List");
    let events = collect_events(
        &adapter(&server),
        with_storage(request, ResponsesStorageMode::Stored),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert!(
        body.get("reasoning").is_none(),
        "the retired universal reasoning mapping never injects a field: {body}"
    );
    assert_eq!(body["stream"], true);
    let tool = &body["tools"][0];
    assert_eq!(tool["type"], "function");
    assert_eq!(tool["name"], "list_directory");
    for runtime_field in ["execution_mode", "replay_policy", "origin", "id"] {
        assert!(
            tool.get(runtime_field).is_none(),
            "runtime-only field {runtime_field} must not reach the provider"
        );
    }
}

/// The canonical tail with continuation preserves the provider-tail ordering
/// `Tool result → User A → User B`: the continuation is retained, the tool
/// result translates to `function_call_output`, and the two inbound
/// messages stay separate ordered `message` items.
#[tokio::test]
async fn continuation_tail_preserves_tool_then_users_order() {
    use rustx::message::types::{
        AgentMessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock,
    };
    use rustx::runtime::identity::MessageId;
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "stored_completed.sse")
    })
    .await;
    let mut request = simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "Continue");
    let user = |text: &str| {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-inbound"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: rustx::message::types::UserSource::Human,
            kind: rustx::message::types::InboundKind::Message,
            timestamp: None,
        })
    };
    request.messages = vec![
        MessageBlock::Agent(AgentMessageBlock {
            id: MessageId::new("msg-boundary"),
            content: vec![AgentContentBlock::Text(TextBlock {
                text: "Old generation.".to_owned(),
            })],
        }),
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("msg-tool-1"),
            tool_call_id: ToolCallId::new("call_1"),
            tool_id: rustx::runtime::identity::ToolId::new("tool-list"),
            result: rustx::tools::types::ToolExecutionResult {
                status: rustx::tools::types::ToolExecutionStatus::Success,
                content: vec![rustx::tools::types::ToolResultContent::Text(TextBlock {
                    text: "listed".to_owned(),
                })],
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
            },
        }),
        user("A"),
        user("B"),
    ];
    request.continuation = Some(ProviderContinuationState::OpenAiResponses(
        OpenAiResponsesContinuation::Stored {
            previous_response_id: "resp_prev".to_owned(),
        },
    ));
    let events = collect_events(
        &adapter(&server),
        with_storage(request, ResponsesStorageMode::Stored),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert_eq!(
        body["previous_response_id"], "resp_prev",
        "the ordinary inbound tail does not clear the continuation"
    );
    let input = body["input"].as_array().expect("input array");
    assert_eq!(
        input.len(),
        3,
        "tool result then user A then user B, all in one tail"
    );
    assert_eq!(input[0]["type"], "function_call_output");
    assert_eq!(input[0]["call_id"], "call_1");
    assert_eq!(input[1]["type"], "message");
    assert_eq!(input[1]["content"][0]["text"], "A");
    assert_eq!(input[2]["type"], "message");
    assert_eq!(input[2]["content"][0]["text"], "B");
    assert_ne!(
        input[1], input[2],
        "user A and user B remain distinct provider items"
    );
    assert_eq!(
        input[1]["content"][0]["text"], "A",
        "A translated at index 1"
    );
    assert_eq!(
        input[2]["content"][0]["text"], "B",
        "B translated at index 2"
    );
}
/// The no-tool tail form `Agent boundary → User A → User B` with a
/// continuation keeps both inbound messages as distinct ordered items.
#[tokio::test]
async fn continuation_no_tool_tail_preserves_both_users() {
    use rustx::message::types::{AgentMessageBlock, UserContentBlock, UserMessageBlock};
    use rustx::runtime::identity::MessageId;
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_responses", "stored_completed.sse")
    })
    .await;
    let mut request = simple_request(ModelProtocol::OpenAiResponses, "gpt-test", "Continue");
    let user = |id: &str, text: &str| {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: rustx::message::types::UserSource::Runtime,
            kind: rustx::message::types::InboundKind::Message,
            timestamp: None,
        })
    };
    request.messages = vec![
        MessageBlock::Agent(AgentMessageBlock {
            id: MessageId::new("msg-boundary"),
            content: vec![AgentContentBlock::Text(TextBlock {
                text: "Old generation.".to_owned(),
            })],
        }),
        user("msg-a", "A"),
        user("msg-b", "B"),
    ];
    request.continuation = Some(ProviderContinuationState::OpenAiResponses(
        OpenAiResponsesContinuation::Stored {
            previous_response_id: "resp_prev".to_owned(),
        },
    ));
    let events = collect_events(
        &adapter(&server),
        with_storage(request, ResponsesStorageMode::Stored),
    )
    .await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let input = body["input"].as_array().expect("input array");
    assert_eq!(input.len(), 2, "user A and user B as distinct items");
    assert_eq!(input[0]["type"], "message");
    assert_eq!(input[0]["content"][0]["text"], "A");
    assert_eq!(input[1]["type"], "message");
    assert_eq!(input[1]["content"][0]["text"], "B");
}
