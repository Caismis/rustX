//! M3 deterministic agent loop tests.
//!
//! Every test drives the loop with scripted fixture models and tools, never
//! a real provider. Tests assert behavior through the recorded
//! `RuntimeEvent` trace, the platform `AttemptOutcome`, and the committed
//! conversation state. Issue #22 adds the conversation inbound mailbox
//! integration: safe-boundary finite drains, Stop-with-pending-inbound
//! continuation, cancellation/failure ownership, and deterministic
//! interleavings driven by explicit synchronization (never wall-clock
//! timing).

mod common;

use std::path::Path;

use common::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, failed_result, model_release, success_result,
    tool_call_events,
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
use rustx::runtime::inbound::{ConversationInboundMailbox, MailboxError};
use rustx::runtime::types::{CancellationReason, RuntimeError};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{ToolCall, ToolExecutionStatus};

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
            // A historical (non-fresh) inbound message: the M3 loop
            // invariants are exercised without Agent Status, expressed as an
            // explicit pure-continuation trigger.
            timestamp: None,
        })],
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: "fake-model".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 512,
    }
}

/// A deterministic context runtime with a window far larger than any
/// scripted request: the mandatory M4 path is active, but no compaction or
/// summary activity can ever trigger in these loop-contract tests.
fn runtime() -> rustx::context::ContextRuntime<'static> {
    use rustx::context::{
        ContextConfig, ContextEngine, ContextRuntime, DefaultTokenEstimator,
        InMemoryCheckpointStore,
    };
    let estimator: std::sync::Arc<dyn rustx::context::TokenEstimator> =
        std::sync::Arc::new(DefaultTokenEstimator);
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 10_000_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator,
    )
    .expect("valid context configuration");
    ContextRuntime::new(
        engine,
        std::sync::Arc::new(common::context::FakeContextSummarizer::new(Vec::new())),
        std::sync::Arc::new(InMemoryCheckpointStore::new()),
    )
}

async fn run(
    model: &FakeModel,
    tools: ToolRegistry,
    cancellation: &AgentCancellation,
) -> AgentExecutionResult {
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    AgentExecution::new(
        request("attempt-1"),
        model,
        capability.lease(),
        cancellation,
        runtime(),
        &tool_runtime,
    )
    .expect("conversation identity matches the tool runtime")
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
    let result = run(&model, tools, &cancellation).await;

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
    let result = run(&model, tools, &cancellation).await;

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
    let result = run(&model, tools, &cancellation).await;

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
    let result = run(&model, tools, &cancellation).await;

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
        let result = run(&model, tools, &cancellation).await;
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
    let result = run(&model, tools, &cancellation).await;
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
    FakeTool::new(
        common::tool("alpha", "tool-alpha"),
        success_result("alpha ok"),
    )
    .register(&mut tools);
    FakeTool::new(common::tool("beta", "tool-beta"), success_result("beta ok"))
        .register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;

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
    FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("ok")).register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;

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
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;

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
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;

    let received = calls.borrow_and_update().clone();
    assert_eq!(received.len(), 1, "the tool was called exactly once");
    assert_eq!(
        received[0].arguments,
        serde_json::json!({"path": ".", "options": {"recursive": true}}),
        "the tool receives the exact canonical arguments"
    );
    assert_eq!(received[0].tool_name, "alpha");
    assert_eq!(received[0].tool_id, ToolId::new("tool-alpha"));
    assert_eq!(received[0].call_id, ToolCallId::new("call-1"));
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
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let _ = run(&model, tools, &cancellation).await;

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

/// The exact trace of an attempt that requests an unknown tool.
fn expected_unknown_tool_trace() -> Vec<RuntimeEvent> {
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
                tool_id: ToolId::new("tool-missing"),
                name: "missing".to_owned(),
            },
        },
        RuntimeEvent::ToolCallArgumentsDelta {
            message_id: agent_message_id(1),
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            call_id: ToolCallId::new("call-1"),
            arguments_delta: "{}".to_owned(),
        },
        RuntimeEvent::ToolCallCompleted {
            message_id: agent_message_id(1),
            block_index: rustx::message::types::ContentBlockIndex::new(0),
            call: ToolCall {
                id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-missing"),
                name: "missing".to_owned(),
                arguments: serde_json::json!({}),
            },
        },
        RuntimeEvent::ModelRequestCompleted {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        },
        RuntimeEvent::AttemptFailed {
            attempt_id: AttemptId::new("attempt-1"),
            error: AttemptFailure::Runtime {
                error: RuntimeError::UnknownTool {
                    name: "missing".to_owned(),
                },
            },
        },
    ]
}

