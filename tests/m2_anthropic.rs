//! Deterministic Anthropic Messages adapter tests.
//!
//! All tests drive the real adapter over a local fixture HTTP server serving
//! provider-shaped SSE streams.

mod common;

use common::{
    collect_events, describe_events, error_fixture, model_tool, simple_request, sse_fixture,
};
use rustx::message::types::{ContentBlockIndex, MessageBlock};
use rustx::model::finish::ModelFinishReason;
use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ModelErrorKind, ModelEvent, ModelProtocol,
    ModelRequest, ModelUsage,
};
use rustx::runtime::continuation::{AnthropicContinuation, ProviderContinuationState};
use rustx::runtime::identity::ToolCallId;

fn adapter(server: &common::FixtureServer) -> AnthropicMessagesAdapter {
    AnthropicMessagesAdapter::new(AnthropicAdapterConfig::new("test-key", server.url("")))
}

fn request_with_tools(prompt: &str) -> ModelRequest {
    let mut request = simple_request(ModelProtocol::AnthropicMessages, "claude-test", prompt);
    request.tools = vec![
        model_tool("list_directory", "tool-list"),
        model_tool("read_file", "tool-read"),
        model_tool("get_weather", "tool-weather"),
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

/// Basic text streaming: provider `text_delta` fragments stream incrementally
/// as they arrive (never buffered until `message_stop`), usage combines the
/// cumulative snapshots including every provider input-token category, and
/// the stream completes with Stop.
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
            text: "Hello".to_owned(),
        },
        "the first provider fragment streams immediately"
    );
    assert_eq!(
        events[2],
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: " world".to_owned(),
        },
        "the second provider fragment streams as its own delta"
    );
    assert_eq!(
        events[3],
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: Some(ModelUsage {
                input_tokens: 30,
                output_tokens: 15,
                total_tokens: 45,
                details: Some(rustx::model::UsageDetails {
                    reasoning_tokens: None,
                    cached_input_tokens: Some(5),
                }),
            }),
        },
        "input = 25 base + 0 cache creation + 5 cache read; snapshots never summed"
    );
    assert_eq!(events.len(), 4, "{}", describe_events(&events));
}

/// Non-empty text/thinking values in `content_block_start` are real output,
/// not placeholders, and are emitted before later deltas.
#[tokio::test]
async fn content_block_start_values_are_not_lost() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "initial_text_thinking.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    let visible: Vec<(&str, &str)> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::TextDelta { text, .. } => Some(("text", text.as_str())),
            ModelEvent::ReasoningDelta { text, .. } => Some(("reasoning", text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        visible,
        vec![
            ("text", "Initial "),
            ("text", "text."),
            ("reasoning", "Initial thinking."),
        ]
    );
    assert!(events.iter().any(|event| matches!(
        event,
        ModelEvent::ContinuationState { block_index, .. } if block_index.get() == 1
    )));
}

/// A complete tool input carried by `content_block_start` produces the same
/// canonical start/delta/completion lifecycle without requiring JSON deltas.
#[tokio::test]
async fn initial_tool_input_is_mapped_completely() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "initial_tool_input.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("List")).await;
    assert!(matches!(
        &events[1],
        ModelEvent::ToolCallStarted { call, .. } if call.id == ToolCallId::new("toolu_initial")
    ));
    assert!(matches!(
        &events[2],
        ModelEvent::ToolCallArgumentsDelta { arguments_delta, .. }
            if arguments_delta == "{\"path\":\".\"}"
    ));
    assert!(matches!(
        &events[3],
        ModelEvent::ToolCallCompleted { call, .. }
            if call.arguments == serde_json::json!({"path": "."})
    ));
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

/// Thinking deltas stream incrementally; the signature closes the block and
/// the continuation state is emitted at block stop.
#[tokio::test]
async fn thinking_deltas_stream_incrementally() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "thinking_streaming.sse")
    })
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
            text: "Let ".to_owned(),
        }
    );
    assert_eq!(
        events[2],
        ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(0),
            text: "me think".to_owned(),
        }
    );
    let state = anthropic_state_of(&events[3]);
    assert_eq!(state.opaque["type"], "thinking");
    assert_eq!(state.opaque["thinking"], "Let me think");
    assert_eq!(state.opaque["signature"], "sig-stream-1");
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            ..
        })
    ));
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

