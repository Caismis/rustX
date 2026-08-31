//! Deterministic `OpenAI` Chat Completions adapter tests.
//!
//! Every test drives the real adapter over a local fixture HTTP server with
//! provider-shaped SSE fixtures; no credentials or network are involved.

use crate::common::{
    assert_compiled_execution_mode_schema, collect_events, describe_events, error_fixture,
    model_selectable_tool, model_tool, simple_request, sse_fixture,
};
use rustx::message::types::{ContentBlockIndex, MessageBlock};
use rustx::model::finish::ModelFinishReason;
use rustx::model::{
    ModelErrorKind, ModelEvent, ModelProtocol, ModelRequest, ModelUsage, OpenAiAdapterConfig,
    OpenAiChatCompletionsAdapter, UsageDetails,
};
use rustx::runtime::identity::{ToolCallId, ToolId};
use rustx::tools::types::ToolCall;

fn adapter(server: &crate::common::FixtureServer) -> OpenAiChatCompletionsAdapter {
    OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new("test-key", server.url("/v1")))
}

fn request_with_tools(prompt: &str) -> ModelRequest {
    let mut request = simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", prompt);
    request.tools = vec![
        model_tool("list_directory", "tool-list"),
        model_tool("read_file", "tool-read"),
    ];
    request
}

trait BlockIndexed {
    fn block_index(&self) -> u32;
}

impl BlockIndexed for ModelEvent {
    fn block_index(&self) -> u32 {
        match self {
            ModelEvent::TextDelta { block_index, .. }
            | ModelEvent::ReasoningDelta { block_index, .. }
            | ModelEvent::RefusalDelta { block_index, .. }
            | ModelEvent::ToolCallStarted { block_index, .. }
            | ModelEvent::ToolCallArgumentsDelta { block_index, .. }
            | ModelEvent::ToolCallCompleted { block_index, .. }
            | ModelEvent::ContinuationState { block_index, .. } => block_index.get(),
            _ => panic!("event without a block index"),
        }
    }
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

/// Plain text streams into one text block with exact fragments, usage, and a
/// Stop completion.
#[tokio::test]
async fn plain_text_stream_normalizes() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(
            ModelProtocol::OpenAiChatCompletions,
            "gpt-test",
            "Say hello",
        ),
    )
    .await;
    let expected = vec![
        ModelEvent::Started,
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "Hello".to_owned(),
        },
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: " world".to_owned(),
        },
        ModelEvent::UsageUpdate {
            usage: ModelUsage {
                input_tokens: 12,
                output_tokens: 3,
                total_tokens: 15,
                details: Some(UsageDetails {
                    reasoning_tokens: Some(1),
                    cached_input_tokens: Some(5),
                }),
            },
        },
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: Some(ModelUsage {
                input_tokens: 12,
                output_tokens: 3,
                total_tokens: 15,
                details: Some(UsageDetails {
                    reasoning_tokens: Some(1),
                    cached_input_tokens: Some(5),
                }),
            }),
        },
    ];
    assert_eq!(
        events,
        expected,
        "event stream mismatch:\n{}",
        describe_events(&events)
    );
    assert_eq!(server.attempt_count(), 1, "exactly one provider attempt");
}

/// Qwen/vLLM reasoning deltas remain a distinct canonical block before the
/// visible answer instead of being silently discarded as an unknown field.
#[tokio::test]
async fn reasoning_deltas_normalize_as_reasoning() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "reasoning_then_text.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "Answer"),
    )
    .await;
    let expected = vec![
        ModelEvent::Started,
        ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(0),
            text: "Plan".to_owned(),
        },
        ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(0),
            text: " first.".to_owned(),
        },
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(1),
            text: "Answer".to_owned(),
        },
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
    ];
    assert_eq!(
        events,
        expected,
        "event stream mismatch:\n{}",
        describe_events(&events)
    );
}