/// An unknown tool fails the attempt explicitly with a typed runtime error
/// and produces no tool-execution event: nothing was resolved and nothing
/// executed, so no execution fact may claim otherwise.
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
    let result = run(&model, tools, &cancellation).await;

    assert_trace(&result.events, &expected_unknown_tool_trace());
    assert_single_terminal(&result.events);
    assert!(
        result.events.iter().all(|event| {
            !matches!(
                event,
                RuntimeEvent::ToolExecutionStarted { .. }
                    | RuntimeEvent::ToolExecutionCompleted { .. }
                    | RuntimeEvent::ToolExecutionFailed { .. }
            )
        }),
        "an unresolved tool must never produce a tool-execution event"
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
        result.terminal_state,
        ExecutionState::Failed,
        "the machine settles failed"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "no continuation is attempted after an unknown tool"
    );
    assert_eq!(
        result.messages.len(),
        1,
        "the agent tool-call message is never committed: preflight rejects          the structurally unresolvable call before the message commit"
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
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;

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
    FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("a")).register(&mut tools);
    FakeTool::new(common::tool("beta", "tool-beta"), success_result("b")).register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;

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

/// The generic Agent Loop checkpoint applies before the first model turn:
/// cancellation already observable at `run()` start settles cancelled with
/// zero `TurnStarted`, zero `ModelRequestStarted`, and zero adapter
/// requests, the single `AttemptCancelled` terminal is last, and the
/// cancellation reason is preserved.
#[tokio::test]
async fn cancellation_before_start_settles_cancelled() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    cancellation.cancel();
    let result = run(&model, tools, &cancellation).await;

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
    let result = run(&model, tools, &cancellation).await;
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

/// The exact trace of a tool call interrupted by cancellation.
/// Cancellation interrupts waiting for a tool with structural settlement:
/// the parked foreground execution observes the attempt signal and settles
/// as cancelled, the cancelled result slot is committed as a canonical tool
/// message, and only then does the attempt settle cancelled. The batch is
/// structurally complete exactly once.
#[tokio::test]
async fn cancellation_interrupts_waiting_for_tool() {
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
    // The parked tool is never released: `run()` must terminate without the
    // tool voluntarily returning.
    let (tool, _never_released) =
        FakeTool::parking(common::tool("alpha", "tool-alpha"), success_result("late"));
    let mut tool_started = tool.started();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        tool_started
            .wait_for(|running| *running)
            .await
            .expect("tool started");
        controller_cancellation.cancel();
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run(&model, tools, &cancellation),
    )
    .await
    .expect("run must terminate without the tool returning");
    controller.await.expect("controller task");

    assert_single_terminal(&result.events);
    let terminal = result.events.last().expect("terminal event");
    assert!(matches!(terminal, RuntimeEvent::AttemptCancelled { .. }));
    assert!(
        result
            .events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionCompleted { .. }))
            .count()
            == 1,
        "the interrupted execution still settles exactly once with a cancelled result"
    );
    let completed = result
        .events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::ToolExecutionCompleted { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("one completion");
    assert_eq!(
        completed.status,
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested
        },
        "the committed result slot is a cancelled result"
    );
    assert!(
        result
            .events
            .iter()
            .position(|event| matches!(event, RuntimeEvent::TurnCompleted))
            .is_some_and(|position| position < result.events.len() - 1),
        "the structurally complete batch commits before the terminal event"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "no continuation starts after cancellation"
    );
    let tool_messages: Vec<&MessageBlock> = result
        .messages
        .iter()
        .filter(|message| matches!(message, MessageBlock::Tool(_)))
        .collect();
    assert_eq!(
        tool_messages.len(),
        1,
        "the cancelled result slot is committed as exactly one tool message"
    );
    assert!(
        !matches!(result.outcome, AttemptOutcome::Completed { .. }),
        "cancellation never completes"
    );
    assert_outcome(
        &result,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested,
        },
    );
    assert_eq!(
        result.terminal_state,
        ExecutionState::Failed,
        "cancellation settles the machine through the failure path"
    );
}

