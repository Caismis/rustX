//! Deterministic Anthropic Messages adapter tests.
//!
//! All tests drive the real adapter over a local fixture HTTP server serving
//! provider-shaped SSE streams.

mod common;

use common::{collect_events, describe_events, error_fixture, simple_request, sse_fixture, tool};
use rustx::message::types::ContentBlockIndex;
use rustx::model::finish::ModelFinishReason;
use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ModelErrorKind, ModelEvent, ModelProtocol,
    ModelRequest, ModelUsage,
};
use rustx::runtime::continuation::{AnthropicContinuation, ProviderContinuationState};
use rustx::runtime::identity::ToolCallId;

fn adapter(server: &common::FixtureServer) -> AnthropicMessagesAdapter {
    AnthropicMessagesAdapter::new(
        AnthropicAdapterConfig::new("test-key").with_api_base(server.url("")),
    )
}

fn request_with_tools(prompt: &str) -> ModelRequest {
    let mut request = simple_request(ModelProtocol::AnthropicMessages, "claude-test", prompt);
    request.tools = vec![
        tool("list_directory", "tool-list"),
        tool("read_file", "tool-read"),
        tool("get_weather", "tool-weather"),
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

fn anthropic_state_of(event: &ModelEvent) -> &AnthropicContinuation {
    let ModelEvent::ContinuationState { state, .. } = event else {
        panic!("expected ContinuationState");
    };
    let ProviderContinuationState::Anthropic(continuation) = state else {
        panic!("expected Anthropic continuation");
    };
    continuation
}

/// Basic text streaming: buffered text flushes at the terminal stop reason,
/// usage combines the cumulative snapshots, and the stream completes with
/// Stop.
#[tokio::test]
async fn text_stream_normalizes() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "Hello"),
    )
    .await;
    assert_eq!(events[0], ModelEvent::Started);
    assert_eq!(
        events[1],
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "Hello world".to_owned(),
        }
    );
    assert_eq!(
        events[2],
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: Some(ModelUsage {
                input_tokens: 25,
                output_tokens: 15,
                total_tokens: 40,
                details: Some(rustx::model::UsageDetails {
                    reasoning_tokens: None,
                    cached_input_tokens: Some(5),
                }),
            }),
        }
    );
    assert_eq!(events.len(), 3, "{}", describe_events(&events));
}

/// Multiple text blocks keep distinct canonical indexes.
#[tokio::test]
async fn multiple_text_blocks_keep_separate_indexes() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "multiple_text_blocks.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert!(matches!(
        events[1],
        ModelEvent::TextDelta {
            block_index: idx,
            ref text,
        } if idx == ContentBlockIndex::new(0) && *text == "First."
    ));
    assert!(matches!(
        events[2],
        ModelEvent::TextDelta {
            block_index: idx,
            ref text,
        } if idx == ContentBlockIndex::new(1) && *text == " Second."
    ));
}

/// A thinking block becomes a canonical reasoning block with its signature
/// preserved as rustX-owned opaque continuation state.
#[tokio::test]
async fn thinking_block_preserves_signature_state() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "thinking.sse"))
            .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "Compute"),
    )
    .await;
    assert_eq!(
        events[1],
        ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(0),
            text: "Let me compute.".to_owned(),
        }
    );
    let state = anthropic_state_of(&events[2]);
    assert_eq!(state.opaque["type"], "thinking");
    assert_eq!(state.opaque["thinking"], "Let me compute.");
    assert_eq!(state.opaque["signature"], "sig-abc123");
    assert!(matches!(
        events[3],
        ModelEvent::TextDelta {
            block_index: idx,
            ..
        } if idx == ContentBlockIndex::new(1)
    ));
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            ..
        })
    ));
}

/// A thinking block with no visible deltas (display omitted) still creates a
/// reconstructable reasoning block through [`ContinuationState`] alone.
#[tokio::test]
async fn signature_only_thinking_is_reconstructable() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "thinking_signature_only.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "Compute"),
    )
    .await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ModelEvent::ReasoningDelta { .. })),
        "no visible reasoning text was fabricated"
    );
    assert!(matches!(
        events[1],
        ModelEvent::ContinuationState {
            block_index: idx,
            ..
        } if idx == ContentBlockIndex::new(0)
    ));
    let state = anthropic_state_of(&events[1]);
    assert_eq!(state.opaque["signature"], "sig-only-xyz");
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            ..
        })
    ));
}