/// `DeepSeek`'s `reasoning_content` spelling maps to the same canonical
/// reasoning block and remains distinct from the visible answer.
#[tokio::test]
async fn deepseek_reasoning_content_normalizes_as_reasoning() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "deepseek_reasoning_then_text.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(
            ModelProtocol::OpenAiChatCompletions,
            "deepseek-v4-flash",
            "Answer",
        ),
    )
    .await;
    let expected = vec![
        ModelEvent::Started,
        ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(0),
            text: "Plan".to_owned(),
        },
        ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(0),
            text: " first.".to_owned(),
        },
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(1),
            text: "Answer".to_owned(),
        },
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
    ];
    assert_eq!(
        events,
        expected,
        "event stream mismatch:\n{}",
        describe_events(&events)
    );
}

/// Qwen emits explicit JSON nulls for optional delta fields and a final
/// choices-empty usage chunk; both are valid protocol values.
#[tokio::test]
async fn qwen_null_fields_and_usage_chunk_are_supported() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "qwen_null_fields.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "qwen-plus", "hi"),
    )
    .await;
    assert!(events.iter().any(|event| matches!(
        event,
        ModelEvent::TextDelta { text, .. } if text == "Hello"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ModelEvent::UsageUpdate { usage }
            if usage.input_tokens == 2 && usage.output_tokens == 1 && usage.total_tokens == 3
    )));
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
}

/// Refusal deltas stream as refusal content, never as plain text.
#[tokio::test]
async fn refusal_deltas_normalize_as_refusal() {
    let server =
        crate::common::FixtureServer::start(|_attempt, _head| sse_fixture("openai_chat", "refusal.sse"))
            .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(
            ModelProtocol::OpenAiChatCompletions,
            "gpt-test",
            "Do the bad thing",
        ),
    )
    .await;
    assert_eq!(events[0], ModelEvent::Started);
    assert!(
        events[1..]
            .iter()
            .all(|event| !matches!(event, ModelEvent::TextDelta { .. }))
    );
    assert_eq!(
        events[1],
        ModelEvent::RefusalDelta {
            block_index: ContentBlockIndex::new(0),
            text: "I cannot".to_owned(),
        }
    );
    assert_eq!(
        events[2],
        ModelEvent::RefusalDelta {
            block_index: ContentBlockIndex::new(0),
            text: " comply with that request.".to_owned(),
        }
    );
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            ..
        })
    ));
}

/// A single tool call streams start, raw argument fragments, and a completed
/// call with parsed arguments.
#[tokio::test]
async fn single_tool_call_normalizes() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "single_tool_call.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("List the directory")).await;
    let expected = vec![
        ModelEvent::Started,
        ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: rustx::tools::types::ToolCallStart {
                id: ToolCallId::new("call_abc"),
                tool_id: ToolId::new("tool-list"),
                name: "list_directory".to_owned(),
            },
        },
        ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(0),
            call_id: ToolCallId::new("call_abc"),
            arguments_delta: r#"{"path":".""#.to_owned(),
        },
        ModelEvent::ToolCallArgumentsDelta {
            block_index: ContentBlockIndex::new(0),
            call_id: ToolCallId::new("call_abc"),
            arguments_delta: "}".to_owned(),
        },
        ModelEvent::ToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call: ToolCall {
                id: ToolCallId::new("call_abc"),
                tool_id: ToolId::new("tool-list"),
                name: "list_directory".to_owned(),
                arguments: serde_json::json!({"path": "."}),
            },
        },
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        },
    ];
    assert_eq!(
        events,
        expected,
        "event stream mismatch:\n{}",
        describe_events(&events)
    );
}