/// Cancellation while waiting for a later tool call of the batch: earlier
/// results stay recorded, the pending foreground execution settles as
/// cancelled, and the structurally complete result batch commits in original
/// model call order before the attempt settles cancelled.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn cancellation_interrupts_later_tool_call() {
    let first = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({"n": 1}),
    };
    let second = ScriptedCall {
        id: "call-2",
        tool_id: "tool-beta",
        name: "beta",
        arguments: serde_json::json!({"n": 2}),
    };
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(tool_call_events(0, &first)[0].clone()),
        FakeStep::Emit(tool_call_events(0, &first)[1].clone()),
        FakeStep::Emit(tool_call_events(0, &first)[2].clone()),
        FakeStep::Emit(tool_call_events(1, &second)[0].clone()),
        FakeStep::Emit(tool_call_events(1, &second)[1].clone()),
        FakeStep::Emit(tool_call_events(1, &second)[2].clone()),
        FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
    ]]);
    let first_tool = FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("a"));
    let (second_tool, _never_released) =
        FakeTool::parking(common::tool("beta", "tool-beta"), success_result("b"));
    let mut second_started = second_tool.started();
    let mut tools = ToolRegistry::new();
    first_tool.register(&mut tools);
    second_tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        second_started
            .wait_for(|running| *running)
            .await
            .expect("second tool started");
        controller_cancellation.cancel();
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run(&model, tools, &cancellation),
    )
    .await
    .expect("run must terminate without the second tool returning");
    controller.await.expect("controller task");

    let executed: Vec<&str> = result
        .events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. } => {
                Some(tool_call_id.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        executed,
        vec!["call-1", "call-2"],
        "both executions settle exactly once; the second settles as cancelled"
    );
    let second_result = result
        .events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id,
                result,
                ..
            } if tool_call_id.as_str() == "call-2" => Some(result.clone()),
            _ => None,
        })
        .expect("call-2 completion");
    assert_eq!(
        second_result.status,
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested
        },
        "the later call receives a cancelled result slot"
    );
    assert_single_terminal(&result.events);
    assert!(matches!(
        result.events.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
    let tool_messages: Vec<&MessageBlock> = result
        .messages
        .iter()
        .filter(|message| matches!(message, MessageBlock::Tool(_)))
        .collect();
    assert_eq!(
        tool_messages.len(),
        2,
        "both result slots commit as canonical tool messages"
    );
    let call_ids: Vec<&str> = tool_messages
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool.tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        call_ids,
        vec!["call-1", "call-2"],
        "canonical tool messages commit in original model call order"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "no continuation after cancellation"
    );
    assert_eq!(
        result.terminal_state,
        ExecutionState::Failed,
        "the machine settles failed from WaitingForTool"
    );
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
    tool.register(&mut tools);
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
    let result = run(&model, tools, &cancellation).await;
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
        let result = run(&model, tools, &cancellation).await;
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
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;

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
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;
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
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;

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
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;

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
    let result = run(&model, tools, &cancellation).await;

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

/// The canonical final usage folds into `ModelRequestCompleted`: the
/// terminal event's reported usage wins, else the latest usage update.
/// Cumulative snapshots are never summed.
#[tokio::test]
async fn usage_folds_updates_and_terminal_usage() {
    let u1 = ModelUsage {
        input_tokens: 10,
        output_tokens: 5,
        total_tokens: 15,
        details: None,
    };
    let u2 = ModelUsage {
        input_tokens: 12,
        output_tokens: 7,
        total_tokens: 19,
        details: None,
    };
    // A: only a usage update, terminal usage absent.
    // B: two updates, terminal usage absent: the latest update wins, never
    //    a sum.
    // C: an update plus terminal reported usage: the terminal usage wins.
    let cases: Vec<(Vec<FakeStep>, Option<ModelUsage>)> = vec![
        (
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(ModelEvent::UsageUpdate { usage: u1.clone() }),
                FakeStep::Emit(text(0, "done")),
                FakeStep::Emit(done(ModelFinishReason::Stop)),
            ],
            Some(u1.clone()),
        ),
        (
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(ModelEvent::UsageUpdate { usage: u1.clone() }),
                FakeStep::Emit(ModelEvent::UsageUpdate { usage: u2.clone() }),
                FakeStep::Emit(done(ModelFinishReason::Stop)),
            ],
            Some(u2.clone()),
        ),
        (
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(ModelEvent::UsageUpdate { usage: u1.clone() }),
                FakeStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: Some(u2.clone()),
                }),
            ],
            Some(u2.clone()),
        ),
        // A stream with no usage at all reports none.
        (
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(text(0, "done")),
                FakeStep::Emit(done(ModelFinishReason::Stop)),
            ],
            None,
        ),
    ];
    for (script, expected) in cases {
        let model = FakeModel::new(vec![script]);
        let tools = ToolRegistry::new();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let result = run(&model, tools, &cancellation).await;
        let reported = result
            .events
            .iter()
            .find_map(|event| match event {
                RuntimeEvent::ModelRequestCompleted { usage, .. } => Some(usage.clone()),
                _ => None,
            })
            .expect("model request completion event");
        assert_eq!(reported, expected, "folded final usage");
        assert_single_terminal(&result.events);
    }
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
    let result = run(&model, tools, &cancellation).await;

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
    let result = run(&model, tools, &cancellation).await;
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
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;
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
    let result = run(&model, tools, &cancellation).await;
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
    let result = run(&model, tools, &cancellation).await;
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
    let result_first = run(&first, tools_first, &cancellation).await;
    let result_second = run(&second, tools_second, &cancellation).await;
    assert_eq!(result_first.events, result_second.events);
    assert_eq!(result_first.messages, result_second.messages);
    assert_eq!(result_first.outcome, result_second.outcome);
    assert_eq!(result_first.terminal_state, result_second.terminal_state);
}

/// Successful settlement completes the real state machine exactly once, and
/// the terminal event is always the last recorded event.
#[tokio::test]
async fn successful_settlement_completes_the_machine() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "done")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;
    assert_eq!(
        result.terminal_state,
        ExecutionState::Completed,
        "success settles the machine to Completed"
    );
    assert!(result.terminal_state.is_terminal());
    assert_single_terminal(&result.events);
}