/// A provider `fallback` block is not disposable transport metadata: it
/// carries positional/replay semantics rustX cannot preserve losslessly, so
/// the adapter terminates immediately with `Unsupported`. Prior streamed
/// content stays provisional output; no canonical fallback block exists, no
/// continuation state is fabricated, and nothing is emitted after the
/// terminal event.
#[tokio::test]
async fn fallback_block_is_unsupported() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "fallback.sse"))
            .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert_eq!(events[0], ModelEvent::Started);
    assert_eq!(
        events[1],
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "Before fallback.".to_owned(),
        },
        "prior streamed output remains provisional adapter output"
    );
    assert_terminal_failed(&events, &ModelErrorKind::Unsupported);
    assert_eq!(
        events.len(),
        3,
        "Started, TextDelta, Failed — nothing after the terminal event: {}",
        describe_events(&events)
    );
    assert!(
        !events.iter().any(
            |event| matches!(event, ModelEvent::TextDelta { text, .. } if text == "After fallback.")
        ),
        "no canonical content after the fallback block"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ModelEvent::Completed { .. })),
        "no Completed after an Unsupported fallback"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ModelEvent::ContinuationState { .. })),
        "no fabricated continuation state for the fallback block"
    );
    let terminals: Vec<&ModelEvent> = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                ModelEvent::Completed { .. } | ModelEvent::Failed { .. }
            )
        })
        .collect();
    assert_eq!(terminals.len(), 1, "exactly one terminal event");
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

/// A refusal is a successful stop condition, not a failure. The provider's
/// partial streamed output is reported as provisional `TextDelta` output (M2
/// never becomes a commit/rollback agent loop), the `stop_details.explanation`
/// streams as `RefusalDelta` on its own deterministic block, and the finish
/// reason is Refusal.
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
    assert_eq!(events[0], ModelEvent::Started);
    assert_eq!(
        events[1],
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "Partial output that must be discarded.".to_owned(),
        },
        "the provider actually streamed this text; M2 reports what it streamed"
    );
    assert_eq!(
        events[2],
        ModelEvent::RefusalDelta {
            block_index: ContentBlockIndex::new(1),
            text: "declined".to_owned(),
        },
        "the refusal explanation streams as refusal, never as ordinary text"
    );
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Refusal,
            ..
        })
    ));
    assert_eq!(events.len(), 4, "{}", describe_events(&events));
}

/// A refusal without a `stop_details.explanation` emits no fabricated refusal
/// text; only the Refusal finish reason is reported.
#[tokio::test]
async fn refusal_without_explanation_does_not_fabricate_text() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        let mut reply = sse_fixture("anthropic", "refusal.sse");
        reply.chunks.clear();
        reply.chunks.push(common::FixtureChunk {
            delay_ms: 0,
            bytes: [
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_14\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-test\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"stop_details\":null,\"usage\":{\"input_tokens\":412,\"output_tokens\":1}}}\n\n".to_string(),
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"refusal\",\"stop_sequence\":null},\"stop_details\":{\"type\":\"refusal\",\"category\":null,\"explanation\":null},\"usage\":{\"output_tokens\":0,\"input_tokens\":412}}\n\n".to_string(),
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_string(),
            ]
            .concat()
            .into_bytes(),
        });
        reply
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "No"),
    )
    .await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ModelEvent::RefusalDelta { .. })),
        "no refusal text is fabricated when the provider reports none"
    );
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Refusal,
            ..
        })
    ));
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

/// Streaming error discriminators map to canonical error kinds just like
/// HTTP errors, while preserving the provider code.
#[tokio::test]
async fn rate_limit_stream_error_maps_semantically() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "rate_limit_error_event.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    let ModelEvent::Failed { error } = events.last().expect("terminal") else {
        panic!("expected Failed");
    };
    assert_eq!(error.kind, ModelErrorKind::RateLimit);
    assert_eq!(error.provider_code.as_deref(), Some("rate_limit_error"));
}

/// `OpenRouter`'s stable `error_type` wins over the lossy Anthropic-native
/// `api_error` wrapper and remains available as the provider code.
#[tokio::test]
async fn openrouter_anthropic_error_type_maps_semantically() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "openrouter_typed_error.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "router/claude", "hi"),
    )
    .await;
    let ModelEvent::Failed { error } = events.last().expect("terminal") else {
        panic!("expected Failed");
    };
    assert_eq!(error.kind, ModelErrorKind::Authentication);
    assert_eq!(error.provider_code.as_deref(), Some("authentication"));
}