/// Multiple tool calls keep stable independent canonical indexes while their
/// argument fragments interleave.
#[tokio::test]
async fn multiple_interleaved_tool_calls_normalize() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "multiple_tool_calls.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("Read and list")).await;
    assert_eq!(events[0], ModelEvent::Started);
    assert_eq!(events[1].block_index(), 0);
    assert_eq!(events[2].block_index(), 1);
    let completed: Vec<&ToolCall> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ToolCallCompleted { call, .. } => Some(call),
            _ => None,
        })
        .collect();
    assert_eq!(completed.len(), 2);
    assert_eq!(completed[0].id, ToolCallId::new("call_1"));
    assert_eq!(completed[0].tool_id, ToolId::new("tool-read"));
    assert_eq!(completed[0].name, "read_file");
    assert_eq!(
        completed[0].arguments,
        serde_json::json!({"path": "/tmp/opencode/a.txt"})
    );
    assert_eq!(completed[1].id, ToolCallId::new("call_2"));
    assert_eq!(completed[1].tool_id, ToolId::new("tool-list"));
    assert_eq!(completed[1].arguments, serde_json::json!({"path": "/home"}));
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            ..
        })
    ));
}

/// Argument fragments that precede the provider call id/name are buffered,
/// then emitted exactly once when the call becomes attributable.
#[tokio::test]
async fn tool_arguments_before_identity_are_not_lost() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "tool_arguments_before_identity.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("List")).await;
    assert!(matches!(
        &events[1],
        ModelEvent::ToolCallStarted { call, .. }
            if call.id == ToolCallId::new("call-late") && call.name == "list_directory"
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
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
}

/// `MiniMax` may terminate with `delta: null` and a cumulative `message`
/// snapshot. Streamed text is not duplicated, while snapshot-only tool calls
/// are recovered into the ordinary canonical tool lifecycle.
#[tokio::test]
async fn minimax_terminal_message_snapshots_are_normalized() {
    let text_server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "minimax_snapshot_text.sse")
    })
    .await;
    let text_events = collect_events(
        &adapter(&text_server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "MiniMax-M2", "hi"),
    )
    .await;
    let text_deltas: Vec<&str> = text_events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text_deltas, vec!["Hello"]);
    assert!(matches!(
        text_events.last(),
        Some(ModelEvent::Completed { .. })
    ));

    let tool_server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "minimax_snapshot_tool.sse")
    })
    .await;
    let tool_events = collect_events(&adapter(&tool_server), request_with_tools("List")).await;
    assert!(tool_events.iter().any(|event| matches!(
        event,
        ModelEvent::ToolCallStarted { call, .. }
            if call.id == ToolCallId::new("call_minimax")
    )));
    assert!(tool_events.iter().any(|event| matches!(
        event,
        ModelEvent::ToolCallCompleted { call, .. }
            if call.arguments == serde_json::json!({"path": "."})
    )));
}

/// A cumulative Chat snapshot corroborates every streamed text fragment; it
/// is not a suffix-repair opportunity and never creates a duplicate delta.
#[tokio::test]
async fn chat_text_snapshot_must_match_all_streamed_deltas() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "minimax_snapshot_text_multi_delta.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "MiniMax-M2", "hi"),
    )
    .await;
    let text_deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text_deltas, vec!["Hel", "lo"]);
    assert_eq!(text_deltas.concat(), "Hello");
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
}

/// A cumulative Chat text snapshot that conflicts with streamed content,
/// including a longer prefix, fails instead of guessing a missing suffix.
#[tokio::test]
async fn chat_conflicting_text_snapshots_fail_explicitly() {
    for fixture in [
        "minimax_snapshot_text_conflict.sse",
        "minimax_snapshot_text_prefix_conflict.sse",
    ] {
        let fixture = fixture.to_owned();
        let response_fixture = fixture.clone();
        let server = crate::common::FixtureServer::start(move |_attempt, _head| {
            sse_fixture("openai_chat", &response_fixture)
        })
        .await;
        let events = collect_events(
            &adapter(&server),
            simple_request(ModelProtocol::OpenAiChatCompletions, "MiniMax-M2", "hi"),
        )
        .await;
        assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
        assert!(matches!(
            events.last(),
            Some(ModelEvent::Failed { error })
                if error.message.contains("cumulative text snapshot")
        ));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ModelEvent::Completed { .. })),
            "fixture {fixture} must not complete after a snapshot contradiction"
        );
    }
}