/// Model failure settles the real machine to Failed.
#[tokio::test]
async fn model_failure_settles_the_machine_to_failed() {
    let model = FakeModel::new(vec![vec![FakeStep::Emit(fail(
        rustx::model::ModelErrorKind::Timeout,
        "boom",
    ))]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;
    assert_eq!(
        result.terminal_state,
        ExecutionState::Failed,
        "model failure settles the real machine to Failed"
    );
    assert!(result.terminal_state.is_terminal());
}

/// Runtime contract failure settles the real machine to Failed.
#[tokio::test]
async fn contract_failure_settles_the_machine_to_failed() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "hi")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
        FakeStep::Emit(text(0, "late")),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = run(&model, tools, &cancellation).await;
    assert_eq!(
        result.terminal_state,
        ExecutionState::Failed,
        "contract failure settles the real machine to Failed"
    );
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Runtime {
                error: RuntimeError::ContractViolation { .. }
            }
        }
    ));
}

/// Cancellation from `RunningModel` settles the real machine to Failed.
#[tokio::test]
async fn cancellation_from_running_model_settles_the_machine_to_failed() {
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
    let result = run(&model, tools, &cancellation).await;
    controller.await.expect("controller task");
    assert_eq!(
        result.terminal_state,
        ExecutionState::Failed,
        "cancellation from RunningModel settles the real machine to Failed"
    );
    assert!(result.terminal_state.is_terminal());
}

/// Cancellation from `WaitingForTool` settles the real machine to Failed.
#[tokio::test]
async fn cancellation_from_waiting_for_tool_settles_the_machine_to_failed() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let mut tool_turn = vec![FakeStep::Emit(started())];
    tool_turn.extend(tool_call_events(0, &call).into_iter().map(FakeStep::Emit));
    tool_turn.push(FakeStep::Emit(done(ModelFinishReason::ToolCalls)));
    let model = FakeModel::new(vec![tool_turn]);
    let (tool, _never_released) =
        FakeTool::parking(common::tool("alpha", "tool-alpha"), success_result("late"));
    let mut tool_started = tool.started();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        tool_started
            .wait_for(|running| *running)
            .await
            .expect("tool started");
        controller_cancellation.cancel();
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run(&model, tools, &cancellation),
    )
    .await
    .expect("run must terminate without the tool returning");
    controller.await.expect("controller task");
    assert_eq!(
        result.terminal_state,
        ExecutionState::Failed,
        "cancellation from WaitingForTool settles the real machine to Failed"
    );
    assert!(result.terminal_state.is_terminal());
}

/// The replay state sequence and the actual state-machine settlement can
/// never diverge: the replay's terminal phase equals the machine settlement
/// for every execution scenario.
#[tokio::test]
async fn replay_settlement_cannot_diverge_from_machine() {
    let call = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let tool_call = vec![
        FakeStep::Emit(tool_call_events(0, &call)[0].clone()),
        FakeStep::Emit(tool_call_events(0, &call)[1].clone()),
        FakeStep::Emit(tool_call_events(0, &call)[2].clone()),
    ];
    let scenarios: Vec<(FakeModel, ToolRegistry)> = vec![
        // Text execution completes.
        (
            FakeModel::new(vec![vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(text(0, "done")),
                FakeStep::Emit(done(ModelFinishReason::Stop)),
            ]]),
            ToolRegistry::new(),
        ),
        // Tool execution completes.
        (
            {
                let mut tool_turn = vec![FakeStep::Emit(started())];
                tool_turn.extend(tool_call.clone());
                tool_turn.push(FakeStep::Emit(done(ModelFinishReason::ToolCalls)));
                FakeModel::new(vec![
                    tool_turn,
                    vec![
                        FakeStep::Emit(started()),
                        FakeStep::Emit(done(ModelFinishReason::Stop)),
                    ],
                ])
            },
            {
                let mut tools = ToolRegistry::new();
                FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("ok"))
                    .register(&mut tools);
                tools
            },
        ),
        // Model failure.
        (
            FakeModel::new(vec![vec![FakeStep::Emit(fail(
                rustx::model::ModelErrorKind::RateLimit,
                "boom",
            ))]]),
            ToolRegistry::new(),
        ),
        // Contract violation.
        (
            FakeModel::new(vec![vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(done(ModelFinishReason::Stop)),
                FakeStep::Emit(fail(rustx::model::ModelErrorKind::Timeout, "late")),
            ]]),
            ToolRegistry::new(),
        ),
    ];
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    for (model, tools) in scenarios {
        let result = run(&model, tools, &cancellation).await;
        let replay = replay_execution_states(&result.events).expect("valid trace");
        assert_eq!(
            replay.last(),
            Some(&result.terminal_state),
            "replay terminal phase must equal the machine settlement"
        );
        assert!(
            result.terminal_state.is_terminal(),
            "the machine must settle terminally"
        );
    }
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
        let result = run(&model, tools, &cancellation).await;
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
    let result = run(&model, tools, &cancellation).await;
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

// ---------------------------------------------------------------------------
// Issue #22 — conversation inbound mailbox
// ---------------------------------------------------------------------------