/// Multiple thinking blocks remain separate canonical reasoning blocks.
#[tokio::test]
async fn multiple_thinking_blocks_stay_separate() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "multiple_thinking.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    let states: Vec<(&ContentBlockIndex, &AnthropicContinuation)> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ContinuationState { block_index, state } => {
                let ProviderContinuationState::Anthropic(continuation) = state else {
                    return None;
                };
                Some((block_index, continuation))
            }
            _ => None,
        })
        .collect();
    assert_eq!(states.len(), 2);
    assert_eq!(*states[0].0, ContentBlockIndex::new(0));
    assert_eq!(states[0].1.opaque["signature"], "sig-1");
    assert_eq!(*states[1].0, ContentBlockIndex::new(1));
    assert_eq!(states[1].1.opaque["signature"], "sig-2");
}

/// `thinking -> tool_use -> thinking -> text` keeps canonical order and
/// independent indexes.
#[tokio::test]
async fn thinking_tool_thinking_text_sequence() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "thinking_tool_thinking_text.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("Inspect")).await;
    let sequence: Vec<&str> = events
        .iter()
        .map(|event| match event {
            ModelEvent::ReasoningDelta { .. } => "reasoning",
            ModelEvent::ToolCallStarted { .. } => "tool_start",
            ModelEvent::ToolCallArgumentsDelta { .. } => "tool_args",
            ModelEvent::ToolCallCompleted { .. } => "tool_completed",
            ModelEvent::TextDelta { .. } => "text",
            ModelEvent::ContinuationState { .. } => "state",
            ModelEvent::Started => "started",
            ModelEvent::Completed { .. } => "completed",
            ModelEvent::Failed { .. } => "failed",
            ModelEvent::UsageUpdate { .. } => "usage",
            ModelEvent::RefusalDelta { .. } => "refusal",
        })
        .collect();
    assert_eq!(
        sequence,
        vec![
            "started",
            "reasoning",
            "state",
            "tool_start",
            "tool_args",
            "tool_args",
            "tool_completed",
            "reasoning",
            "state",
            "text",
            "completed",
        ],
        "{}",
        describe_events(&events)
    );
    // Canonical indexes: reasoning(0), tool(1), reasoning(2), text(3), plus
    // the state event targets the second reasoning block.
    let tool_start_index = events
        .iter()
        .find_map(|event| match event {
            ModelEvent::ToolCallStarted { block_index, .. } => Some(*block_index),
            _ => None,
        })
        .expect("tool start");
    assert_eq!(tool_start_index, ContentBlockIndex::new(1));
    let text_index = events
        .iter()
        .find_map(|event| match event {
            ModelEvent::TextDelta { block_index, .. } => Some(*block_index),
            _ => None,
        })
        .expect("text");
    assert_eq!(text_index, ContentBlockIndex::new(3));
    let state_indexes: Vec<ContentBlockIndex> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ContinuationState { block_index, .. } => Some(*block_index),
            _ => None,
        })
        .collect();
    assert_eq!(
        state_indexes,
        vec![ContentBlockIndex::new(0), ContentBlockIndex::new(2)],
        "each thinking block carries its own state"
    );
}

/// Tool use streams start, raw fragments, and one parsed completion at block
/// stop, with finish reason `ToolCalls`.
#[tokio::test]
async fn tool_use_streams_and_completes() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "tool_use.sse"))
            .await;
    let events = collect_events(&adapter(&server), request_with_tools("Weather")).await;
    assert!(matches!(
        events[1],
        ModelEvent::ToolCallStarted {
            block_index: idx,
            ref call,
        } if idx == ContentBlockIndex::new(0)
            && call.id == ToolCallId::new("toolu_07")
            && call.name == "get_weather"
    ));
    let fragments: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ToolCallArgumentsDelta {
                arguments_delta, ..
            } => Some(arguments_delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        fragments,
        vec![r#"{"location":"#, " \"San", " Francisco\"}"]
    );
    assert!(matches!(
        events[events.len() - 2],
        ModelEvent::ToolCallCompleted {
            block_index: idx,
            ref call,
        } if idx == ContentBlockIndex::new(0)
            && call.arguments == serde_json::json!({"location": "San Francisco"})
    ));
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            ..
        })
    ));
}

