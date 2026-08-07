//! M3 deterministic agent loop tests.
//!
//! Every test drives the loop with scripted fixture models and tools, never
//! a real provider. Tests assert behavior through the recorded
//! `RuntimeEvent` trace, the platform `AttemptOutcome`, and the committed
//! conversation state.

mod common;

use std::path::Path;

use common::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, failed_result, success_result, tool_call_events,
};
use common::replay_execution_states;
use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionRequest, AgentExecutionResult, ExecutionState,
};
use rustx::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use rustx::message::types::{
    AgentContentBlock, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::{ModelProtocol, ModelUsage, ReasoningEffort};
use rustx::runtime::continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId};
use rustx::runtime::types::{CancellationReason, RuntimeError};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::ToolCall;

fn request(attempt: &str) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id: ConversationId::new("conv-1"),
        attempt_id: AttemptId::new(attempt),
        initial_messages: vec![MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-user-1"),
            content: vec![UserContentBlock::Text(rustx::message::content::TextBlock {
                text: "What is 2+2?".to_owned(),
            })],
            source: UserSource::Human,
            kind: rustx::message::types::InboundKind::Message,
        })],
        model: "fake-model".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 512,
    }
}

async fn run(
    model: &FakeModel,
    tools: &ToolRegistry,
    cancellation: &AgentCancellation,
) -> AgentExecutionResult {
    AgentExecution::new(request("attempt-1"), model, tools, cancellation)
        .run()
        .await
}

/// The terminal events of an attempt.
fn terminal_events(events: &[RuntimeEvent]) -> Vec<&RuntimeEvent> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptTimedOut { .. }
                    | RuntimeEvent::AttemptLimitExceeded { .. }
                    | RuntimeEvent::AttemptFailed { .. }
            )
        })
        .collect()
}

/// Asserts exactly one terminal event and that it is the last event.
fn assert_single_terminal(events: &[RuntimeEvent]) -> &RuntimeEvent {
    let terminals = terminal_events(events);
    assert_eq!(terminals.len(), 1, "exactly one terminal event");
    assert_eq!(
        events.last(),
        Some(terminals[0]),
        "no runtime events may follow the terminal event"
    );
    terminals[0]
}

/// Asserts the platform outcome equals the outcome of the terminal event.
fn assert_outcome(result: &AgentExecutionResult, expected: AttemptOutcome) {
    assert_eq!(
        result.outcome, expected,
        "platform outcome mismatch: {:?}",
        result.events
    );
    let terminal = result.events.last().expect("terminal event");
    assert_eq!(
        AttemptOutcome::from_terminal_event(terminal),
        Some(expected),
        "outcome must match the terminal event"
    );
}

/// Asserts the exact recorded trace.
fn assert_trace(events: &[RuntimeEvent], expected: &[RuntimeEvent]) {
    assert_eq!(
        events,
        expected,
        "trace mismatch:\nactual:   {}\nexpected: {}",
        describe_trace(events),
        describe_trace(expected)
    );
}

fn describe_trace(events: &[RuntimeEvent]) -> String {
    events
        .iter()
        .map(|event| serde_json::to_string(event).expect("serialize event"))
        .collect::<Vec<_>>()
        .join("\n          ")
}

fn started() -> ModelEvent {
    ModelEvent::Started
}

fn text(index: u32, delta: &str) -> ModelEvent {
    ModelEvent::TextDelta {
        block_index: rustx::message::types::ContentBlockIndex::new(index),
        text: delta.to_owned(),
    }
}

fn reasoning(index: u32, delta: &str) -> ModelEvent {
    ModelEvent::ReasoningDelta {
        block_index: rustx::message::types::ContentBlockIndex::new(index),
        text: delta.to_owned(),
    }
}

fn refusal(index: u32, delta: &str) -> ModelEvent {
    ModelEvent::RefusalDelta {
        block_index: rustx::message::types::ContentBlockIndex::new(index),
        text: delta.to_owned(),
    }
}

fn done(reason: ModelFinishReason) -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: reason,
        usage: None,
    }
}

fn fail(kind: rustx::model::ModelErrorKind, message: &str) -> ModelEvent {
    ModelEvent::Failed {
        error: rustx::model::ModelError {
            kind,
            message: message.to_owned(),
            retry_after_ms: None,
            provider_code: None,
        },
    }
}

fn agent_message_id(turn: u32) -> MessageId {
    MessageId::new(format!("attempt-1-agent-{turn}"))
}

// ---------------------------------------------------------------------------
// Basic execution
// ---------------------------------------------------------------------------