/// A timestamped ordinary inbound message for mailbox enqueue.
fn inbound_user(id: &str, text: &str, source: UserSource) -> UserMessageBlock {
    UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(rustx::message::content::TextBlock {
            text: text.to_owned(),
        })],
        source,
        kind: rustx::message::types::InboundKind::Message,
        timestamp: Some(
            chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("parse fixed timestamp")
                .with_timezone(&chrono::Utc),
        ),
    }
}

/// The message id of one canonical block.
fn block_id(block: &MessageBlock) -> String {
    match block {
        MessageBlock::System(system) => system.id.to_string(),
        MessageBlock::User(user) => user.id.to_string(),
        MessageBlock::Agent(agent) => agent.id.to_string(),
        MessageBlock::Tool(tool) => tool.id.to_string(),
    }
}

/// Runs the attempt with the given mailbox bound as the canonical
/// conversation mailbox of the tool runtime.
async fn run_with_mailbox(
    model: &FakeModel,
    tools: ToolRegistry,
    cancellation: &AgentCancellation,
    mailbox: ConversationInboundMailbox,
) -> AgentExecutionResult {
    let tool_runtime = common::tool_runtime_with_mailbox("conv-1", mailbox);
    let capability = common::capability_lease(tools, &tool_runtime).await;
    AgentExecution::new(
        request("attempt-1"),
        model,
        capability.lease(),
        cancellation,
        runtime(),
        &tool_runtime,
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await
}

/// An attempt over a tool runtime of a different conversation is rejected
/// structurally: the request conversation and the conversation tool runtime
/// (and therefore its canonical mailbox) must agree.
#[tokio::test]
async fn conversation_mismatch_with_the_tool_runtime_is_rejected() {
    let model = FakeModel::new(Vec::new());
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime("conv-other");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let error = AgentExecution::new(
        request("attempt-1"),
        &model,
        capability.lease(),
        &cancellation,
        runtime(),
        &tool_runtime,
    )
    .err()
    .expect("a mismatched conversation must be rejected");
    assert!(matches!(error, MailboxError::ConversationMismatch { .. }));
}

/// Foreground tools with an empty mailbox preserve the exact M3 behavior:
/// tool result batch, then the continuation, with no synthetic user message.
#[tokio::test]
async fn foreground_tools_with_empty_mailbox_keep_exact_behavior() {
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
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
    let result = run_with_mailbox(&model, tools, &cancellation, mailbox).await;

    assert_trace(&result.events, &expected_single_tool_trace());
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    let user_blocks: Vec<&MessageBlock> = result
        .messages
        .iter()
        .filter(|block| matches!(block, MessageBlock::User(_)))
        .collect();
    assert_eq!(
        user_blocks.len(),
        1,
        "no synthetic user message appears when the mailbox is empty"
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let ids: Vec<String> = requests[1].messages.iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec!["msg-user-1", "attempt-1-agent-1", "attempt-1-tool-1-call-1"],
        "the continuation carries input + agent + tool result only"
    );
}

/// Foreground tools with an inbound batch: the batch drains only after the
/// complete tool-result batch committed, and the next model request sees
/// both inbound messages in one continuation.
#[tokio::test]
async fn foreground_tools_with_inbound_batch_attach_one_ordered_batch() {
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
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let (tool, release) = FakeTool::parking(
        common::tool("alpha", "tool-alpha"),
        success_result("listed"),
    );
    let mut tool_started = tool.started();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
    let controller_mailbox = mailbox.clone();
    let controller = tokio::spawn(async move {
        tool_started
            .wait_for(|running| *running)
            .await
            .expect("tool started");
        let sequence_a = controller_mailbox
            .enqueue(inbound_user("msg-inbound-a", "human A", UserSource::Human))
            .expect("enqueue human A");
        let sequence_b = controller_mailbox
            .enqueue(inbound_user(
                "msg-inbound-b",
                "runtime B",
                UserSource::Runtime,
            ))
            .expect("enqueue runtime B");
        assert!(
            sequence_a < sequence_b,
            "Human and Runtime share one sequence domain"
        );
        release.notify_waiters();
    });
    let result = run_with_mailbox(&model, tools, &cancellation, mailbox.clone()).await;
    controller.await.expect("controller task");
    assert!(mailbox.drain().is_none(), "the drained batch is consumed");

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    // Canonical history before the continuation: agent tool call, tool
    // result, then the distinct timestamped inbound messages in sequence
    // order, followed by the final agent message. The drain never splits
    // the tool-result batch.
    let ids: Vec<String> = result.messages.iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec![
            "msg-user-1".to_owned(),
            "attempt-1-agent-1".to_owned(),
            "attempt-1-tool-1-call-1".to_owned(),
            "msg-inbound-a".to_owned(),
            "msg-inbound-b".to_owned(),
            "attempt-1-agent-2".to_owned(),
        ]
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 2, "one continuation after the batch");
    let MessageBlock::User(user_a) = &requests[1].messages[3] else {
        panic!("fourth message of the continuation must be user A");
    };
    assert_eq!(user_a.id, MessageId::new("msg-inbound-a"));
    assert_eq!(user_a.source, UserSource::Human);
    assert!(
        user_a.timestamp.is_some(),
        "A keeps its persisted timestamp"
    );
    let MessageBlock::User(user_b) = &requests[1].messages[4] else {
        panic!("fifth message of the continuation must be user B");
    };
    assert_eq!(user_b.id, MessageId::new("msg-inbound-b"));
    assert_eq!(user_b.source, UserSource::Runtime);
    assert!(
        user_a.content != user_b.content || user_a.id != user_b.id,
        "A and B stay distinct canonical messages"
    );
    assert!(mailbox.drain().is_none(), "the drained batch is consumed");
}