/// Current Anthropic responses carry refusal `stop_details` inside the message
/// delta object; the explanation becomes canonical refusal text.
#[tokio::test]
async fn nested_refusal_stop_details_are_mapped() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "nested_refusal.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert!(events.iter().any(|event| matches!(
        event,
        ModelEvent::RefusalDelta { text, .. } if text == "Request declined."
    )));
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Refusal,
            ..
        })
    ));
}

/// Known citation semantics are not silently erased, and malformed block
/// lifecycles fail as provider protocol errors.
#[tokio::test]
async fn citations_and_invalid_block_lifecycles_fail_explicitly() {
    let cases = [
        ("citation_delta.sse", ModelErrorKind::Unsupported),
        ("stop_without_start.sse", ModelErrorKind::ProviderError),
        ("unclosed_block.sse", ModelErrorKind::ProviderError),
        ("nonempty_message_start.sse", ModelErrorKind::ProviderError),
    ];
    for (fixture, expected) in cases {
        let response_fixture = fixture.to_owned();
        let server = common::FixtureServer::start(move |_attempt, _head| {
            sse_fixture("anthropic", &response_fixture)
        })
        .await;
        let events = collect_events(
            &adapter(&server),
            simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
        )
        .await;
        assert_terminal_failed(&events, &expected);
    }
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

/// The Anthropic request carries only model-facing tool fields and sends the
/// runtime-resolved `max_tokens` explicitly.
///
/// It also proves the retired universal reasoning mapping is gone: with no
/// configured request parameters the adapter synthesizes **no** `thinking`
/// and **no** `output_config` field at all.
#[tokio::test]
async fn request_serialization_is_model_facing_only() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let mut request = request_with_tools("hi");
    request.invocation.max_output_tokens = 4096;
    let events = collect_events(&adapter(&server), request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert_eq!(body["model"], "claude-test");
    assert_eq!(body["stream"], true);
    assert_eq!(
        body["max_tokens"], 4096,
        "the runtime-resolved output limit is sent explicitly"
    );
    assert!(
        body.get("thinking").is_none() && body.get("output_config").is_none(),
        "no legacy reasoning field is ever synthesized: {body}"
    );
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

/// A reasoning profile's configured request parameters reach the Anthropic
/// wire **exactly**, and nothing else appears.
///
/// The two profiles deliberately use different provider-specific shapes, so
/// the assertion cannot pass by an enum conversion: the wire JSON is exactly
/// the configured overlay.
#[tokio::test]
async fn reasoning_profiles_produce_their_exact_configured_overlay() {
    let cases = [
        (
            serde_json::json!({"thinking": {"type": "disabled"}, "temperature": 0.7}),
            "off",
        ),
        (
            serde_json::json!({
                "thinking": {"type": "enabled", "budget_tokens": 32_000},
                "output_config": {"effort": "high"},
                "temperature": 1.0
            }),
            "on",
        ),
    ];
    for (params, label) in cases {
        let server =
            common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse"))
                .await;
        let mut request = simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi");
        request.invocation.request_params = common::request_params(params.clone());
        let events = collect_events(&adapter(&server), request).await;
        assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
        let body: serde_json::Value =
            serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
        let configured = params.as_object().expect("object");
        for (key, value) in configured {
            assert_eq!(&body[key], value, "profile {label} key {key}");
        }
        // Nothing beyond the runtime-owned structural fields and the exact
        // configured overlay appears.
        let allowed: std::collections::BTreeSet<&str> =
            ["model", "max_tokens", "messages", "stream"]
                .into_iter()
                .chain(configured.keys().map(String::as_str))
                .collect();
        for key in body.as_object().expect("object").keys() {
            assert!(
                allowed.contains(key.as_str()),
                "profile {label} produced an unexpected wire field {key}: {body}"
            );
        }
    }
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
    // The canonical history contains a prior Assistant message with a signed
    // thinking block whose state was captured by the adapter.
    request.messages.insert(
        0,
        rustx::message::types::MessageBlock::Assistant(
            rustx::message::types::AssistantMessageBlock {
                id: rustx::runtime::identity::MessageId::new("msg-prev"),
                content: vec![rustx::message::types::AssistantContentBlock::Reasoning(
                    rustx::message::types::ReasoningBlock {
                        text: Some("Previous chain.".to_owned()),
                        provider_state: Some(ProviderContinuationState::Anthropic(
                            AnthropicContinuation {
                                opaque: opaque.clone(),
                            },
                        )),
                    },
                )],
            },
        ),
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
        rustx::message::types::MessageBlock::Assistant(
            rustx::message::types::AssistantMessageBlock {
                id: rustx::runtime::identity::MessageId::new("msg-prev"),
                content: vec![rustx::message::types::AssistantContentBlock::Reasoning(
                    rustx::message::types::ReasoningBlock {
                        text: Some("Unsigned chain.".to_owned()),
                        provider_state: None,
                    },
                )],
            },
        ),
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
                managed_output: None,
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
        rustx::message::types::MessageBlock::Assistant(
            rustx::message::types::AssistantMessageBlock {
                id: rustx::runtime::identity::MessageId::new("msg-prev"),
                content: vec![rustx::message::types::AssistantContentBlock::Reasoning(
                    rustx::message::types::ReasoningBlock {
                        text: None,
                        provider_state: Some(ProviderContinuationState::Anthropic(
                            AnthropicContinuation {
                                opaque: serde_json::json!({"signature": "sig-a"}),
                            },
                        )),
                    },
                )],
            },
        ),
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

/// `redacted_thinking.data` survives losslessly into the opaque Anthropic
/// continuation state: the provider block becomes a canonical reasoning block
/// with no fabricated visible text, and the full provider object replays
/// verbatim on a later request.
#[tokio::test]
async fn redacted_thinking_preserves_opaque_data() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "redacted_thinking.sse")
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
        "no visible reasoning text is fabricated for a redacted block"
    );
    let state = anthropic_state_of(&events[1]);
    assert_eq!(
        state.opaque,
        serde_json::json!({
            "type": "redacted_thinking",
            "data": "opaque-redacted-provider-data",
        }),
        "the provider block is preserved losslessly as its full provider object"
    );
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            ..
        })
    ));
}