/// A Chat response that contains only a cumulative message snapshot is
/// recovered as one canonical delta and then completes normally.
#[tokio::test]
async fn chat_snapshot_only_text_is_recovered_once() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "snapshot_only_text.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi"),
    )
    .await;
    let text_deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text_deltas, vec!["Hello"]);
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
}

/// Reasoning aliases normalize before cumulative comparison, and a snapshot
/// can recover reasoning when no delta was streamed.
#[tokio::test]
async fn chat_reasoning_snapshots_are_consistent() {
    for (fixture, expected) in [
        ("reasoning_snapshot_matching.sse", "Plan"),
        ("reasoning_snapshot_only.sse", "Only in snapshot."),
    ] {
        let fixture = fixture.to_owned();
        let response_fixture = fixture.clone();
        let server = crate::common::FixtureServer::start(move |_attempt, _head| {
            sse_fixture("openai_chat", &response_fixture)
        })
        .await;
        let events = collect_events(
            &adapter(&server),
            simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi"),
        )
        .await;
        let reasoning: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                ModelEvent::ReasoningDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(reasoning, vec![expected], "fixture {fixture}");
        assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    }
}

#[tokio::test]
async fn chat_conflicting_reasoning_snapshot_fails_explicitly() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "reasoning_snapshot_conflict.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi"),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
    assert!(matches!(
        events.last(),
        Some(ModelEvent::Failed { error })
            if error.message.contains("cumulative reasoning snapshot")
    ));
}

#[tokio::test]
async fn chat_refusal_snapshots_are_consistent() {
    let matching_server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "refusal_snapshot_matching.sse")
    })
    .await;
    let matching_events = collect_events(
        &adapter(&matching_server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi"),
    )
    .await;
    let refusals: Vec<&str> = matching_events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::RefusalDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(refusals, vec!["I cannot"]);
    assert!(matches!(
        matching_events.last(),
        Some(ModelEvent::Completed { .. })
    ));

    let conflict_server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "refusal_snapshot_conflict.sse")
    })
    .await;
    let conflict_events = collect_events(
        &adapter(&conflict_server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi"),
    )
    .await;
    assert_terminal_failed(&conflict_events, &ModelErrorKind::ProviderError);
    assert!(matches!(
        conflict_events.last(),
        Some(ModelEvent::Failed { error })
            if error.message.contains("cumulative refusal snapshot")
    ));
}

/// Stream shapes with output semantics that cannot fit one canonical Assistant
/// turn fail explicitly instead of being partially consumed.
#[tokio::test]
async fn unsupported_chat_stream_shapes_fail_explicitly() {
    for fixture in [
        "multiple_choices.sse",
        "legacy_function_call.sse",
        "custom_tool_call.sse",
        "moderation.sse",
        "reasoning_details.sse",
        "qwen_audio.sse",
        "glm_web_search.sse",
    ] {
        let fixture = fixture.to_owned();
        let response_fixture = fixture.clone();
        let server = crate::common::FixtureServer::start(move |_attempt, _head| {
            sse_fixture("openai_chat", &response_fixture)
        })
        .await;
        let events = collect_events(&adapter(&server), request_with_tools("hi")).await;
        assert_terminal_failed(&events, &ModelErrorKind::Unsupported);
        assert_eq!(server.attempt_count(), 1, "fixture {fixture}");
    }
}

/// Text and tool calls order by canonical block index.
#[tokio::test]
async fn text_then_tool_call_orders_by_block() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "text_then_tool.sse")
    })
    .await;
    let mut request = request_with_tools("Check and list");
    request.tools.push(model_tool("bash", "tool-bash"));
    let events = collect_events(&adapter(&server), request).await;
    assert_eq!(events[1].block_index(), 0);
    assert_eq!(events[2].block_index(), 1);
}