/// Multiple tool calls keep independent canonical indexes.
#[tokio::test]
async fn multiple_tool_calls_keep_independent_indexes() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "multiple_tool_calls.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("Do both")).await;
    let completed: Vec<(ContentBlockIndex, &str)> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ToolCallCompleted { block_index, call } => {
                Some((*block_index, call.name.as_str()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(completed.len(), 2);
    assert_eq!(completed[0].0, ContentBlockIndex::new(0));
    assert_eq!(completed[0].1, "read_file");
    assert_eq!(completed[1].0, ContentBlockIndex::new(1));
    assert_eq!(completed[1].1, "list_directory");
}

/// A fallback block consumes no canonical index: provider index 1 (fallback)
/// does not shift canonical indexes 0 and 1.
#[tokio::test]
async fn fallback_block_does_not_shift_canonical_indexes() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "fallback.sse"))
            .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    let text: Vec<(ContentBlockIndex, &str)> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::TextDelta { block_index, text } => Some((*block_index, text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        text,
        vec![
            (ContentBlockIndex::new(0), "Before fallback."),
            (ContentBlockIndex::new(1), "After fallback."),
        ],
        "provider fallback indexes must not shift canonical indexes"
    );
}

/// `stop_sequence` and `max_tokens` finish mapping.
#[tokio::test]
async fn stop_reasons_map_explicitly() {
    for (fixture, expected) in [
        ("stop_sequence.sse", ModelFinishReason::Stop),
        ("max_tokens.sse", ModelFinishReason::Length),
        ("context_window_exceeded.sse", ModelFinishReason::Length),
    ] {
        let fixture = fixture.to_owned();
        let fixture_for_server = fixture.clone();
        let server = common::FixtureServer::start(move |_attempt, _head| {
            sse_fixture("anthropic", &fixture_for_server)
        })
        .await;
        let events = collect_events(
            &adapter(&server),
            simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
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
    }
}

/// `pause_turn` is preserved with its continuation semantics, never mapped to
/// an ordinary stop.
#[tokio::test]
async fn pause_turn_is_other_not_stop() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "pause_turn.sse"))
            .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Other { reason },
            ..
        }) if reason == "pause_turn"
    ));
}

/// A refusal is a successful stop condition, not a failure: partial text
/// output is discarded per provider semantics and the finish reason is
/// Refusal.
#[tokio::test]
async fn refusal_is_a_successful_stop_condition() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "refusal.sse"))
            .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "No"),
    )
    .await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ModelEvent::TextDelta { .. })),
        "partial refusal output must be discarded, never emitted as text"
    );
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Refusal,
            ..
        })
    ));
    assert_eq!(events.len(), 2, "{}", describe_events(&events));
}

/// An error SSE event fails the invocation with the provider code preserved.
#[tokio::test]
async fn error_event_fails_with_provider_code() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "error_event.sse"))
            .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    let ModelEvent::Failed { error } = events.last().expect("terminal") else {
        panic!("expected Failed");
    };
    assert_eq!(error.kind, ModelErrorKind::ProviderError);
    assert_eq!(error.provider_code.as_deref(), Some("overloaded_error"));
    assert!(error.message.contains("Overloaded"));
}

/// Unknown top-level events and pings do not crash the parser; output after
/// them still normalizes.
#[tokio::test]
async fn unknown_top_level_events_are_tolerated() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "unknown_events.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert!(events.iter().any(
        |event| matches!(event, ModelEvent::TextDelta { text, .. } if text == "Still works.")
    ));
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            ..
        })
    ));
}

/// Provider-hosted tool blocks (`server_tool_use`) are rejected explicitly and
/// never become rustX tool calls.
#[tokio::test]
async fn server_tool_use_is_unsupported() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "server_tool_use.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::Unsupported);
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, ModelEvent::ToolCallStarted { .. }))
    );
}

/// Malformed complete tool JSON terminates the stream with a failure.
#[tokio::test]
async fn malformed_tool_json_fails() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "malformed_tool_json.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("Broken")).await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
}

/// An interrupted SSE stream is a normalized transport failure.
#[tokio::test]
async fn interrupted_stream_fails() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "interrupted.sse"))
            .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
}

/// `message_stop` without a stop reason is a provider protocol violation.
#[tokio::test]
async fn missing_stop_reason_fails() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "no_stop_reason.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
}

/// A model calling an unknown tool name fails with `InvalidRequest`.
#[tokio::test]
async fn unknown_tool_name_fails() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "unknown_tool_name.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("Call")).await;
    assert_terminal_failed(&events, &ModelErrorKind::InvalidRequest);
}