/// The preserved `redacted_thinking` provider object replays verbatim in the
/// next request's assistant message: opaque state → canonical serialization →
/// provider replay, without loss.
#[tokio::test]
async fn redacted_thinking_replays_losslessly() {
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let mut request = simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi");
    let opaque = serde_json::json!({
        "type": "redacted_thinking",
        "data": "opaque-redacted-provider-data",
    });
    request.messages.insert(
        0,
        rustx::message::types::MessageBlock::Assistant(
            rustx::message::types::AssistantMessageBlock {
                id: rustx::runtime::identity::MessageId::new("msg-redacted"),
                content: vec![rustx::message::types::AssistantContentBlock::Reasoning(
                    rustx::message::types::ReasoningBlock {
                        text: None,
                        provider_state: Some(ProviderContinuationState::Anthropic(
                            AnthropicContinuation {
                                opaque: opaque.clone(),
                            },
                        )),
                    },
                )],
            },
        ),
    );
    let events = collect_events(&adapter(&server), request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let assistant = &body["messages"][0];
    assert_eq!(assistant["role"], "assistant");
    let block = &assistant["content"][0];
    assert_eq!(
        block["type"], "redacted_thinking",
        "the provider block type is preserved"
    );
    assert_eq!(
        block["data"], "opaque-redacted-provider-data",
        "the opaque provider state replays verbatim"
    );
    assert_eq!(
        block, &opaque,
        "the replayed block is the exact preserved provider object"
    );
}

/// Effective input usage includes every provider input-token category
/// (`input_tokens` + `cache_creation_input_tokens` + `cache_read_input_tokens`),
/// thinking tokens map to `UsageDetails.reasoning_tokens`, and cumulative
/// `message_delta` snapshots are used as-is, never summed over time.
#[tokio::test]
async fn usage_accounts_for_cache_categories_and_thinking_tokens() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "usage_cached_thinking.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    let ModelEvent::Completed { usage, .. } = events.last().expect("terminal") else {
        panic!("expected Completed");
    };
    let usage = usage.as_ref().expect("usage reported");
    assert_eq!(
        usage.input_tokens,
        2095 + 2051 + 2051,
        "input = base + cache creation + cache read"
    );
    assert_eq!(usage.output_tokens, 510);
    assert_eq!(usage.total_tokens, usage.input_tokens + usage.output_tokens);
    let details = usage.details.as_ref().expect("details reported");
    assert_eq!(
        details.reasoning_tokens,
        Some(128),
        "thinking_tokens mapped"
    );
    assert_eq!(
        details.cached_input_tokens,
        Some(2051),
        "cache_read mapped once, not double counted"
    );
}