/// Finish reason mapping for `length` and `content_filter`.
#[tokio::test]
async fn finish_reasons_map_explicitly() {
    for (fixture, expected) in [
        ("length.sse", ModelFinishReason::Length),
        ("content_filter.sse", ModelFinishReason::ContentFilter),
        (
            "unknown_finish_reason.sse",
            ModelFinishReason::Other {
                reason: "future_reason".to_owned(),
            },
        ),
    ] {
        let server = crate::common::FixtureServer::start(move |_attempt, _head| {
            sse_fixture("openai_chat", fixture)
        })
        .await;
        let events = collect_events(
            &adapter(&server),
            simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi"),
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

/// A provider stream error terminates with Failed(ProviderError), not a fake
/// completion.
#[tokio::test]
async fn stream_error_fails() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "stream_error.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi"),
    )
    .await;
    assert_eq!(events[0], ModelEvent::Started);
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
}

/// `OpenRouter`'s typed in-band errors retain their stable classification and
/// upstream provider code instead of collapsing to a generic stream failure.
#[tokio::test]
async fn openrouter_stream_errors_map_semantically() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "openrouter_rate_limit.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "router/model", "hi"),
    )
    .await;
    let ModelEvent::Failed { error } = events.last().expect("terminal") else {
        panic!("expected Failed");
    };
    assert_eq!(error.kind, ModelErrorKind::RateLimit);
    assert_eq!(
        error.provider_code.as_deref(),
        Some("upstream_rate_limited")
    );
}

/// Provider-specific failure finish reasons are failures, not successful
/// completions carrying an opaque `Other` reason.
#[tokio::test]
async fn provider_failure_finish_reasons_fail() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "deepseek_resource_failure.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "deepseek-v4", "hi"),
    )
    .await;
    let ModelEvent::Failed { error } = events.last().expect("terminal") else {
        panic!("expected Failed");
    };
    assert_eq!(error.kind, ModelErrorKind::ProviderError);
    assert_eq!(
        error.provider_code.as_deref(),
        Some("insufficient_system_resource")
    );
}

/// A stream ending without a finish reason is a normalized failure.
#[tokio::test]
async fn interrupted_stream_fails() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "interrupted.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi"),
    )
    .await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ModelEvent::TextDelta { .. }))
    );
}

/// Malformed complete tool JSON terminates the stream with a failure; it is
/// never executed and never partially completed.
#[tokio::test]
async fn malformed_tool_arguments_fail() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "malformed_tool_arguments.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("Broken call")).await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, ModelEvent::ToolCallCompleted { .. }))
    );
}

/// A tool call without a provider invocation id fails explicitly.
#[tokio::test]
async fn tool_call_without_id_fails() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "tool_call_without_id.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("Call")).await;
    assert_terminal_failed(&events, &ModelErrorKind::ProviderError);
}

/// A model calling an unknown tool name fails with `InvalidRequest`.
#[tokio::test]
async fn unknown_tool_name_fails() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "unknown_tool_name.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("Call")).await;
    assert_terminal_failed(&events, &ModelErrorKind::InvalidRequest);
}