/// A text-only turn completes with the exact canonical trace.
#[tokio::test]
async fn text_execution_completes_with_exact_trace() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "hello")),
        FakeStep::Emit(text(0, " world")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    let expected = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-1"),
        },
        RuntimeEvent::TurnStarted,
        RuntimeEvent::ModelRequestStarted {
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::AgentMessageStarted {
            message_id: agent_message_id(1),
        },
        RuntimeEvent::AgentTextDelta {
            message_id: agent_message_id(1),
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            delta: "hello".to_owned(),
        },
        RuntimeEvent::AgentTextDelta {
            message_id: agent_message_id(1),
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            delta: " world".to_owned(),
        },
        RuntimeEvent::ModelRequestCompleted {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
        RuntimeEvent::TurnCompleted,
        RuntimeEvent::AttemptCompleted {
            attempt_id: AttemptId::new("attempt-1"),
            finish_reason: ModelFinishReason::Stop,
        },
    ];
    assert_trace(&result.events, &expected);
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    let MessageBlock::Agent(agent) = result.messages.last().expect("agent message") else {
        panic!("final message must be an agent message");
    };
    let AgentContentBlock::Text(block) = &agent.content[0] else {
        panic!("final message must contain text");
    };
    assert_eq!(block.text, "hello world");
}

/// Several deltas across blocks assemble in stream order.
#[tokio::test]
async fn several_deltas_assemble_in_stream_order() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "a")),
        FakeStep::Emit(text(1, "b")),
        FakeStep::Emit(text(0, "c")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    let deltas: Vec<&str> = result
        .events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::AgentTextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["a", "b", "c"], "delta order preserved");
    let MessageBlock::Agent(agent) = result.messages.last().expect("agent message") else {
        panic!("final message must be an agent message");
    };
    let texts: Vec<&str> = agent
        .content
        .iter()
        .filter_map(|block| match block {
            AgentContentBlock::Text(block) => Some(block.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["ac", "b"], "blocks assemble in index order");
}

/// A model failure before any content fails the attempt without committing
/// a message.
#[tokio::test]
async fn model_failure_before_content_fails_attempt() {
    let model = FakeModel::new(vec![vec![FakeStep::Emit(fail(
        rustx::model::ModelErrorKind::Timeout,
        "timed out",
    ))]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    let error = rustx::model::ModelError {
        kind: rustx::model::ModelErrorKind::Timeout,
        message: "timed out".to_owned(),
        retry_after_ms: None,
        provider_code: None,
    };
    let expected = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-1"),
        },
        RuntimeEvent::TurnStarted,
        RuntimeEvent::ModelRequestStarted {
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::ModelRequestFailed {
            error: error.clone(),
        },
        RuntimeEvent::AttemptFailed {
            attempt_id: AttemptId::new("attempt-1"),
            error: AttemptFailure::Model { error },
        },
    ];
    assert_trace(&result.events, &expected);
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model {
                error: rustx::model::ModelError {
                    kind: rustx::model::ModelErrorKind::Timeout,
                    message: "timed out".to_owned(),
                    retry_after_ms: None,
                    provider_code: None,
                },
            },
        },
    );
    assert_eq!(
        result.messages.len(),
        1,
        "no agent message is committed from a failed turn"
    );
}

/// A model failure after partial content keeps the streamed deltas in the
/// trace but commits nothing.
#[tokio::test]
async fn model_failure_after_partial_content_commits_nothing() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "partial")),
        FakeStep::Emit(fail(
            rustx::model::ModelErrorKind::ProviderError,
            "stream broke",
        )),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::AgentTextDelta { .. })),
        "streamed deltas remain in the trace"
    );
    assert_single_terminal(&result.events);
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model { .. }
        }
    ));
    assert_eq!(result.messages.len(), 1, "nothing committed");
}

/// Every scenario ends with exactly one terminal event.
#[tokio::test]
async fn exactly_one_terminal_event_across_scenarios() {
    let scenarios: Vec<Vec<Vec<FakeStep>>> = vec![
        vec![vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ]],
        vec![vec![FakeStep::Emit(fail(
            rustx::model::ModelErrorKind::RateLimit,
            "nope",
        ))]],
        vec![vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "hi")),
            FakeStep::Emit(fail(rustx::model::ModelErrorKind::Authentication, "nope")),
        ]],
    ];
    for script in scenarios {
        let model = FakeModel::new(script);
        let tools = ToolRegistry::new();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let result = run(&model, &tools, &cancellation).await;
        assert_single_terminal(&result.events);
    }
}

// ---------------------------------------------------------------------------
// Ordering
// ---------------------------------------------------------------------------

/// No runtime event follows the completed terminal event (asserted by
/// `assert_single_terminal` over a full trace).
#[tokio::test]
async fn no_events_after_completed_terminal() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "done")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;
    assert_single_terminal(&result.events);
    assert_eq!(result.events.len(), 8, "exact event count of a text turn");
}