/// HTTP error mapping for the direct Anthropic transport, including
/// Retry-After extraction and context-window detection.
#[tokio::test]
async fn http_errors_normalize() {
    type ErrorCase = (
        &'static str,
        u16,
        &'static str,
        Option<(&'static str, &'static str)>,
        ModelErrorKind,
    );
    let cases: Vec<ErrorCase> = vec![
        (
            "anthropic_401.json",
            401,
            "Unauthorized",
            None,
            ModelErrorKind::Authentication,
        ),
        (
            "anthropic_429.json",
            429,
            "Too Many Requests",
            Some(("Retry-After", "7")),
            ModelErrorKind::RateLimit,
        ),
        (
            "anthropic_400_invalid.json",
            400,
            "Bad Request",
            None,
            ModelErrorKind::InvalidRequest,
        ),
        (
            "anthropic_400_too_long.json",
            400,
            "Bad Request",
            None,
            ModelErrorKind::ContextWindowExceeded,
        ),
        (
            "anthropic_500.json",
            500,
            "Internal Server Error",
            None,
            ModelErrorKind::ProviderError,
        ),
        (
            "anthropic_529.json",
            529,
            "Overloaded",
            None,
            ModelErrorKind::ProviderError,
        ),
    ];
    for (fixture, status, reason, header, expected_kind) in cases {
        let fixture = fixture.to_owned();
        let fixture_for_server = fixture.clone();
        let header = header.map(|(name, value)| (name.to_owned(), value.to_owned()));
        let server = common::FixtureServer::start(move |_attempt, _head| {
            let mut reply = error_fixture(&fixture_for_server);
            reply.status = status;
            reply.reason = reason;
            if let Some((name, value)) = &header {
                reply.headers.push((name.clone(), value.clone()));
            }
            reply
        })
        .await;
        let events = collect_events(
            &adapter(&server),
            simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
        )
        .await;
        assert_eq!(events.len(), 2, "fixture {fixture}");
        let ModelEvent::Failed { error } = &events[1] else {
            panic!("fixture {fixture}: expected Failed");
        };
        assert_eq!(error.kind, expected_kind, "fixture {fixture}");
        if fixture == "anthropic_429.json" {
            assert_eq!(error.retry_after_ms, Some(7000));
            assert_eq!(error.provider_code.as_deref(), Some("rate_limit_error"));
        }
    }
}

/// The Anthropic request carries only model-facing tool fields, an exact
/// reasoning effort mapping, and the required `max_tokens` default.
#[tokio::test]
async fn request_serialization_is_model_facing_only() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let mut request = request_with_tools("hi");
    request.reasoning = rustx::model::ReasoningEffort::High;
    let events = collect_events(&adapter(&server), request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert_eq!(body["model"], "claude-test");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 4096, "documented default when unset");
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["thinking"]["display"], "high");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"][0]["type"], "text");
    let tool = &body["tools"][0];
    assert_eq!(tool["name"], "list_directory");
    assert!(tool["input_schema"].is_object());
    for runtime_field in ["execution_mode", "replay_policy", "origin", "id"] {
        assert!(
            tool.get(runtime_field).is_none(),
            "runtime-only field {runtime_field} must not reach the provider"
        );
    }
}

/// `Anthropic` rejects `ReasoningEffort::Minimal` before the network; it is never
/// remapped to another effort level.
#[tokio::test]
async fn minimal_reasoning_effort_is_unsupported() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let mut request = simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi");
    request.reasoning = rustx::model::ReasoningEffort::Minimal;
    let events = collect_events(&adapter(&server), request).await;
    assert_eq!(events.len(), 1);
    assert_terminal_failed(&events, &ModelErrorKind::Unsupported);
    assert_eq!(server.attempt_count(), 0, "no provider request");
}

/// Previous signed thinking is replayed from its opaque provider state, never
/// reconstructed from canonical text.
#[tokio::test]
async fn previous_thinking_replays_from_opaque_state() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let mut request = simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi");
    let opaque = serde_json::json!({
        "type": "thinking",
        "thinking": "Previous chain.",
        "signature": "sig-prev",
    });
    // The canonical history contains a prior agent message with a signed
    // thinking block whose state was captured by the adapter.
    request.messages.insert(
        0,
        rustx::message::types::MessageBlock::Agent(rustx::message::types::AgentMessageBlock {
            id: rustx::runtime::identity::MessageId::new("msg-prev"),
            content: vec![rustx::message::types::AgentContentBlock::Reasoning(
                rustx::message::types::ReasoningBlock {
                    text: Some("Previous chain.".to_owned()),
                    provider_state: Some(ProviderContinuationState::Anthropic(
                        AnthropicContinuation {
                            opaque: opaque.clone(),
                        },
                    )),
                },
            )],
        }),
    );
    let events = collect_events(&adapter(&server), request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let assistant = &body["messages"][0];
    assert_eq!(assistant["role"], "assistant");
    let thinking = &assistant["content"][0];
    assert_eq!(thinking["type"], "thinking");
    assert_eq!(thinking["thinking"], "Previous chain.");
    assert_eq!(thinking["signature"], "sig-prev");
    assert_eq!(assistant["content"].as_array().map(Vec::len), Some(1));
}