/// HTTP error mapping: authentication, rate limit with Retry-After, invalid
/// request, context overflow, 5xx, timeout.
#[tokio::test]
#[allow(clippy::too_many_lines)] // one table-driven error matrix per test file
async fn http_errors_normalize() {
    type ErrorCase = (
        &'static str,
        u16,
        &'static str,
        Option<Vec<(&'static str, &'static str)>>,
        ModelErrorKind,
    );
    let cases: Vec<ErrorCase> = vec![
        (
            "openai_401.json",
            401,
            "Unauthorized",
            None,
            ModelErrorKind::Authentication,
        ),
        (
            "openai_403.json",
            403,
            "Forbidden",
            None,
            ModelErrorKind::Authentication,
        ),
        (
            "openai_429.json",
            429,
            "Too Many Requests",
            Some(vec![("Retry-After", "3")]),
            ModelErrorKind::RateLimit,
        ),
        (
            "openai_400_invalid.json",
            400,
            "Bad Request",
            None,
            ModelErrorKind::InvalidRequest,
        ),
        (
            "openai_400_context.json",
            400,
            "Bad Request",
            None,
            ModelErrorKind::ContextWindowExceeded,
        ),
        (
            "openai_400_compatible_context.json",
            400,
            "Bad Request",
            None,
            ModelErrorKind::ContextWindowExceeded,
        ),
        (
            "openai_500.json",
            500,
            "Internal Server Error",
            None,
            ModelErrorKind::ProviderError,
        ),
        (
            "openai_529_raw.txt",
            529,
            "Overloaded",
            None,
            ModelErrorKind::ProviderError,
        ),
        (
            "openai_408.json",
            408,
            "Request Timeout",
            None,
            ModelErrorKind::Timeout,
        ),
    ];
    for (fixture, status, reason, headers, expected_kind) in cases {
        let fixture = fixture.to_owned();
        let fixture_for_server = fixture.clone();
        let headers: Vec<(String, String)> = headers
            .map(|headers| {
                headers
                    .into_iter()
                    .map(|(name, value)| (name.to_owned(), value.to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let server = crate::common::FixtureServer::start(move |_attempt, _head| {
            let mut reply = error_fixture(&fixture_for_server);
            reply.status = status;
            reply.reason = reason;
            reply.headers.extend(headers.clone());
            reply
        })
        .await;
        let events = collect_events(
            &adapter(&server),
            simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi"),
        )
        .await;
        assert_eq!(
            events.len(),
            2,
            "fixture {fixture}: expected [Started, Failed]: {}",
            describe_events(&events)
        );
        assert_eq!(events[0], ModelEvent::Started);
        let ModelEvent::Failed { error } = &events[1] else {
            panic!("fixture {fixture}: expected Failed");
        };
        assert_eq!(error.kind, expected_kind, "fixture {fixture}");
        if fixture == "openai_429.json" {
            assert_eq!(error.retry_after_ms, Some(3000), "Retry-After extraction");
            assert_eq!(error.provider_code.as_deref(), Some("rate_limit_exceeded"));
        }
    }
}

/// A pre-flight validation failure produces a terminal Failed without any
/// provider request and without Started.
#[tokio::test]
async fn continuation_state_is_rejected_before_network() {
    use rustx::runtime::continuation::{OpenAiResponsesContinuation, ProviderContinuationState};
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let mut request = simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi");
    request.continuation = Some(ProviderContinuationState::OpenAiResponses(
        OpenAiResponsesContinuation::Stored {
            previous_response_id: "resp_1".to_owned(),
        },
    ));
    let events = collect_events(&adapter(&server), request).await;
    assert_eq!(events.len(), 1, "terminal Failed without Started");
    assert_terminal_failed(&events, &ModelErrorKind::InvalidRequest);
    assert_eq!(server.attempt_count(), 0, "no provider request was made");
}

/// Duplicate tool names are rejected before the request is sent.
#[tokio::test]
async fn duplicate_tool_names_rejected_before_network() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let mut request = request_with_tools("hi");
    request
        .tools
        .push(model_tool("list_directory", "tool-other"));
    let events = collect_events(&adapter(&server), request).await;
    assert_eq!(events.len(), 1);
    assert_terminal_failed(&events, &ModelErrorKind::InvalidRequest);
    assert_eq!(server.attempt_count(), 0);
}

/// The serialized request carries only model-facing tool fields: runtime-only
/// semantics (`execution_mode`, `replay_policy`, `origin`, `ToolId`) never reach the
/// provider.
#[tokio::test]
async fn request_serialization_is_model_facing_only() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let events = collect_events(&adapter(&server), request_with_tools("List")).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    assert_eq!(body["model"], "gpt-test");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"][0]["text"], "List");
    assert_eq!(body["stream_options"]["include_usage"], true);
    let tool = &body["tools"][0];
    assert_eq!(tool["type"], "function");
    assert_eq!(tool["function"]["name"], "list_directory");
    assert_eq!(tool["function"]["description"], "Tool list_directory");
    assert!(tool["function"]["parameters"].is_object());
    for runtime_field in ["execution_mode", "replay_policy", "origin", "id"] {
        assert!(
            tool["function"].get(runtime_field).is_none(),
            "runtime-only field {runtime_field} must not reach the provider"
        );
        assert!(
            tool.get(runtime_field).is_none(),
            "runtime-only field {runtime_field} must not reach the provider"
        );
    }
    assert_eq!(server.attempt_count(), 1);
}

/// The stream lifecycle is deterministic: one terminal event, nothing after.
#[tokio::test]
async fn lifecycle_has_one_terminal_event() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let events = collect_events(
        &adapter(&server),
        simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi"),
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

/// `Assistant tool call → Tool result → User A → User B` stays representable as
/// ordered Chat Completions messages: assistant(tool call), tool, user A,
/// user B — no provider-side merging.
#[tokio::test]
async fn tool_then_consecutive_inbound_users_translate_in_order() {
    use rustx::message::types::{
        AssistantMessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock,
    };
    use rustx::runtime::identity::MessageId;
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let mut request = simple_request(ModelProtocol::OpenAiChatCompletions, "gpt-test", "hi");
    let user = |id: &str, text: &str| {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(rustx::message::content::TextBlock {
                text: text.to_owned(),
            })],
            source: rustx::message::types::UserSource::Human,
            kind: rustx::message::types::InboundKind::Message,
            timestamp: None,
        })
    };
    request.messages = vec![
        rustx::model::ModelInputMessage::Canonical(MessageBlock::Assistant(
            AssistantMessageBlock {
                id: MessageId::new("msg-a1"),
                content: vec![rustx::message::types::AssistantContentBlock::ToolCall(
                    rustx::tools::types::ToolCall {
                        id: rustx::runtime::identity::ToolCallId::new("call_1"),
                        tool_id: rustx::runtime::identity::ToolId::new("tool-list"),
                        name: "list_directory".to_owned(),
                        arguments: serde_json::json!({"path": "."}),
                    },
                )],
            },
        )),
        rustx::model::ModelInputMessage::Canonical(MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("msg-t1"),
            tool_call_id: rustx::runtime::identity::ToolCallId::new("call_1"),
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
        })),
        rustx::model::ModelInputMessage::Canonical(user("msg-a", "A")),
        rustx::model::ModelInputMessage::Canonical(user("msg-b", "B")),
    ];
    let events = collect_events(&adapter(&server), request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 4, "assistant, tool, user A, user B");
    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(
        messages[0]["tool_calls"][0]["function"]["name"],
        "list_directory"
    );
    assert_eq!(messages[1]["role"], "tool");
    assert_eq!(messages[1]["content"][0]["text"], "listed");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["text"], "A");
    assert_eq!(messages[3]["role"], "user");
    assert_eq!(messages[3]["content"][0]["text"], "B");
}

/// Provider adapters translate the compiled `ModelSelectable` definition
/// verbatim: the required `execution_mode` selector reaches the provider
/// inside the tool's parameter schema, and no adapter implements
/// policy-specific logic to put it there.
#[tokio::test]
async fn model_selectable_execution_mode_schema_reaches_the_provider_verbatim() {
    let server = crate::common::FixtureServer::start(|_attempt, _head| {
        sse_fixture("openai_chat", "plain_text.sse")
    })
    .await;
    let mut request = request_with_tools("List");
    request.tools = vec![model_selectable_tool("bash", "tool-bash")];
    let events = collect_events(&adapter(&server), request).await;
    assert!(matches!(events.last(), Some(ModelEvent::Completed { .. })));
    let body: serde_json::Value =
        serde_json::from_str(&server.request_body(0)).expect("request body is JSON");
    let function = &body["tools"][0]["function"];
    assert_eq!(function["name"], "bash");
    assert!(
        function["description"]
            .as_str()
            .expect("description")
            .contains("execution_mode"),
        "the compiled reminder is translated verbatim: {function}"
    );
    assert_compiled_execution_mode_schema(&function["parameters"], "command");
}