/// A tool turn's calls execute in block order.
#[tokio::test]
async fn tool_calls_execute_in_block_order() {
    let calls = [
        ScriptedCall {
            id: "call-a",
            tool_id: "tool-alpha",
            name: "alpha",
            arguments: serde_json::json!({"n": 1}),
        },
        ScriptedCall {
            id: "call-b",
            tool_id: "tool-beta",
            name: "beta",
            arguments: serde_json::json!({"n": 2}),
        },
        ScriptedCall {
            id: "call-c",
            tool_id: "tool-alpha",
            name: "alpha",
            arguments: serde_json::json!({"n": 3}),
        },
    ];
    let mut turn_one = vec![FakeStep::Emit(started())];
    for (index, call) in calls.iter().enumerate() {
        for event in tool_call_events(u32::try_from(index).expect("block index"), call) {
            turn_one.push(FakeStep::Emit(event));
        }
    }
    turn_one.push(FakeStep::Emit(done(ModelFinishReason::ToolCalls)));
    let model = FakeModel::new(vec![
        turn_one,
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "done")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);

    let mut tools = ToolRegistry::new();
    tools.insert(FakeTool::new(
        common::tool("alpha", "tool-alpha"),
        success_result("alpha ok"),
    ));
    tools.insert(FakeTool::new(
        common::tool("beta", "tool-beta"),
        success_result("beta ok"),
    ));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    let executed: Vec<(String, String)> = result
        .events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                tool_id,
                ..
            } => Some((tool_call_id.to_string(), tool_id.to_string())),
            _ => None,
        })
        .collect();
    assert_eq!(
        executed,
        vec![
            ("call-a".to_owned(), "tool-alpha".to_owned()),
            ("call-b".to_owned(), "tool-beta".to_owned()),
            ("call-c".to_owned(), "tool-alpha".to_owned()),
        ],
        "tool calls execute in block order"
    );
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

/// The continuation request starts only after every tool of the turn
/// completed.
#[tokio::test]
async fn continuation_starts_after_tool_completion() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({"path": "."}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "done")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.insert(FakeTool::new(
        common::tool("alpha", "tool-alpha"),
        success_result("ok"),
    ));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    let last_completion = result
        .events
        .iter()
        .rposition(|event| matches!(event, RuntimeEvent::ToolExecutionCompleted { .. }))
        .expect("tool completion recorded");
    let continuation_start = result
        .events
        .iter()
        .skip(last_completion + 1)
        .position(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
        .map(|offset| offset + last_completion + 1)
        .expect("continuation starts after completion");
    assert_eq!(model.requests().len(), 2, "exactly two model invocations");
    assert!(continuation_start > last_completion);
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

// ---------------------------------------------------------------------------
// Tool lifecycle
// ---------------------------------------------------------------------------

/// The exact expected trace of one tool call followed by a continuation.
fn expected_single_tool_trace() -> Vec<RuntimeEvent> {
    vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-1"),
        },
        RuntimeEvent::TurnStarted,
        RuntimeEvent::ModelRequestStarted {
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::AgentMessageStarted {
            message_id: agent_message_id(1),
        },
        RuntimeEvent::ToolCallStarted {
            message_id: agent_message_id(1),
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            call: rustx::tools::types::ToolCallStart {
                id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-alpha"),
                name: "alpha".to_owned(),
            },
        },
        RuntimeEvent::ToolCallArgumentsDelta {
            message_id: agent_message_id(1),
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            call_id: ToolCallId::new("call-1"),
            arguments_delta: r#"{"path":"."}"#.to_owned(),
        },
        RuntimeEvent::ToolCallCompleted {
            message_id: agent_message_id(1),
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            call: ToolCall {
                id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-alpha"),
                name: "alpha".to_owned(),
                arguments: serde_json::json!({"path": "."}),
            },
        },
        RuntimeEvent::ModelRequestCompleted {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        },
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-alpha"),
        },
        RuntimeEvent::ToolExecutionCompleted {
            tool_call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-alpha"),
            result: success_result("listed"),
        },
        RuntimeEvent::TurnCompleted,
        RuntimeEvent::TurnStarted,
        RuntimeEvent::ModelRequestStarted {
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::AgentMessageStarted {
            message_id: agent_message_id(2),
        },
        RuntimeEvent::AgentTextDelta {
            message_id: agent_message_id(2),
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            delta: "done".to_owned(),
        },
        RuntimeEvent::ModelRequestCompleted {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
        RuntimeEvent::TurnCompleted,
        RuntimeEvent::AttemptCompleted {
            attempt_id: AttemptId::new("attempt-1"),
            finish_reason: ModelFinishReason::Stop,
        },
    ]
}

/// One tool call, its result, and the model continuation complete the
/// attempt.
#[tokio::test]
async fn single_tool_call_then_continuation() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({"path": "."}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "done")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tool = FakeTool::new(
        common::tool("alpha", "tool-alpha"),
        success_result("listed"),
    );
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    assert_trace(&result.events, &expected_single_tool_trace());
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert_eq!(
        result.messages.len(),
        4,
        "input + two agent messages + tool message"
    );
    assert!(matches!(result.messages[1], MessageBlock::Agent(_)));
    assert!(matches!(result.messages[2], MessageBlock::Tool(_)));
    assert!(matches!(result.messages[3], MessageBlock::Agent(_)));
}