/// Previous thinking without provider state cannot be replayed as a signed
/// block; it fails explicitly instead of flattening into text.
#[tokio::test]
async fn stateless_previous_thinking_fails_explicitly() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let mut request = simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi");
    request.messages.insert(
        0,
        rustx::message::types::MessageBlock::Agent(rustx::message::types::AgentMessageBlock {
            id: rustx::runtime::identity::MessageId::new("msg-prev"),
            content: vec![rustx::message::types::AgentContentBlock::Reasoning(
                rustx::message::types::ReasoningBlock {
                    text: Some("Unsigned chain.".to_owned()),
                    provider_state: None,
                },
            )],
        }),
    );
    let events = collect_events(&adapter(&server), request).await;
    assert_eq!(events.len(), 1, "rejected before the network");
    assert_terminal_failed(&events, &ModelErrorKind::Unsupported);
    assert_eq!(server.attempt_count(), 0);
}

/// Tool results merge consecutive canonical tool messages into one provider
/// user message of `tool_result` blocks.
#[tokio::test]
async fn tool_results_merge_into_one_user_message() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let mut request = simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi");
    let make_tool_message = |id: &str| {
        rustx::message::types::MessageBlock::Tool(rustx::message::types::ToolMessageBlock {
            id: rustx::runtime::identity::MessageId::new(format!("msg-{id}")),
            tool_call_id: ToolCallId::new(id),
            tool_id: rustx::runtime::identity::ToolId::new("tool-list"),
            result: rustx::tools::types::ToolExecutionResult {
                status: rustx::tools::types::ToolExecutionStatus::Success,
                content: vec![rustx::tools::types::ToolResultContent::Text(
                    rustx::message::content::TextBlock {
                        text: format!("result-{id}"),
                    },
                )],
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
            },
        })
    };
    request.messages.push(make_tool_message("call_a"));
    request.messages.push(make_tool_message("call_b"));
    let events = collect_events(&adapter(&server), request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let messages = body["messages"].as_array().expect("messages");
    let user = messages.last().expect("last message");
    assert_eq!(user["role"], "user");
    let content = user["content"].as_array().expect("content");
    assert_eq!(
        content.len(),
        2,
        "both tool_results merged into one user message"
    );
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "call_a");
    assert_eq!(content[0]["content"][0]["text"], "result-call_a");
    assert_eq!(content[1]["tool_use_id"], "call_b");
}

/// The continuation boundary for Anthropic: request-level continuation state
/// must match the boundary reasoning block or fill it when absent.
#[tokio::test]
async fn continuation_contradiction_with_boundary_is_rejected() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let mut request = simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi");
    request.messages.insert(
        0,
        rustx::message::types::MessageBlock::Agent(rustx::message::types::AgentMessageBlock {
            id: rustx::runtime::identity::MessageId::new("msg-prev"),
            content: vec![rustx::message::types::AgentContentBlock::Reasoning(
                rustx::message::types::ReasoningBlock {
                    text: None,
                    provider_state: Some(ProviderContinuationState::Anthropic(
                        AnthropicContinuation {
                            opaque: serde_json::json!({"signature": "sig-a"}),
                        },
                    )),
                },
            )],
        }),
    );
    request.continuation = Some(ProviderContinuationState::Anthropic(
        AnthropicContinuation {
            opaque: serde_json::json!({"signature": "sig-b"}),
        },
    ));
    let events = collect_events(&adapter(&server), request).await;
    assert_eq!(events.len(), 1);
    assert_terminal_failed(&events, &ModelErrorKind::InvalidRequest);
    assert_eq!(server.attempt_count(), 0);
}

/// The stream lifecycle has exactly one terminal event.
#[tokio::test]
async fn lifecycle_has_one_terminal_event() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    let terminals: Vec<&ModelEvent> = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                ModelEvent::Completed { .. } | ModelEvent::Failed { .. }
            )
        })
        .collect();
    assert_eq!(terminals.len(), 1);
}