/// The later-correction regression: enqueuing "deploy it" then "actually do
/// not deploy it" during one running tool turn yields exactly one batch and
/// exactly one subsequent model request observing both, in order.
#[tokio::test]
async fn later_correction_ships_one_batch_and_one_continuation() {
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
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let (tool, release) = FakeTool::parking(
        common::tool("alpha", "tool-alpha"),
        success_result("listed"),
    );
    let mut tool_started = tool.started();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
    let controller_mailbox = mailbox.clone();
    let controller = tokio::spawn(async move {
        tool_started
            .wait_for(|running| *running)
            .await
            .expect("tool started");
        controller_mailbox
            .enqueue(inbound_user("msg-corr-1", "deploy it", UserSource::Human))
            .expect("enqueue correction A");
        controller_mailbox
            .enqueue(inbound_user(
                "msg-corr-2",
                "actually do not deploy it",
                UserSource::Human,
            ))
            .expect("enqueue correction B");
        release.notify_waiters();
    });
    let result = run_with_mailbox(&model, tools, &cancellation, mailbox).await;
    controller.await.expect("controller task");

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    let requests = model.requests();
    assert_eq!(
        requests.len(),
        2,
        "exactly one subsequent model request sees the correction batch"
    );
    let ids: Vec<String> = requests[1].messages.iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec![
            "msg-user-1".to_owned(),
            "attempt-1-agent-1".to_owned(),
            "attempt-1-tool-1-call-1".to_owned(),
            "msg-corr-1".to_owned(),
            "msg-corr-2".to_owned(),
        ],
        "A then B in inbound sequence order, never a request containing only A"
    );
    assert_eq!(
        requests[1].messages[3..]
            .iter()
            .filter(|block| matches!(block, MessageBlock::User(_)))
            .count(),
        2,
        "the correction messages remain separate canonical user messages"
    );
}

/// Model Stop with pending inbound is the formerly ambiguous case: the
/// first Stop must not settle the attempt before the already-pending
/// inbound work was observed.
#[tokio::test]
async fn stop_with_pending_inbound_does_not_settle_until_batch_consumed() {
    let (release, parked) = model_release();
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "doing")),
            FakeStep::ParkUntilReleased(parked.clone()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "done")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
    let controller_mailbox = mailbox.clone();
    let mut model_parked = model.parked();
    let controller = tokio::spawn(async move {
        model_parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("model parked");
        controller_mailbox
            .enqueue(inbound_user("msg-stop-a", "deploy it", UserSource::Human))
            .expect("enqueue while turn 1 is in flight");
        release.send(true).expect("release turn 1");
    });
    let result = run_with_mailbox(&model, tools, &cancellation, mailbox).await;
    controller.await.expect("controller task");

    assert_eq!(
        model.requests().len(),
        2,
        "the first Stop must not settle the attempt while inbound work is pending"
    );
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    let ids: Vec<String> = result.messages.iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec![
            "msg-user-1".to_owned(),
            "attempt-1-agent-1".to_owned(),
            "msg-stop-a".to_owned(),
            "attempt-1-agent-2".to_owned(),
        ],
        "turn 1 AgentMessage, the drained inbound message, then the final turn"
    );
    let requests = model.requests();
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|block| matches!(block, MessageBlock::User(user) if user.id == MessageId::new("msg-stop-a"))),
        "model request 2 contains the drained inbound message"
    );
}

/// The finite settlement rule: a successful no-tool turn whose safe-boundary
/// snapshot observes an empty mailbox settles; a message enqueued after that
/// snapshot never reopens the attempt and stays in the conversation mailbox.
#[tokio::test]
async fn empty_snapshot_settlement_is_finite_and_never_reopens() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "done")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
    let result = run_with_mailbox(&model, tools, &cancellation, mailbox.clone()).await;

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert_eq!(
        model.requests().len(),
        1,
        "the empty snapshot permits immediate settlement"
    );
    // Strictly after the empty-snapshot linearization point (the attempt
    // already settled): the new message stays queued for later conversation
    // processing; the old attempt is never reopened.
    mailbox
        .enqueue(inbound_user("msg-late-1", "new work", UserSource::Human))
        .expect("enqueue after settlement");
    assert_eq!(
        model.requests().len(),
        1,
        "a later enqueue never reopens the settled attempt"
    );
    let batch = mailbox.drain().expect("the late message remains pending");
    assert_eq!(batch.items().len(), 1);
    assert_eq!(batch.items()[0].message().id, MessageId::new("msg-late-1"));
    assert_eq!(
        batch.watermark().get(),
        1,
        "a fresh mailbox starts at sequence 1"
    );
}