/// The tool receives the exact canonical arguments.
#[tokio::test]
async fn tool_receives_exact_canonical_arguments() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({"path": ".", "options": {"recursive": true}}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tool = FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("ok"));
    let mut calls = tool.calls();
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    let received = calls.borrow_and_update().clone();
    assert_eq!(received.len(), 1, "the tool was called exactly once");
    assert_eq!(
        received[0].arguments,
        serde_json::json!({"path": ".", "options": {"recursive": true}}),
        "the tool receives the exact canonical arguments"
    );
    assert_eq!(received[0].name, "alpha");
    assert_eq!(received[0].tool_id, ToolId::new("tool-alpha"));
    assert_single_terminal(&result.events);
}

/// The tool result is passed back to the model without fabricated data.
#[tokio::test]
async fn tool_result_passed_back_verbatim() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let expected_result = success_result("exact output");
    let tool = FakeTool::new(common::tool("alpha", "tool-alpha"), expected_result.clone());
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let _ = run(&model, &tools, &cancellation).await;

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let MessageBlock::Tool(tool_message) = &requests[1].messages[2] else {
        panic!("second request must contain the tool message");
    };
    assert_eq!(
        tool_message.result, expected_result,
        "result passed back verbatim"
    );
    assert_eq!(tool_message.tool_call_id, ToolCallId::new("call-1"));
}

/// An unknown tool fails the attempt explicitly with a typed runtime error.
#[tokio::test]
async fn unknown_tool_fails_deterministically() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-missing",
        name: "missing",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
        FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
        FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
        FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    assert_single_terminal(&result.events);
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ToolExecutionFailed { .. })),
        "no tool result exists, so the execution failure is recorded"
    );
    assert_outcome(
        &result,
        AttemptOutcome::Failed {
            error: AttemptFailure::Runtime {
                error: RuntimeError::UnknownTool {
                    name: "missing".to_owned(),
                },
            },
        },
    );
    assert_eq!(
        model.requests().len(),
        1,
        "no continuation is attempted after an unknown tool"
    );
}

/// A failing tool produces a normalized failed result that is passed back
/// to the model; the attempt continues.
#[tokio::test]
async fn tool_execution_failure_is_passed_back_and_continues() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "recovered")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let failed = failed_result("boom");
    let tool = FakeTool::new(common::tool("alpha", "tool-alpha"), failed.clone());
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let MessageBlock::Tool(tool_message) = &requests[1].messages[2] else {
        panic!("continuation must carry the failed tool result");
    };
    assert_eq!(
        tool_message.result, failed,
        "failed result passed back verbatim"
    );
    assert!(
        matches!(
            tool_message.result.status,
            rustx::tools::types::ToolExecutionStatus::Failed { .. }
        ),
        "failure status preserved, never flattened into text"
    );
}

/// Multiple ordered tool calls in one turn execute and continue once.
#[tokio::test]
async fn multiple_ordered_tool_calls_continue_once() {
    let first = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({"x": 1}),
    };
    let second = ScriptedCall {
        id: "call-2",
        tool_id: "tool-beta",
        name: "beta",
        arguments: serde_json::json!({"y": 2}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &first)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &first)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &first)[2].clone()),
            FakeStep::Emit(tool_call_events(1, &second)[0].clone()),
            FakeStep::Emit(tool_call_events(1, &second)[1].clone()),
            FakeStep::Emit(tool_call_events(1, &second)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "done")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.insert(FakeTool::new(
        common::tool("alpha", "tool-alpha"),
        success_result("a"),
    ));
    tools.insert(FakeTool::new(
        common::tool("beta", "tool-beta"),
        success_result("b"),
    ));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    let requests = model.requests();
    assert_eq!(requests.len(), 2, "one continuation after the batch");
    let tool_messages: Vec<&MessageBlock> = requests[1]
        .messages
        .iter()
        .filter(|block| matches!(block, MessageBlock::Tool(_)))
        .collect();
    assert_eq!(tool_messages.len(), 2, "both results in the continuation");
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