/// A provider content-block event with a missing index is a hard protocol
/// failure: it is never reinterpreted as index 0 and no canonical block
/// events are emitted.
#[tokio::test]
async fn missing_block_index_is_a_provider_failure() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "missing_index.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ModelEvent::TextDelta { .. })),
        "no canonical block events may follow an index violation"
    );
}

/// A provider content-block event with a non-integer index is a hard protocol
/// failure with the same guarantees.
#[tokio::test]
async fn invalid_block_index_is_a_provider_failure() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "invalid_index.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ModelEvent::TextDelta { .. }))
    );
}

/// A `redacted_thinking` block without its required opaque `data` field is a
/// provider protocol error; an empty fabricated opaque value is never
/// preserved.
#[tokio::test]
async fn redacted_thinking_without_data_is_a_provider_failure() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "redacted_missing_data.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "hi"),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ModelEvent::ContinuationState { .. })),
        "no fabricated opaque continuation state"
    );
}

/// A thinking block that stops without a `signature_delta` is a provider
/// protocol error: without its provider signature the block cannot be
/// replayed losslessly, so a state-less reasoning block is never emitted.
#[tokio::test]
async fn thinking_without_signature_is_a_provider_failure() {
    let server = common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("anthropic", "thinking_missing_signature.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::AnthropicMessages, "claude-test", "Compute"),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ModelEvent::ContinuationState { .. })),
        "no continuation state without a provider signature"
    );
}

/// `Assistant tool call → Tool result → User A → User B` translates in logical
/// order: the complete tool-result group flushes before the inbound
/// messages, and A/B stay separate wire user messages (never merged).
#[tokio::test]
async fn tool_then_consecutive_inbound_users_translate_in_order() {
    use rustx::message::types::{AssistantMessageBlock, ToolMessageBlock, UserMessageBlock};
    use rustx::runtime::identity::MessageId;
    let server =
        common::FixtureServer::start(|_attempt, _head| sse_fixture("anthropic", "text.sse")).await;
    let mut request = request_with_tools("hi");
    let user = |id: &str, text: &str| {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![rustx::message::types::UserContentBlock::Text(
                rustx::message::content::TextBlock {
                    text: text.to_owned(),
                },
            )],
            source: rustx::message::types::UserSource::Human,
            kind: rustx::message::types::InboundKind::Message,
            timestamp: None,
        })
    };
    request.messages = vec![
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("msg-a1"),
            content: vec![rustx::message::types::AssistantContentBlock::ToolCall(
                rustx::tools::types::ToolCall {
                    id: ToolCallId::new("call_1"),
                    tool_id: rustx::runtime::identity::ToolId::new("tool-list"),
                    name: "list_directory".to_owned(),
                    arguments: serde_json::json!({"path": "."}),
                },
            )],
        }),
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("msg-t1"),
            tool_call_id: ToolCallId::new("call_1"),
            tool_id: rustx::runtime::identity::ToolId::new("tool-list"),
            result: rustx::tools::types::ToolExecutionResult {
                status: rustx::tools::types::ToolExecutionStatus::Success,
                content: vec![rustx::tools::types::ToolResultContent::Text(
                    rustx::message::content::TextBlock {
                        text: "listed".to_owned(),
                    },
                )],
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            },
        }),
        user("msg-a", "A"),
        user("msg-b", "B"),
    ];
    let events = collect_events(&adapter(&server), request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 4, "assistant, tool result, user A, user B");
    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[0]["content"][0]["type"], "tool_use");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["type"], "text");
    assert_eq!(messages[2]["content"][0]["text"], "A");
    assert_eq!(messages[3]["role"], "user");
    assert_eq!(messages[3]["content"][0]["type"], "text");
    assert_eq!(messages[3]["content"][0]["text"], "B");
}