/// Cancellation observable before the safe-boundary snapshot: no drain
/// happens, the pending item stays in the mailbox, and nothing is appended.
#[tokio::test]
async fn cancellation_before_safe_boundary_leaves_mailbox_untouched() {
    let (release, parked) = model_release();
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "doing")),
        FakeStep::ParkUntilReleased(parked.clone()),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
    let controller_mailbox = mailbox.clone();
    let controller_cancellation = cancellation.clone();
    let mut model_parked = model.parked();
    let controller = tokio::spawn(async move {
        model_parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("model parked");
        controller_mailbox
            .enqueue(inbound_user("msg-cancel-a", "pending", UserSource::Human))
            .expect("enqueue pending message");
        controller_cancellation.cancel();
        release.send(true).expect("release turn");
    });
    let result = run_with_mailbox(&model, tools, &cancellation, mailbox.clone()).await;
    controller.await.expect("controller task");

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested,
        },
    );
    assert!(
        !result
            .messages
            .iter()
            .any(|block| matches!(block, MessageBlock::User(user) if user.id == MessageId::new("msg-cancel-a"))),
        "the pending message is not appended to attempt history"
    );
    let batch = mailbox.drain().expect("the pending message remains");
    assert_eq!(batch.items().len(), 1);
    assert_eq!(
        batch.items()[0].message().id,
        MessageId::new("msg-cancel-a")
    );
}

/// Cancellation observed mid-continuation (after the drained batch already
/// committed at the first safe boundary): the batch stays canonical exactly
/// once, never requeued, and the attempt settles cancelled without the
/// in-flight continuation completing.
#[tokio::test]
async fn cancellation_mid_continuation_keeps_drained_batch_canonical() {
    let first = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({"n": 1}),
    };
    let second = ScriptedCall {
        id: "call-2",
        tool_id: "tool-beta",
        name: "beta",
        arguments: serde_json::json!({"n": 2}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &first)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &first)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &first)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &second)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &second)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &second)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
    ]);
    let (first_tool, first_release) =
        FakeTool::parking(common::tool("alpha", "tool-alpha"), success_result("a"));
    let (second_tool, _never_released) =
        FakeTool::parking(common::tool("beta", "tool-beta"), success_result("b"));
    let mut first_started = first_tool.started();
    let mut second_started = second_tool.started();
    let mut tools = ToolRegistry::new();
    first_tool.register(&mut tools);
    second_tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
    let controller_mailbox = mailbox.clone();
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        first_started
            .wait_for(|running| *running)
            .await
            .expect("first tool started");
        controller_mailbox
            .enqueue(inbound_user("msg-commit-a", "committed", UserSource::Human))
            .expect("enqueue before the first safe boundary");
        first_release.notify_waiters();
        // Turn 2 begins only after the first safe boundary drained and
        // appended the batch; cancellation is observed during turn 2's tool
        // wait, after the batch commit point.
        second_started
            .wait_for(|running| *running)
            .await
            .expect("second tool started");
        controller_cancellation.cancel();
    });
    let result = run_with_mailbox(&model, tools, &cancellation, mailbox.clone()).await;
    controller.await.expect("controller task");

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested,
        },
    );
    let committed: Vec<&MessageBlock> = result
        .messages
        .iter()
        .filter(|block| matches!(block, MessageBlock::User(user) if user.id == MessageId::new("msg-commit-a")))
        .collect();
    assert_eq!(
        committed.len(),
        1,
        "the drained batch exists exactly once in canonical history"
    );
    assert!(
        mailbox.drain().is_none(),
        "the appended batch is consumed from the mailbox and never requeued"
    );
    assert_eq!(
        model.requests().len(),
        2,
        "cancellation observed mid-continuation issues no third model request"
    );
}

/// A terminal model failure never drains the mailbox: the pending item
/// remains available for later conversation processing.
#[tokio::test]
async fn terminal_model_failure_leaves_pending_inbound_untouched() {
    let (release, parked) = model_release();
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text(0, "partial")),
        FakeStep::ParkUntilReleased(parked.clone()),
        FakeStep::Emit(fail(
            rustx::model::ModelErrorKind::ProviderError,
            "stream broke",
        )),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
    let controller_mailbox = mailbox.clone();
    let mut model_parked = model.parked();
    let controller = tokio::spawn(async move {
        model_parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("model parked");
        controller_mailbox
            .enqueue(inbound_user("msg-fail-a", "pending", UserSource::Human))
            .expect("enqueue pending message");
        release.send(true).expect("release the failing turn");
    });
    let result = run_with_mailbox(&model, tools, &cancellation, mailbox.clone()).await;
    controller.await.expect("controller task");

    assert_single_terminal(&result.events);
    assert!(matches!(
        result.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model { .. }
        }
    ));
    assert!(
        !result
            .messages
            .iter()
            .any(|block| matches!(block, MessageBlock::User(user) if user.id == MessageId::new("msg-fail-a"))),
        "no post-failure drain appends the pending message"
    );
    assert_eq!(
        mailbox.drain().expect("pending").items()[0].message().id,
        MessageId::new("msg-fail-a")
    );
}