/// Cancellation before the attempt starts settles as cancelled.
#[tokio::test]
async fn cancellation_before_start_settles_cancelled() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    cancellation.cancel();
    let result = run(&model, &tools, &cancellation).await;

    assert_eq!(
        model.requests().len(),
        0,
        "no model request before cancellation"
    );
    let expected = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-1"),
        },
        RuntimeEvent::AttemptCancelled {
            attempt_id: AttemptId::new("attempt-1"),
            reason: CancellationReason::UserRequested,
        },
    ];
    assert_trace(&result.events, &expected);
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested,
        },
    );
}

/// Cancellation during model generation after partial text settles
/// cancelled; the streamed deltas remain, nothing after the terminal.
#[tokio::test]
async fn cancellation_during_generation_after_partial_text() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "partial")),
        FakeStep::ParkUntilCancelled,
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut emitted = model.emitted();
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        emitted
            .wait_for(|count| *count >= 2)
            .await
            .expect("emitted");
        controller_cancellation.cancel();
    });
    let result = run(&model, &tools, &cancellation).await;
    controller.await.expect("controller task");

    assert_eq!(
        model.requests().len(),
        1,
        "no continuation after cancellation"
    );
    let expected = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-1"),
        },
        RuntimeEvent::TurnStarted,
        RuntimeEvent::ModelRequestStarted {
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::AgentMessageStarted {
            message_id: agent_message_id(1),
        },
        RuntimeEvent::AgentTextDelta {
            message_id: agent_message_id(1),
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            delta: "partial".to_owned(),
        },
        RuntimeEvent::AttemptCancelled {
            attempt_id: AttemptId::new("attempt-1"),
            reason: CancellationReason::UserRequested,
        },
    ];
    assert_trace(&result.events, &expected);
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested,
        },
    );
    assert_eq!(
        result.messages.len(),
        1,
        "no message committed on cancellation"
    );
}

/// Cancellation while a tool is executing records the tool completion fact
/// but never continues the model.
#[tokio::test]
async fn cancellation_while_waiting_for_tool() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
        FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
        FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
        FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
    ]]);
    let (tool, release) =
        FakeTool::parking(common::tool("alpha", "tool-alpha"), success_result("late"));
    let mut tool_started = tool.started();
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        tool_started
            .wait_for(|running| *running)
            .await
            .expect("tool started");
        controller_cancellation.cancel();
        release.notify_one();
    });
    let result = run(&model, &tools, &cancellation).await;
    controller.await.expect("controller task");

    assert_eq!(
        model.requests().len(),
        1,
        "no continuation starts after cancellation"
    );
    assert_single_terminal(&result.events);
    assert!(matches!(
        result.events.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ToolExecutionCompleted { .. })),
        "the tool completion fact is recorded"
    );
    assert_outcome(
        &result,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested,
        },
    );
}

/// Cancellation observed at the tool-batch boundary prevents the
/// continuation model invocation.
#[tokio::test]
async fn cancellation_before_continuation_invokes_no_model() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
        FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
        FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
        FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
    ]]);
    let (tool, release) =
        FakeTool::parking(common::tool("alpha", "tool-alpha"), success_result("late"));
    let mut tool_started = tool.started();
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        tool_started
            .wait_for(|running| *running)
            .await
            .expect("tool started");
        controller_cancellation.cancel();
        release.notify_one();
    });
    let result = run(&model, &tools, &cancellation).await;
    controller.await.expect("controller task");

    assert_eq!(
        model.requests().len(),
        1,
        "the continuation request is never sent"
    );
    assert_single_terminal(&result.events);
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
}

/// Cancellation during continuation generation settles cancelled after the
/// second model request started.
#[tokio::test]
async fn cancellation_during_continuation_generation() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "continuation ")),
            FakeStep::ParkUntilCancelled,
        ],
    ]);
    let tool = FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("ok"));
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut emitted = model.emitted();
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        emitted
            .wait_for(|count| *count >= 7)
            .await
            .expect("continuation deltas");
        controller_cancellation.cancel();
    });
    let result = run(&model, &tools, &cancellation).await;
    controller.await.expect("controller task");

    assert_eq!(
        model.requests().len(),
        2,
        "the continuation request was sent"
    );
    assert_single_terminal(&result.events);
    assert!(matches!(
        result.events.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::AgentTextDelta { .. })),
        "continuation deltas before cancellation remain in the trace"
    );
    assert_outcome(
        &result,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested,
        },
    );
}