/// An unknown tool failure settles the attempt without consuming the
/// pending mailbox work.
#[tokio::test]
async fn unknown_tool_failure_leaves_pending_inbound_untouched() {
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
    let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
    let controller_mailbox = mailbox.clone();
    let mut emitted = model.emitted();
    let controller = tokio::spawn(async move {
        emitted
            .wait_for(|count| *count >= 1)
            .await
            .expect("model request in flight");
        controller_mailbox
            .enqueue(inbound_user("msg-ut-a", "pending", UserSource::Runtime))
            .expect("enqueue while the request is in flight");
    });
    let result = run_with_mailbox(&model, tools, &cancellation, mailbox.clone()).await;
    controller.await.expect("controller task");

    assert_single_terminal(&result.events);
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
        mailbox.drain().expect("pending").items()[0].message().id,
        MessageId::new("msg-ut-a")
    );
}

/// An ordinary inbound drain preserves the pending provider continuation:
/// the next model request retains the same continuation state when no
/// compaction occurred.
#[tokio::test]
async fn continuation_retained_across_inbound_drain() {
    let state = ProviderContinuationState::Anthropic(AnthropicContinuation {
        opaque: serde_json::json!({"signature": "sig-1", "opaque": [1, 2, 3]}),
    });
    let (release, parked) = model_release();
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(reasoning(0, "Thinking.")),
            FakeStep::Emit(ModelEvent::ContinuationState {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                state: state.clone(),
            }),
            FakeStep::Emit(text(1, "doing")),
            FakeStep::ParkUntilReleased(parked.clone()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
    let controller_mailbox = mailbox.clone();
    let mut model_parked = model.parked();
    let controller = tokio::spawn(async move {
        model_parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("model parked");
        controller_mailbox
            .enqueue(inbound_user("msg-cont-a", "continue", UserSource::Human))
            .expect("enqueue inbound message");
        release.send(true).expect("release turn 1");
    });
    let result = run_with_mailbox(&model, tools, &cancellation, mailbox).await;
    controller.await.expect("controller task");

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].continuation,
        Some(state),
        "the ordinary inbound drain does not invalidate the continuation"
    );
}

/// One attempt may consume several batches, but never more than one batch
/// at one safe boundary: each tool turn drains exactly the items pending at
/// its own boundary.
#[tokio::test]
async fn one_attempt_consumes_multiple_batches_at_different_boundaries() {
    let first = ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({"n": 1}),
    };
    let second = ScriptedCall {
        id: "call-2",
        tool_id: "tool-beta",
        name: "beta",
        arguments: serde_json::json!({"n": 2}),
    };
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &first)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &first)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &first)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &second)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &second)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &second)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text(0, "done")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let (first_tool, first_release) =
        FakeTool::parking(common::tool("alpha", "tool-alpha"), success_result("a"));
    let (second_tool, second_release) =
        FakeTool::parking(common::tool("beta", "tool-beta"), success_result("b"));
    let mut first_started = first_tool.started();
    let mut second_started = second_tool.started();
    let mut tools = ToolRegistry::new();
    first_tool.register(&mut tools);
    second_tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mailbox = ConversationInboundMailbox::new(ConversationId::new("conv-1"));
    let controller_mailbox = mailbox.clone();
    let controller = tokio::spawn(async move {
        first_started
            .wait_for(|running| *running)
            .await
            .expect("first tool started");
        controller_mailbox
            .enqueue(inbound_user("msg-batch-a", "A", UserSource::Human))
            .expect("enqueue A before boundary 1");
        first_release.notify_waiters();
        second_started
            .wait_for(|running| *running)
            .await
            .expect("second tool started");
        controller_mailbox
            .enqueue(inbound_user("msg-batch-c", "C", UserSource::Human))
            .expect("enqueue C before boundary 2");
        second_release.notify_waiters();
    });
    let result = run_with_mailbox(&model, tools, &cancellation, mailbox).await;
    controller.await.expect("controller task");

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert_eq!(
        model.requests().len(),
        3,
        "turn 3 settles only after both boundaries drained their batches"
    );
    let ids: Vec<String> = result.messages.iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec![
            "msg-user-1".to_owned(),
            "attempt-1-agent-1".to_owned(),
            "attempt-1-tool-1-call-1".to_owned(),
            "msg-batch-a".to_owned(),
            "attempt-1-agent-2".to_owned(),
            "attempt-1-tool-2-call-2".to_owned(),
            "msg-batch-c".to_owned(),
            "attempt-1-agent-3".to_owned(),
        ],
        "batch A at boundary 1, batch C at boundary 2, never merged"
    );
}