/// Cancellation never produces a completed outcome.
#[tokio::test]
async fn cancellation_never_results_in_completed() {
    for reason in [
        CancellationReason::UserRequested,
        CancellationReason::RuntimeShutdown,
        CancellationReason::ParentCancelled,
    ] {
        let model = FakeModel::new(vec![vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "hi")),
            FakeStep::ParkUntilCancelled,
        ]]);
        let tools = ToolRegistry::new();
        let cancellation = AgentCancellation::new(reason);
        let mut emitted = model.emitted();
        let controller_cancellation = cancellation.clone();
        let controller = tokio::spawn(async move {
            emitted
                .wait_for(|count| *count >= 2)
                .await
                .expect("emitted");
            controller_cancellation.cancel();
        });
        let result = run(&model, &tools, &cancellation).await;
        controller.await.expect("controller task");
        assert_single_terminal(&result.events);
        assert!(
            !matches!(result.outcome, AttemptOutcome::Completed { .. }),
            "cancellation must never complete"
        );
        assert_eq!(
            result.outcome,
            AttemptOutcome::Cancelled { reason },
            "the reported reason is the attempt's reason"
        );
    }
}

// ---------------------------------------------------------------------------
// Continuation
// ---------------------------------------------------------------------------

/// The provider continuation state propagates losslessly into the next
/// request and onto the committed reasoning block.
#[tokio::test]
async fn continuation_state_propagates_losslessly() {
    let state = ProviderContinuationState::Anthropic(AnthropicContinuation {
        opaque: serde_json::json!({"signature": "sig-1", "opaque": [1, 2, 3]}),
    });
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(reasoning(0, "Thinking.")),
            FakeStep::Emit(ModelEvent::ContinuationState {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                state: state.clone(),
            }),
            FakeStep::Emit(tool_call_events(1, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(1, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(1, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tool = FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("ok"));
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].continuation,
        Some(state.clone()),
        "the boundary continuation state is propagated losslessly"
    );
    let MessageBlock::Agent(agent) = &result.messages[1] else {
        panic!("first committed message must be the agent message");
    };
    let AgentContentBlock::Reasoning(reasoning) = &agent.content[0] else {
        panic!("first block must be a reasoning block");
    };
    assert_eq!(
        reasoning.provider_state,
        Some(state),
        "state attached to its block"
    );
}

/// Opaque continuation state passes through without inspection.
#[tokio::test]
async fn opaque_continuation_is_never_inspected() {
    let state =
        ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {
            items: vec![serde_json::json!({
                "type": "reasoning",
                "id": "rs_1",
                "summary": [],
                "encrypted_content": "opaque-bytes"
            })],
        });
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(ModelEvent::ContinuationState {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                state: state.clone(),
            }),
            FakeStep::Emit(tool_call_events(1, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(1, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(1, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tool = FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("ok"));
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;
    assert_eq!(
        model.requests()[1].continuation,
        Some(state),
        "opaque items pass through byte-for-byte"
    );
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

/// A model that requires continuation state but receives none fails
/// explicitly; the loop never fabricates the missing state.
#[tokio::test]
async fn missing_required_continuation_fails_explicitly() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![FakeStep::Emit(fail(
            rustx::model::ModelErrorKind::Unsupported,
            "required provider continuation state is missing",
        ))],
    ]);
    let tool = FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("ok"));
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    assert_eq!(
        model.requests()[1].continuation,
        None,
        "the loop passes exactly what the stream reported: nothing"
    );
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model {
                error: rustx::model::ModelError {
                    kind: rustx::model::ModelErrorKind::Unsupported,
                    message: "required provider continuation state is missing".to_owned(),
                    retry_after_ms: None,
                    provider_code: None,
                },
            },
        },
    );
}

/// Reasoning deltas without continuation state stay unadorned; no reasoning
/// state is ever fabricated.
#[tokio::test]
async fn no_reasoning_state_is_fabricated() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(reasoning(0, "Visible reasoning.")),
            FakeStep::Emit(tool_call_events(1, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(1, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(1, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tool = FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("ok"));
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    assert_eq!(
        model.requests()[1].continuation,
        None,
        "no continuation state exists to propagate"
    );
    let MessageBlock::Agent(agent) = &result.messages[1] else {
        panic!("first committed message must be the agent message");
    };
    let AgentContentBlock::Reasoning(reasoning) = &agent.content[0] else {
        panic!("first block must be a reasoning block");
    };
    assert_eq!(reasoning.text.as_deref(), Some("Visible reasoning."));
    assert_eq!(reasoning.provider_state, None, "no state fabricated");
}

/// An unsupported capability failure (the fallback-block boundary) stays a
/// terminal model failure and never becomes text.
#[tokio::test]
async fn unsupported_capability_stays_terminal_failure() {
    let model = FakeModel::new(vec![vec![FakeStep::Emit(fail(
        rustx::model::ModelErrorKind::Unsupported,
        "server-side fallback blocks are not supported",
    ))]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model {
                error: rustx::model::ModelError {
                    kind: rustx::model::ModelErrorKind::Unsupported,
                    message: "server-side fallback blocks are not supported".to_owned(),
                    retry_after_ms: None,
                    provider_code: None,
                },
            },
        },
    );
    assert_eq!(
        result.messages.len(),
        1,
        "the failure never becomes a committed message"
    );
}

/// Usage reported by the terminal model event reaches the trace.
#[tokio::test]
async fn usage_is_reported_on_model_request_completed() {
    let usage = ModelUsage {
        input_tokens: 12,
        output_tokens: 7,
        total_tokens: 19,
        details: Some(rustx::model::types::UsageDetails {
            reasoning_tokens: Some(3),
            cached_input_tokens: None,
        }),
    };
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(ModelEvent::UsageUpdate {
            usage: usage.clone(),
        }),
        FakeStep::Emit(text(0, "done")),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: Some(usage.clone()),
        }),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    let reported = result
        .events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::ModelRequestCompleted { usage, .. } => usage.clone(),
            _ => None,
        })
        .expect("model request completion event");
    assert_eq!(reported, usage);
    assert_single_terminal(&result.events);
}

/// A refusal is a successful stop: the committed message holds only the
/// refusal block and provisional content is rolled back.
#[tokio::test]
async fn refusal_semantics_preserved() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "I would normally answer, but")),
        FakeStep::Emit(refusal(1, "I cannot comply.")),
        FakeStep::Emit(done(ModelFinishReason::Refusal)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Refusal,
        },
    );
    let MessageBlock::Agent(agent) = result.messages.last().expect("agent message") else {
        panic!("final message must be an agent message");
    };
    assert_eq!(agent.content.len(), 1, "provisional content rolled back");
    assert!(
        matches!(
            &agent.content[0],
            AgentContentBlock::Refusal(block) if block.text == "I cannot comply."
        ),
        "the refusal block is preserved, never flattened into text"
    );
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// A text-only trace reconstructs `Idle → RunningModel → Completed`.
#[tokio::test]
async fn replay_text_execution() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "hello")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;
    assert_eq!(
        replay_execution_states(&result.events).expect("valid trace"),
        vec![
            ExecutionState::Idle,
            ExecutionState::RunningModel,
            ExecutionState::Completed,
        ]
    );
}

/// A tool trace reconstructs
/// `Idle → RunningModel → WaitingForTool → RunningModel → Completed`.
#[tokio::test]
async fn replay_tool_execution() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tool = FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("ok"));
    let mut tools = ToolRegistry::new();
    tools.insert(tool);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;
    assert_eq!(
        replay_execution_states(&result.events).expect("valid trace"),
        vec![
            ExecutionState::Idle,
            ExecutionState::RunningModel,
            ExecutionState::WaitingForTool,
            ExecutionState::RunningModel,
            ExecutionState::Completed,
        ]
    );
}

/// A failed trace reconstructs `Idle → ... → Failed`.
#[tokio::test]
async fn replay_failed_execution() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "partial")),
        FakeStep::Emit(fail(rustx::model::ModelErrorKind::RateLimit, "nope")),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;
    assert_eq!(
        replay_execution_states(&result.events).expect("valid trace"),
        vec![
            ExecutionState::Idle,
            ExecutionState::RunningModel,
            ExecutionState::Failed,
        ]
    );
}

/// A cancelled trace reconstructs `Idle → ... → Failed`.
#[tokio::test]
async fn replay_cancelled_execution() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "partial")),
        FakeStep::ParkUntilCancelled,
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut emitted = model.emitted();
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        emitted
            .wait_for(|count| *count >= 2)
            .await
            .expect("emitted");
        controller_cancellation.cancel();
    });
    let result = run(&model, &tools, &cancellation).await;
    controller.await.expect("controller task");
    assert_eq!(
        replay_execution_states(&result.events).expect("valid trace"),
        vec![
            ExecutionState::Idle,
            ExecutionState::RunningModel,
            ExecutionState::Failed,
        ]
    );
}

/// Identical fixture inputs produce identical event traces and results.
#[tokio::test]
async fn identical_inputs_produce_identical_traces() {
    let script = || {
        vec![vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "hello ")),
            FakeStep::Emit(text(0, "world")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ]]
    };
    let first = FakeModel::new(script());
    let second = FakeModel::new(script());
    let tools_first = ToolRegistry::new();
    let tools_second = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result_first = run(&first, &tools_first, &cancellation).await;
    let result_second = run(&second, &tools_second, &cancellation).await;
    assert_eq!(result_first.events, result_second.events);
    assert_eq!(result_first.messages, result_second.messages);
    assert_eq!(result_first.outcome, result_second.outcome);
}

/// The replay fold rejects invalid event sequences explicitly.
#[test]
fn replay_rejects_invalid_sequences() {
    let attempt_id = AttemptId::new("attempt-1");
    let second_start = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt_id.clone(),
        },
        RuntimeEvent::TurnStarted,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt_id.clone(),
        },
    ];
    assert!(
        replay_execution_states(&second_start).is_err(),
        "attempt started twice"
    );

    let event_after_terminal = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt_id.clone(),
        },
        RuntimeEvent::AttemptCompleted {
            attempt_id: attempt_id.clone(),
            finish_reason: ModelFinishReason::Stop,
        },
        RuntimeEvent::TurnStarted,
    ];
    assert!(
        replay_execution_states(&event_after_terminal).is_err(),
        "events after the terminal event are rejected"
    );

    let completed_then_failed = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt_id.clone(),
        },
        RuntimeEvent::AttemptCompleted {
            attempt_id: attempt_id.clone(),
            finish_reason: ModelFinishReason::Stop,
        },
        RuntimeEvent::AttemptFailed {
            attempt_id: attempt_id.clone(),
            error: AttemptFailure::Runtime {
                error: RuntimeError::InvalidState {
                    message: "late".to_owned(),
                },
            },
        },
    ];
    assert!(
        replay_execution_states(&completed_then_failed).is_err(),
        "a second terminal outcome is rejected"
    );

    let completion_from_tool_phase = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt_id.clone(),
        },
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-a"),
        },
        RuntimeEvent::AttemptCompleted {
            attempt_id: attempt_id.clone(),
            finish_reason: ModelFinishReason::Stop,
        },
    ];
    assert!(
        replay_execution_states(&completion_from_tool_phase).is_err(),
        "completion outside a running-model phase is rejected"
    );
}

// ---------------------------------------------------------------------------
// Contract boundaries
// ---------------------------------------------------------------------------

/// M3 modules contain no provider-specific branching: the agent kernel
/// source never names a provider protocol.
#[test]
fn agent_modules_contain_no_provider_branching() {
    let agent_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("agent");
    let forbidden = [
        "OpenAI",
        "OpenAi",
        "Anthropic",
        "Responses",
        "ChatCompletions",
        "Messages",
    ];
    let mut files = Vec::new();
    collect_rs_files(&agent_dir, &mut files);
    assert!(!files.is_empty(), "agent sources must exist");
    for file in files {
        let source = std::fs::read_to_string(&file).expect("read agent source");
        for token in forbidden {
            assert!(
                !source.contains(token),
                "{token} must not appear in the agent kernel: {}",
                file.display()
            );
        }
    }
}

fn collect_rs_files(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read agent directory") {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// Malformed canonical model streams are rejected with an explicit
/// contract-violation terminal failure.
#[tokio::test]
async fn malformed_streams_are_rejected() {
    let usage = ModelUsage {
        input_tokens: 1,
        output_tokens: 1,
        total_tokens: 2,
        details: None,
    };
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let cases: Vec<Vec<FakeStep>> = vec![
        // A non-terminal event after the terminal event.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "hi")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
            FakeStep::Emit(text(0, "late")),
        ],
        // A second terminal event.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
            FakeStep::Emit(fail(rustx::model::ModelErrorKind::Timeout, "late")),
        ],
        // A terminal completion before Started.
        vec![FakeStep::Emit(done(ModelFinishReason::Stop))],
        // Content before Started.
        vec![
            FakeStep::Emit(text(0, "early")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        // A tool-call delta referencing an unknown call.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                call_id: ToolCallId::new("ghost"),
                arguments_delta: "{}".to_owned(),
            }),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        // A duplicate tool-call completion.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        // A stream that ends without any terminal event.
        vec![FakeStep::Emit(started()), FakeStep::Emit(text(0, "hi"))],
        // A skipped block index.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(2, "gap")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        // Usage update after the terminal event.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
            FakeStep::Emit(ModelEvent::UsageUpdate {
                usage: usage.clone(),
            }),
        ],
    ];
    for script in cases {
        let model = FakeModel::new(vec![script]);
        let tools = ToolRegistry::new();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let result = run(&model, &tools, &cancellation).await;
        assert_single_terminal(&result.events);
        assert!(
            matches!(
                result.outcome,
                AttemptOutcome::Failed {
                    error: AttemptFailure::Runtime {
                        error: RuntimeError::ContractViolation { .. }
                    }
                }
            ),
            "malformed streams fail with ContractViolation: {:?}",
            result.events
        );
    }
}

/// An unfinished tool call at the terminal event is a contract violation.
#[tokio::test]
async fn unfinished_tool_call_at_terminal_is_rejected() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
        FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, &tools, &cancellation).await;
    assert_single_terminal(&result.events);
    assert!(
        matches!(
            result.outcome,
            AttemptOutcome::Failed {
                error: AttemptFailure::Runtime {
                    error: RuntimeError::ContractViolation { .. }
                }
            }
        ),
        "an uncompleted tool call cannot be executed or fabricated"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "no continuation after the contract violation"
    );
}
