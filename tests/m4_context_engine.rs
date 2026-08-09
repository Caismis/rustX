//! M4 deterministic context engine tests.
//!
//! Every test is deterministic and network-free. Engine-level tests drive
//! `ContextEngine` directly with scripted estimators; agent-level tests
//! drive `AgentExecution` with the M4 `ContextRuntime` bundle over scripted
//! fixture models, tools, and summarizers, and assert behavior through the
//! recorded `RuntimeEvent` trace, the platform outcome, the committed
//! canonical history, and the recorded requests.

mod common;

use std::sync::Arc;

use common::context::{FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator};
use common::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, model_release, success_result, tool_call_events,
};
use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionRequest, AgentExecutionResult,
};
use rustx::context::{
    ContextBoundary, ContextCheckpointStore, ContextConfig, ContextEngine, ContextError,
    ContextErrorKind, ContextRuntime, ContextSummarizer, DefaultTokenEstimator,
    InMemoryCheckpointStore, ModelBackedSummarizer, ProjectionItem, ProviderObservedInput,
    SummaryInputItem, SummaryModelConfig, SummaryRequest, TokenEstimator, TokenMeasurement,
    TokenMeasurementSource,
};
use rustx::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AgentContentBlock, AgentMessageBlock, ContentBlockIndex, InboundKind, MessageBlock,
    ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::{ModelProtocol, ModelRequest, ModelUsage, ReasoningEffort};
use rustx::runtime::continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId};
use rustx::runtime::inbound::ConversationInboundMailbox;
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{ToolCall, ToolCallStart, ToolExecutionResult, ToolExecutionStatus};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn user(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

fn system(id: &str, text: &str) -> MessageBlock {
    MessageBlock::System(rustx::message::types::SystemMessageBlock {
        id: MessageId::new(id),
        authority: rustx::message::types::SystemAuthority::Platform,
        content: vec![TextBlock {
            text: text.to_owned(),
        }],
    })
}

fn text_block(text: &str) -> AgentContentBlock {
    AgentContentBlock::Text(TextBlock {
        text: text.to_owned(),
    })
}

fn call_block(id: &str) -> AgentContentBlock {
    AgentContentBlock::ToolCall(ToolCall {
        id: ToolCallId::new(id),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".to_owned(),
        arguments: serde_json::json!({}),
    })
}

fn agent(id: &str, blocks: Vec<AgentContentBlock>) -> MessageBlock {
    MessageBlock::Agent(AgentMessageBlock {
        id: MessageId::new(id),
        content: blocks,
    })
}

fn tool_message(id: &str, call_id: &str) -> MessageBlock {
    MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new(id),
        tool_call_id: ToolCallId::new(call_id),
        tool_id: ToolId::new("tool-alpha"),
        result: ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: vec![],
            duration_ms: 1,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
        },
    })
}

fn engine(
    window: u64,
    reserve: u64,
    keep_recent: u64,
    estimator: Arc<dyn TokenEstimator>,
) -> ContextEngine {
    ContextEngine::new(
        ContextConfig {
            context_window_tokens: window,
            reserve_tokens: reserve,
            keep_recent_tokens: keep_recent,
        },
        estimator,
    )
    .expect("valid context configuration")
}

fn weighted(per_message: u64, per_block: u64, per_tool: u64) -> Arc<ScriptedEstimator> {
    Arc::new(ScriptedEstimator::new(per_message, per_block, per_tool))
}

fn scripted(
    per_message: u64,
    per_block: u64,
    per_tool: u64,
    overrides: &[(&str, u64)],
) -> Arc<ScriptedEstimator> {
    let mut estimator = ScriptedEstimator::new(per_message, per_block, per_tool);
    for (id, tokens) in overrides {
        estimator = estimator.with_override(id, *tokens);
    }
    Arc::new(estimator)
}

fn conversation() -> ConversationId {
    ConversationId::new("conv-1")
}

fn summary_id(generation: u64) -> MessageId {
    rustx::context::summary_message_id(&conversation(), generation)
}

fn started() -> ModelEvent {
    ModelEvent::Started
}

fn text_delta(index: u32, delta: &str) -> ModelEvent {
    ModelEvent::TextDelta {
        block_index: ContentBlockIndex::new(index),
        text: delta.to_owned(),
    }
}

fn done(reason: ModelFinishReason) -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: reason,
        usage: None,
    }
}

fn done_with_usage(reason: ModelFinishReason, input_tokens: u64) -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: reason,
        usage: Some(ModelUsage {
            input_tokens,
            output_tokens: 4,
            total_tokens: input_tokens + 4,
            details: None,
        }),
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

fn overflow_event() -> ModelEvent {
    fail(
        rustx::model::ModelErrorKind::ContextWindowExceeded,
        "context window exceeded",
    )
}

fn overflow_error() -> rustx::model::ModelError {
    rustx::model::ModelError {
        kind: rustx::model::ModelErrorKind::ContextWindowExceeded,
        message: "context window exceeded".to_owned(),
        retry_after_ms: None,
        provider_code: None,
    }
}

fn request(
    attempt: &str,
    initial_messages: Vec<MessageBlock>,
    max_output_tokens: u32,
) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id: conversation(),
        attempt_id: AttemptId::new(attempt),
        initial_messages,
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: "fake-model".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        reasoning: ReasoningEffort::Medium,
        max_output_tokens,
    }
}

fn agent_message_id(turn: u32) -> MessageId {
    MessageId::new(format!("attempt-1-agent-{turn}"))
}

fn retry_message_id(turn: u32) -> MessageId {
    MessageId::new(format!("attempt-1-agent-{turn}-retry-1"))
}

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

fn assert_outcome(result: &AgentExecutionResult, expected: &AttemptOutcome) {
    assert_eq!(
        result.outcome, *expected,
        "platform outcome mismatch: {:?}",
        result.events
    );
}

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

fn call_start() -> ToolCallStart {
    ToolCallStart {
        id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".to_owned(),
    }
}

fn call_done() -> ToolCall {
    ToolCall {
        id: ToolCallId::new("call-1"),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".to_owned(),
        arguments: serde_json::json!({}),
    }
}

fn scripted_call() -> ScriptedCall {
    ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    }
}

fn tool_registry_with_alpha() -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    FakeTool::new(common::tool("alpha", "tool-alpha"), success_result("ok")).register(&mut tools);
    tools
}

fn runtime_with(
    window: u64,
    reserve: u64,
    keep_recent: u64,
    estimator: Arc<dyn TokenEstimator>,
    summarizer: FakeContextSummarizer,
    store: Arc<InMemoryCheckpointStore>,
) -> ContextRuntime<'static> {
    ContextRuntime::new(
        engine(window, reserve, keep_recent, estimator),
        Arc::new(summarizer),
        store,
    )
}

fn message_id_of(message: &MessageBlock) -> String {
    match message {
        MessageBlock::System(system) => system.id.as_str().to_owned(),
        MessageBlock::User(user) => user.id.as_str().to_owned(),
        MessageBlock::Agent(agent) => agent.id.as_str().to_owned(),
        MessageBlock::Tool(tool) => tool.id.as_str().to_owned(),
    }
}

fn checkpoint(
    generation: u64,
    summary_text: &str,
    boundary: ContextBoundary,
    tokens_before: TokenMeasurement,
) -> rustx::context::ContextCheckpoint {
    rustx::context::ContextCheckpoint {
        conversation_id: conversation(),
        generation,
        summary: UserMessageBlock {
            id: summary_id(generation),
            content: vec![UserContentBlock::Text(TextBlock {
                text: summary_text.to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::CompactionSummary,
            timestamp: None,
        },
        boundary,
        tokens_before,
        estimated_tokens_after: 0,
    }
}

// ---------------------------------------------------------------------------
// Context assembly
// ---------------------------------------------------------------------------

/// A short history stays below the threshold: no compaction.
#[test]
fn short_history_requires_no_compaction() {
    let engine = engine(100, 10, 5, weighted(10, 10, 10));
    let history = vec![user("u1", "hi"), user("u2", "bye")];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    assert!(
        !engine
            .should_compact(&projection, 0)
            .expect("threshold decision")
    );
    assert!(engine.fits_under_soft_limit(&projection, 0).expect("fits"));
}

/// Projection ordering is deterministic: pinned system prefix, checkpoint
/// summary, retained suffix.
#[test]
fn projection_ordering_is_deterministic() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let history = vec![
        system("sys-1", "Be concise."),
        user("u1", "hi"),
        agent("a1", vec![text_block("ok")]),
        user("u2", "more"),
    ];
    let checkpoint = checkpoint(
        1,
        "earlier summary",
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("u1"),
        },
        TokenMeasurement {
            input_tokens: 30,
            source: TokenMeasurementSource::Estimated,
        },
    );
    let first = engine
        .build_projection(&history, Some(&checkpoint), &[], None, None)
        .expect("projection");
    let second = engine
        .build_projection(&history, Some(&checkpoint), &[], None, None)
        .expect("projection again");
    assert_eq!(first, second, "projection must be a pure function");
    let kinds: Vec<&str> = first
        .items
        .iter()
        .map(|item| match item {
            ProjectionItem::Message(MessageBlock::System(_)) => "system",
            ProjectionItem::Message(MessageBlock::User(user))
                if user.kind == InboundKind::CompactionSummary =>
            {
                "summary"
            }
            ProjectionItem::Message(_) => "suffix",
            ProjectionItem::AgentSlice { .. } => "slice",
        })
        .collect();
    assert_eq!(kinds, vec!["system", "summary", "suffix", "suffix"]);
    assert_eq!(first.checkpoint_generation, Some(1));
}

/// The same history produces the same estimate.
#[test]
fn same_context_produces_same_estimate() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let history = vec![user("u1", "hi"), agent("a1", vec![text_block("ok")])];
    let first = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let second = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection again");
    assert_eq!(first.estimated_input, second.estimated_input);
    assert_eq!(
        first.estimated_input.source,
        TokenMeasurementSource::Estimated
    );
}

/// Tool definitions contribute to the planned request estimate.
#[test]
fn tool_definitions_contribute_to_the_request_estimate() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let history = vec![user("u1", "hi")];
    let tools = vec![
        common::model_tool("alpha", "tool-alpha"),
        common::model_tool("beta", "tool-beta"),
    ];
    let without_tools = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection without tools");
    let with_tools = engine
        .build_projection(&history, None, &tools, None, None)
        .expect("projection with tools");
    assert_eq!(with_tools.estimated_input.input_tokens, 30);
    assert_eq!(without_tools.estimated_input.input_tokens, 10);
}

/// Tool definitions never satisfy the recent-conversation retention target:
/// the retention decision is a pure function of conversation content, while
/// the full request estimate still includes the tool overhead.
#[test]
fn tool_definitions_never_satisfy_the_recent_retention_target() {
    let history = vec![
        user("u1", ""),
        agent("a1", vec![text_block("x")]),
        user("u2", ""),
        agent("a2", vec![text_block("y")]),
    ];
    let tools = vec![common::model_tool("alpha", "tool-alpha")];
    // Target 20: with conversation weights of 10/10, retiring u1 and a1
    // retains exactly u2+a2 = 20. If the huge tool weight counted toward the
    // target, the engine would retire everything instead.
    let cheap = engine(10_000_000, 0, 20, weighted(10, 10, 0));
    let expensive = engine(10_000_000, 0, 20, weighted(10, 10, 1_000_000));
    let projection_cheap = cheap
        .build_projection(&history, None, &tools, None, None)
        .expect("projection");
    let projection_expensive = expensive
        .build_projection(&history, None, &tools, None, None)
        .expect("projection");
    let plan_cheap = cheap
        .plan_compaction(
            &history,
            None,
            &projection_cheap,
            &tools,
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    let plan_expensive = expensive
        .plan_compaction(
            &history,
            None,
            &projection_expensive,
            &tools,
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // Identical retention decision: the tool weight changes the full request
    // estimate but never the recent-conversation target.
    assert_eq!(
        plan_cheap.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("a1"),
        }
    );
    assert_eq!(plan_cheap.boundary, plan_expensive.boundary);
    assert!(plan_expensive.split_turn_prefix.is_none());
    // The full request estimate still reflects the tool overhead.
    assert!(
        plan_expensive.planned_estimate_after > plan_cheap.planned_estimate_after,
        "tool definitions still affect the full request estimate"
    );
}

// ---------------------------------------------------------------------------
// Token accounting
// ---------------------------------------------------------------------------

/// A provider-reported measurement applies only to exactly the projection
/// that was measured; everything else is a deterministic estimate.
#[test]
fn provider_reported_usage_applies_only_to_the_exact_projection() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let history = vec![user("u1", "hi")];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: projection.fingerprint(),
        input_tokens: 42,
    };
    let measured = engine
        .build_projection(&history, None, &[], Some(&observed), None)
        .expect("projection with observed usage");
    assert_eq!(measured.estimated_input.input_tokens, 42);
    assert_eq!(
        measured.estimated_input.source,
        TokenMeasurementSource::ProviderReported
    );

    // A different history is a different projection: the observed
    // measurement does not apply, and the estimate is used instead.
    let grown = vec![user("u1", "hi"), user("u2", "more")];
    let estimated = engine
        .build_projection(&grown, None, &[], Some(&observed), None)
        .expect("projection with stale observation");
    assert_eq!(estimated.estimated_input.input_tokens, 20);
    assert_eq!(
        estimated.estimated_input.source,
        TokenMeasurementSource::Estimated
    );
}

/// Missing provider usage means the deterministic estimate, never a
/// fabricated measurement.
#[test]
fn missing_usage_falls_back_to_the_estimate() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let history = vec![user("u1", "hi")];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    assert_eq!(projection.estimated_input.input_tokens, 10);
    assert_eq!(
        projection.estimated_input.source,
        TokenMeasurementSource::Estimated
    );
}

/// An estimate never becomes provider usage: the measurement stays an
/// estimate with explicit provenance, and no `ModelUsage` is derived from
/// it anywhere in the context plane.
#[test]
fn estimates_never_become_model_usage() {
    let engine = engine(1_000, 10, 5, Arc::new(DefaultTokenEstimator));
    let history = vec![user("u1", "hi")];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    assert_eq!(
        projection.estimated_input.source,
        TokenMeasurementSource::Estimated
    );
    // `TokenMeasurement` is the only measurement type the projection
    // carries: there is no conversion path from an estimate to `ModelUsage`.
    assert!(matches!(
        projection.estimated_input,
        TokenMeasurement {
            source: TokenMeasurementSource::Estimated,
            ..
        }
    ));
}

/// The default estimator is deterministic and implements the documented
/// `ceil(bytes / 4)` formula over runtime-owned canonical serialization.
#[test]
fn default_estimator_formula_is_frozen() {
    let engine = engine(1_000, 10, 5, Arc::new(DefaultTokenEstimator));
    let history = vec![user("u1", "hi")];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let expected = rustx::context::bytes_to_tokens(
        serde_json::to_vec(&projection.items)
            .expect("serialize")
            .len() as u64,
    );
    assert_eq!(projection.estimated_input.input_tokens, expected);
}

// ---------------------------------------------------------------------------
// Threshold
// ---------------------------------------------------------------------------

/// Compaction triggers at `estimated >= soft_input_limit`; equality
/// compacts deterministically.
#[test]
fn threshold_equality_compacts() {
    let engine = engine(100, 0, 5, weighted(20, 20, 20));
    let at = engine
        .build_projection(&vec![user("u1", ""); 5], None, &[], None, None)
        .expect("projection");
    assert_eq!(at.estimated_input.input_tokens, 100);
    assert!(
        engine
            .should_compact(&at, 0)
            .expect("at threshold: compact")
    );

    let below = engine
        .build_projection(&vec![user("u1", ""); 4], None, &[], None, None)
        .expect("projection");
    assert_eq!(below.estimated_input.input_tokens, 80);
    assert!(
        !engine
            .should_compact(&below, 0)
            .expect("below threshold: no compaction")
    );

    let above = engine
        .build_projection(&vec![user("u1", ""); 6], None, &[], None, None)
        .expect("projection");
    assert_eq!(above.estimated_input.input_tokens, 120);
    assert!(engine.should_compact(&above, 0).expect("above threshold"));
}

/// The soft limit accounts for the output budget and the reserve.
#[test]
fn soft_limit_respects_output_budget_and_reserve() {
    let engine = engine(200, 40, 5, weighted(10, 10, 10));
    assert_eq!(engine.soft_input_limit(0).expect("no output"), 160);
    assert_eq!(engine.soft_input_limit(60).expect("output budget"), 100);
    assert!(
        engine.soft_input_limit(160).is_err(),
        "window <= reserve + output must be rejected"
    );
    assert!(
        engine.soft_input_limit(161).is_err(),
        "window < reserve + output must be rejected"
    );
}

/// Impossible context configurations are rejected explicitly; no fallback
/// constant is hidden.
#[test]
fn invalid_configuration_is_rejected() {
    let error = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 100,
            reserve_tokens: 100,
            keep_recent_tokens: 5,
        },
        weighted(10, 10, 10),
    )
    .expect_err("window == reserve must be rejected");
    assert_eq!(error.kind, ContextErrorKind::InvalidConfiguration);

    let error = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 90,
            reserve_tokens: 100,
            keep_recent_tokens: 5,
        },
        weighted(10, 10, 10),
    )
    .expect_err("window < reserve must be rejected");
    assert_eq!(error.kind, ContextErrorKind::InvalidConfiguration);

    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 100,
            reserve_tokens: 99,
            keep_recent_tokens: 5,
        },
        weighted(10, 10, 10),
    )
    .expect("a one-token budget is legal");
    assert_eq!(
        engine
            .soft_input_limit(1)
            .expect_err("no room for output")
            .kind,
        ContextErrorKind::InvalidConfiguration
    );
    assert_eq!(engine.soft_input_limit(0).expect("one token"), 1);
}

// ---------------------------------------------------------------------------
// Whole cut points
// ---------------------------------------------------------------------------

/// A simple complete turn retires at a whole-turn boundary that covers the
/// turn's tool results.
#[test]
fn simple_complete_turn_boundary() {
    let engine = engine(100, 0, 5, weighted(100, 10, 100));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(
        plan.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("t1"),
        }
    );
    assert_eq!(plan.newly_retired.len(), 3);
    assert!(plan.split_turn_prefix.is_none());
}

/// Multiple tool calls of one agent message are never separated from their
/// results: when the whole turn cannot fit, the engine splits between the
/// calls so each call and its result stay on the same side.
#[test]
fn multiple_tool_calls_stay_with_their_results() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![call_block("c1"), call_block("c2")]),
        tool_message("t1", "c1"),
        tool_message("t2", "c2"),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // The retired call c1 is covered together with its result t1, and the
    // retained call c2 stays with its result t2.
    let split = plan.split_turn_prefix.as_ref().expect("split prefix");
    assert_eq!(split.retired_prefix, vec![call_block("c1")]);
    assert_eq!(split.retired_tool_messages.len(), 1);
    assert_eq!(split.retired_tool_messages[0].id.as_str(), "t1");
    assert_eq!(
        plan.boundary,
        ContextBoundary::InsideAgent {
            message_id: MessageId::new("a1"),
            first_retained_block: ContentBlockIndex::new(1),
        }
    );
    let (_, rebuilt) = engine
        .apply_compaction(&conversation(), &history, None, &plan, "s1", &[])
        .expect("apply");
    assert!(rebuilt.items.iter().any(|item| matches!(
        item,
        ProjectionItem::AgentSlice { content, .. }
            if content == &vec![call_block("c2")]
    )));
    assert!(rebuilt.items.iter().any(|item| matches!(
        item,
        ProjectionItem::Message(MessageBlock::Tool(tool)) if tool.id.as_str() == "t2"
    )));
    assert!(!rebuilt.items.iter().any(|item| matches!(
        item,
        ProjectionItem::Message(MessageBlock::Tool(tool)) if tool.id.as_str() == "t1"
    )));
}

/// Orphan tool messages are malformed history, never guessed around.
#[test]
fn orphan_tool_message_is_rejected() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = vec![user("u1", ""), tool_message("t1", "ghost")];
    let error = engine
        .build_projection(&history, None, &[], None, None)
        .expect_err("malformed history");
    assert_eq!(error.kind, ContextErrorKind::MalformedHistory);
}

/// No tool-call/result edge crosses the chosen cut: turns are retired or
/// retained whole.
#[test]
fn no_edge_crosses_the_chosen_cut() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        agent("a2", vec![call_block("c2")]),
        tool_message("t2", "c2"),
        user("u2", ""),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    let (checkpoint, rebuilt) = engine
        .apply_compaction(&conversation(), &history, None, &plan, "s1", &[])
        .expect("apply");
    assert_eq!(checkpoint.generation, 1);
    let retained: Vec<String> = rebuilt
        .items
        .iter()
        .map(|item| match item {
            ProjectionItem::Message(message) => message_id_of(message),
            ProjectionItem::AgentSlice { .. } => "slice".to_owned(),
        })
        .collect();
    // Only the summary and the final user message remain literal: both
    // turns were retired whole, so no edge can cross the cut.
    assert_eq!(
        retained,
        vec!["conv-1-summary-1".to_owned(), "u2".to_owned()]
    );
}

/// Candidate selection is deterministic: the same plan twice.
#[test]
fn candidate_selection_is_deterministic() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        user("u2", ""),
        agent("a2", vec![call_block("c2")]),
        tool_message("t2", "c2"),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let first = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    let second = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan again");
    assert_eq!(first, second);
}

/// Message count alone does not control the cut: the token target does.
#[test]
fn message_count_alone_does_not_control_the_cut() {
    // One huge message and two tiny ones; the target (25 tokens) keeps the
    // two tiny messages and retires the huge one.
    let engine = engine(1_000, 0, 25, scripted(10, 10, 10, &[("huge", 500)]));
    let history = vec![user("huge", ""), user("small1", ""), user("small2", "")];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(
        plan.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("huge"),
        }
    );
}

// ---------------------------------------------------------------------------
// Recent suffix retention
// ---------------------------------------------------------------------------

/// The retained suffix approximates the recent-token target.
#[test]
fn retained_suffix_approximates_the_recent_target() {
    let engine = engine(1_000, 0, 25, weighted(10, 10, 10));
    let history = vec![
        user("u1", ""),
        user("u2", ""),
        user("u3", ""),
        user("u4", ""),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // Latest boundary retaining at least 25 tokens: retire one, keep three.
    assert_eq!(
        plan.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("u1"),
        }
    );
    assert_eq!(plan.planned_estimate_after, 30);
}

/// Structural safety wins over the recent-token target: a would-be cut
/// inside a turn is skipped and the whole turn is retained.
#[test]
fn structural_rule_may_force_extra_retention() {
    let engine = engine(1_000, 0, 20, scripted(10, 10, 10, &[("t1", 100)]));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        user("u2", ""),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // The naive "keep the last two messages" cut would retire a1 but keep
    // t1, separating the call from its result; the valid cut retains the
    // whole turn (130 tokens) even though that exceeds the target.
    assert_eq!(
        plan.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("u1"),
        }
    );
}

/// A token target may force retaining fewer messages when one message
/// dominates the token budget.
#[test]
fn token_target_may_retain_fewer_messages_than_recent() {
    let engine = engine(1_000, 0, 20, scripted(10, 10, 10, &[("big", 500)]));
    let history = vec![
        user("big", ""),
        user("m1", ""),
        user("m2", ""),
        user("m3", ""),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // Target 20: retire the huge message and the next one, keeping exactly
    // the two recent small messages.
    assert_eq!(
        plan.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("m1"),
        }
    );
}

// ---------------------------------------------------------------------------
// System authority
// ---------------------------------------------------------------------------

/// System messages stay literal, pinned, and never summarized; the summary
/// is a runtime inbound user message.
#[test]
fn system_messages_are_pinned_and_never_summarized() {
    let engine = engine(300, 0, 5, weighted(100, 10, 100));
    let system_block = system("sys-1", "Trusted: be concise. Byte-for-byte.");
    let history = vec![
        system_block.clone(),
        user("u1", ""),
        agent("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    assert_eq!(projection.estimated_input.input_tokens, 310);
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // The system message never enters the summary input.
    for item in &plan.newly_retired {
        assert!(!matches!(
            item,
            SummaryInputItem::Message(MessageBlock::System(_))
        ));
    }
    let (checkpoint, rebuilt) = engine
        .apply_compaction(&conversation(), &history, None, &plan, "summary", &[])
        .expect("apply");
    // The projection leads with the pinned system message, byte-for-byte.
    let ProjectionItem::Message(MessageBlock::System(pinned)) = &rebuilt.items[0] else {
        panic!("pinned system message must be the first projection item");
    };
    let MessageBlock::System(expected_system) = &system_block else {
        unreachable!("fixture system block");
    };
    assert_eq!(pinned, expected_system);
    // The summary is a runtime inbound user message, never a system block.
    assert_eq!(checkpoint.summary.source, UserSource::Runtime);
    assert_eq!(checkpoint.summary.kind, InboundKind::CompactionSummary);
    assert!(matches!(
        &rebuilt.items[1],
        ProjectionItem::Message(MessageBlock::User(user))
            if user.kind == InboundKind::CompactionSummary
    ));
}

/// A checkpoint whose `AfterMessage` boundary is absorbed by a later pinned
/// system prefix must not contribute its summary: the covered history is
/// literal again, and injecting the summary would duplicate it.
#[test]
fn absorbed_after_message_checkpoint_does_not_inject_its_summary() {
    let engine = engine(1_000, 0, 5, weighted(10, 10, 0));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![text_block("x")]),
        user("u2", ""),
        system("sys-2", "trusted"),
        user("u3", ""),
    ];
    let previous = checkpoint(
        1,
        "sum(U1/A1)",
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("a1"),
        },
        TokenMeasurement {
            input_tokens: 30,
            source: TokenMeasurementSource::Estimated,
        },
    );
    let projection = engine
        .build_projection(&history, Some(&previous), &[], None, None)
        .expect("projection");
    let ids: Vec<String> = projection
        .items
        .iter()
        .map(|item| match item {
            ProjectionItem::Message(message) => message_id_of(message),
            ProjectionItem::AgentSlice { .. } => "slice".to_owned(),
        })
        .collect();
    assert_eq!(
        ids,
        vec!["u1", "a1", "u2", "sys-2", "u3"],
        "the projection is fully literal: no summary, no duplication"
    );
    assert_eq!(
        projection.checkpoint_generation, None,
        "an absorbed checkpoint contributes no generation"
    );
}

/// The same absorption policy applies to an `InsideAgent` checkpoint whose
/// split message is pinned: no summary, no projection-only slice.
#[test]
fn absorbed_inside_agent_checkpoint_does_not_inject_its_summary() {
    let engine = engine(1_000, 0, 5, weighted(10, 10, 0));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![text_block("intro"), call_block("c1")]),
        tool_message("t1", "c1"),
        system("sys-2", "trusted"),
        user("u2", ""),
    ];
    let previous = checkpoint(
        1,
        "sum",
        ContextBoundary::InsideAgent {
            message_id: MessageId::new("a1"),
            first_retained_block: ContentBlockIndex::new(1),
        },
        TokenMeasurement {
            input_tokens: 40,
            source: TokenMeasurementSource::Estimated,
        },
    );
    let projection = engine
        .build_projection(&history, Some(&previous), &[], None, None)
        .expect("projection");
    let ids: Vec<String> = projection
        .items
        .iter()
        .map(|item| match item {
            ProjectionItem::Message(message) => message_id_of(message),
            ProjectionItem::AgentSlice { .. } => "slice".to_owned(),
        })
        .collect();
    assert_eq!(
        ids,
        vec!["u1", "a1", "t1", "sys-2", "u2"],
        "the projection is fully literal: no summary, no slice, no duplication"
    );
    assert_eq!(projection.checkpoint_generation, None);
}

/// After absorption, the next valid compaction establishes a fresh
/// checkpoint without mutating canonical history.
#[test]
fn fresh_checkpoint_is_established_after_absorption() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![text_block("x")]),
        user("u2", ""),
        system("sys-2", "trusted"),
        user("u3", ""),
        user("u4", ""),
    ];
    let previous = checkpoint(
        1,
        "sum",
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("a1"),
        },
        TokenMeasurement {
            input_tokens: 30,
            source: TokenMeasurementSource::Estimated,
        },
    );
    let projection = engine
        .build_projection(&history, Some(&previous), &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            Some(&previous),
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    let (next, rebuilt) = engine
        .apply_compaction(
            &conversation(),
            &history,
            Some(&previous),
            &plan,
            "fresh",
            &[],
        )
        .expect("fresh checkpoint");
    assert_eq!(next.generation, 2);
    let ids: Vec<String> = rebuilt
        .items
        .iter()
        .map(|item| match item {
            ProjectionItem::Message(message) => message_id_of(message),
            ProjectionItem::AgentSlice { .. } => "slice".to_owned(),
        })
        .collect();
    assert_eq!(
        ids,
        vec!["u1", "a1", "u2", "sys-2", summary_id(2).as_str()],
        "the fresh checkpoint summary replaces the absorbed one"
    );
    // Canonical history is untouched.
    assert_eq!(history.len(), 6);
}

/// End-to-end: an absorbed stored checkpoint must not leak its old summary
/// into the next summarization through the real runtime path. The stored
/// checkpoint keeps its generation lineage, but the summary source is
/// inactive: `previous_summary == None` and only the currently compactable
/// suffix is retired.
#[tokio::test]
async fn absorbed_checkpoint_never_leaks_its_summary_into_the_next_compaction() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "answer")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    // Stored generation-1 checkpoint whose boundary (after A1) is absorbed
    // by the pinned System S2 of the current canonical history.
    let previous = checkpoint(
        1,
        "old-summary",
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("a1"),
        },
        TokenMeasurement {
            input_tokens: 300,
            source: TokenMeasurementSource::Estimated,
        },
    );
    store.save(&previous).expect("store generation 1");
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "fresh-suffix-summary".to_owned(),
    )]));
    let runtime = ContextRuntime::new(
        engine(400, 0, 0, weighted(100, 10, 0)),
        summarizer.clone(),
        store.clone(),
    );
    let history = vec![
        user("u1", ""),
        agent("a1", vec![text_block("x")]),
        user("u2", ""),
        system("sys-2", "trusted"),
        user("u3", ""),
        user("u4", ""),
    ];
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", history, 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    // The summarizer received exactly one request, with no previous summary:
    // the absorbed checkpoint's old summary must never reappear transitively.
    let summary_requests = summarizer.requests();
    assert_eq!(summary_requests.len(), 1);
    assert_eq!(
        summary_requests[0].previous_summary, None,
        "an absorbed checkpoint is never an incremental summary source"
    );
    // Only the currently compactable suffix is newly retired — never the
    // pinned, checkpoint-covered history.
    let newly: Vec<String> = summary_requests[0]
        .newly_retired
        .iter()
        .map(|item| match item {
            SummaryInputItem::Message(message) => message_id_of(message),
            SummaryInputItem::AgentSlice { message_id, .. } => format!("slice:{message_id}"),
        })
        .collect();
    assert_eq!(newly, vec!["u3", "u4"]);
    // The generation lineage survives absorption: generation 2 follows
    // stored generation 1; it is never reset to 1.
    let latest = store.load(&conversation()).expect("store").expect("latest");
    assert_eq!(latest.generation, 2);
    assert_eq!(
        latest.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("u4"),
        }
    );
    // The subsequent model projection carries the pinned literal history
    // exactly once, the fresh summary exactly once, and the retained suffix
    // — never the old checkpoint summary.
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    let ids: Vec<String> = requests[0].messages.iter().map(message_id_of).collect();
    assert_eq!(
        ids,
        vec![
            "u1".to_owned(),
            "a1".to_owned(),
            "u2".to_owned(),
            "sys-2".to_owned(),
            summary_id(2).as_str().to_owned(),
        ]
    );
    let serialized = serde_json::to_string(&requests[0].messages).expect("serialize");
    assert!(serialized.contains("fresh-suffix-summary"));
    assert!(
        !serialized.contains("old-summary"),
        "the absorbed checkpoint's old summary never reaches the projection"
    );
}

/// The same end-to-end guarantee for an absorbed `InsideAgent` checkpoint.
#[tokio::test]
async fn absorbed_inside_agent_checkpoint_never_leaks_its_summary() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "answer")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let previous = checkpoint(
        1,
        "old-summary",
        ContextBoundary::InsideAgent {
            message_id: MessageId::new("a1"),
            first_retained_block: ContentBlockIndex::new(1),
        },
        TokenMeasurement {
            input_tokens: 400,
            source: TokenMeasurementSource::Estimated,
        },
    );
    store.save(&previous).expect("store generation 1");
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "fresh-suffix-summary".to_owned(),
    )]));
    let runtime = ContextRuntime::new(
        engine(500, 0, 0, weighted(100, 10, 0)),
        summarizer.clone(),
        store.clone(),
    );
    let history = vec![
        user("u1", ""),
        agent("a1", vec![text_block("intro"), call_block("c1")]),
        tool_message("t1", "c1"),
        user("u2", ""),
        system("sys-2", "trusted"),
        user("u3", ""),
    ];
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", history, 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    let summary_requests = summarizer.requests();
    assert_eq!(summary_requests.len(), 1);
    assert_eq!(
        summary_requests[0].previous_summary, None,
        "an absorbed InsideAgent checkpoint is never a summary source"
    );
    let newly: Vec<String> = summary_requests[0]
        .newly_retired
        .iter()
        .map(|item| match item {
            SummaryInputItem::Message(message) => message_id_of(message),
            SummaryInputItem::AgentSlice { message_id, .. } => format!("slice:{message_id}"),
        })
        .collect();
    assert_eq!(newly, vec!["u3"]);
    let latest = store.load(&conversation()).expect("store").expect("latest");
    assert_eq!(latest.generation, 2, "the lineage survives absorption");
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    let ids: Vec<String> = requests[0].messages.iter().map(message_id_of).collect();
    assert_eq!(
        ids,
        vec![
            "u1".to_owned(),
            "a1".to_owned(),
            "t1".to_owned(),
            "u2".to_owned(),
            "sys-2".to_owned(),
            summary_id(2).as_str().to_owned(),
        ]
    );
    let serialized = serde_json::to_string(&requests[0].messages).expect("serialize");
    assert!(serialized.contains("fresh-suffix-summary"));
    assert!(
        !serialized.contains("old-summary"),
        "the absorbed checkpoint's old summary never reaches the projection"
    );
}

/// If pinned context alone prevents fitting, compaction fails explicitly.
#[test]
fn pinned_context_alone_cannot_fit_fails_explicitly() {
    let engine = engine(120, 0, 5, scripted(10, 10, 10, &[("sys-1", 130)]));
    let history = vec![
        system("sys-1", "trusted"),
        user("u1", ""),
        agent("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let error = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect_err("cannot fit");
    assert_eq!(error.kind, ContextErrorKind::CannotFit);
}

// ---------------------------------------------------------------------------
// Split turn
// ---------------------------------------------------------------------------

/// A genuinely oversized single turn requires a cut inside the agent
/// message: the retired prefix and its tool result go to the summarizer,
/// the retained slice starts at an exact content block index.
#[test]
fn oversized_turn_splits_inside_the_agent_message() {
    let engine = engine(45, 0, 5, weighted(10, 10, 10));
    let history = vec![
        user("u1", ""),
        agent(
            "a1",
            vec![
                text_block("intro"),
                call_block("c1"),
                text_block("middle"),
                call_block("c2"),
                text_block("outro"),
            ],
        ),
        tool_message("t1", "c1"),
        tool_message("t2", "c2"),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(
        plan.boundary,
        ContextBoundary::InsideAgent {
            message_id: MessageId::new("a1"),
            first_retained_block: ContentBlockIndex::new(2),
        }
    );
    // Newly retired whole messages before the split: only the user message.
    assert_eq!(plan.newly_retired.len(), 1);
    let split = plan.split_turn_prefix.as_ref().expect("split prefix");
    assert_eq!(split.message_id.as_str(), "a1");
    assert_eq!(
        split.retired_prefix,
        vec![text_block("intro"), call_block("c1")]
    );
    assert_eq!(split.retired_tool_messages.len(), 1);
    assert_eq!(split.retired_tool_messages[0].id.as_str(), "t1");

    let (checkpoint, rebuilt) = engine
        .apply_compaction(&conversation(), &history, None, &plan, "summary", &[])
        .expect("apply");
    assert_eq!(checkpoint.generation, 1);
    // The retained slice starts at the exact content block index.
    let items = &rebuilt.items;
    assert_eq!(items.len(), 3);
    let ProjectionItem::AgentSlice {
        source_message_id,
        content,
    } = &items[1]
    else {
        panic!("the split message must project as an agent slice");
    };
    assert_eq!(source_message_id.as_str(), "a1");
    assert_eq!(
        *content,
        vec![text_block("middle"), call_block("c2"), text_block("outro")]
    );
    // The retained call's result remains; the retired call's result is
    // covered by the summary and does not appear literally.
    assert!(matches!(
        &items[2],
        ProjectionItem::Message(MessageBlock::Tool(tool)) if tool.id.as_str() == "t2"
    ));
    assert!(!items.iter().any(|item| matches!(
        item,
        ProjectionItem::Message(MessageBlock::Tool(tool)) if tool.id.as_str() == "t1"
    )));
    // Canonical history is untouched.
    assert!(history.iter().any(|message| matches!(
        message,
        MessageBlock::Agent(agent) if agent.id.as_str() == "a1" && agent.content.len() == 5
    )));
}

/// Whole-turn preference wins over split-turn compaction: when the latest
/// complete turn fits but the target asks for more than fits, the latest
/// turn is retained whole and never split.
#[test]
fn whole_turn_preference_wins_over_splitting_the_latest_turn() {
    // Turn 1: u1 (10) + a1 (10). Turn 2: u2 (10) + a2 (30). The latest turn
    // (40) fits exactly under the soft limit; the target asks for both turns
    // (50), which cannot fit. The engine must retain the latest turn whole
    // instead of splitting it.
    let engine = engine(40, 0, 50, weighted(10, 10, 0));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![text_block("x")]),
        user("u2", ""),
        agent(
            "a2",
            vec![text_block("y"), text_block("z"), text_block("w")],
        ),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(
        plan.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("a1"),
        },
        "the cut retains the latest complete turn whole"
    );
    assert!(
        plan.split_turn_prefix.is_none(),
        "the latest turn is never split merely because the target cannot be achieved"
    );
    assert_eq!(plan.planned_estimate_after, 40);
}

/// If no structurally safe split exists, a safe whole-turn cut wins even
/// when it violates the soft recent-token preference, and the giant tool
/// result is supplied intact to the summarizer.
#[test]
fn no_safe_split_falls_back_to_a_whole_turn_cut() {
    let engine = engine(60, 0, 5, scripted(10, 10, 10, &[("t1", 1_000)]));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(
        plan.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("t1"),
        }
    );
    // The giant tool result is retired intact.
    assert!(plan.newly_retired.iter().any(|item| matches!(
        item,
        SummaryInputItem::Message(MessageBlock::Tool(tool)) if tool.id.as_str() == "t1"
    )));
    assert!(plan.split_turn_prefix.is_none());
}

/// Repeated compaction after an `InsideAgent` checkpoint retires only the
/// residual slice, never the already-summarized prefix again.
#[test]
fn repeated_compaction_after_an_inside_agent_checkpoint() {
    let engine = engine(45, 0, 5, weighted(10, 10, 10));
    let history = vec![
        user("u1", ""),
        agent(
            "a1",
            vec![
                text_block("intro"),
                call_block("c1"),
                text_block("middle"),
                call_block("c2"),
                text_block("outro"),
            ],
        ),
        tool_message("t1", "c1"),
        tool_message("t2", "c2"),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let first = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("first plan");
    assert!(matches!(
        first.boundary,
        ContextBoundary::InsideAgent { .. }
    ));
    let (checkpoint1, _) = engine
        .apply_compaction(&conversation(), &history, None, &first, "s1", &[])
        .expect("first apply");
    assert_eq!(checkpoint1.generation, 1);

    // Canonical history grows; the checkpoint boundary still holds.
    let grown = vec![
        history[0].clone(),
        history[1].clone(),
        history[2].clone(),
        history[3].clone(),
        user("u2", ""),
        agent("a3", vec![call_block("c3")]),
        tool_message("t3", "c3"),
    ];
    let projection2 = engine
        .build_projection(&grown, Some(&checkpoint1), &[], None, None)
        .expect("second projection");
    let second = engine
        .plan_compaction(
            &grown,
            Some(&checkpoint1),
            &projection2,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("second plan");
    assert_eq!(
        second.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("u2"),
        }
    );
    let newly: Vec<String> = second
        .newly_retired
        .iter()
        .map(|item| match item {
            SummaryInputItem::Message(message) => message_id_of(message),
            SummaryInputItem::AgentSlice { message_id, .. } => format!("slice:{message_id}"),
        })
        .collect();
    assert_eq!(newly, vec!["slice:a1", "t2", "u2"]);
    let request = engine
        .summary_request(&grown, Some(&checkpoint1), &second)
        .expect("summary request");
    assert_eq!(request.previous_summary.as_deref(), Some("s1"));
    let (checkpoint2, _) = engine
        .apply_compaction(
            &conversation(),
            &grown,
            Some(&checkpoint1),
            &second,
            "s2",
            &[],
        )
        .expect("second apply");
    assert_eq!(checkpoint2.generation, 2);
}

// ---------------------------------------------------------------------------
// Checkpoints and incremental compaction
// ---------------------------------------------------------------------------

/// The first checkpoint: generation 1, runtime user summary, stable
/// boundary, correct token provenance, history untouched.
#[test]
fn first_checkpoint_is_committed_with_full_metadata() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        user("u2", ""),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.estimated_before.input_tokens, 310);
    assert_eq!(
        plan.estimated_before.source,
        TokenMeasurementSource::Estimated
    );
    let (checkpoint, rebuilt) = engine
        .apply_compaction(&conversation(), &history, None, &plan, "s1", &[])
        .expect("apply");
    assert_eq!(checkpoint.generation, 1);
    assert_eq!(checkpoint.conversation_id, conversation());
    assert_eq!(checkpoint.summary.id, summary_id(1));
    assert_eq!(checkpoint.summary.source, UserSource::Runtime);
    assert_eq!(checkpoint.summary.kind, InboundKind::CompactionSummary);
    assert_eq!(checkpoint.tokens_before, plan.estimated_before);
    assert_eq!(checkpoint.estimated_tokens_after, 101);
    assert!(matches!(
        checkpoint.boundary,
        ContextBoundary::AfterMessage { .. }
    ));
    assert_eq!(rebuilt.checkpoint_generation, Some(1));
    // History is untouched.
    assert_eq!(history.len(), 4);
    assert!(matches!(history[1], MessageBlock::Agent(_)));
}

/// The second compaction is incremental: it receives the previous summary
/// and only the newly retired material, never the raw prefix again.
#[test]
fn incremental_second_checkpoint_receives_only_new_material() {
    let engine = engine(220, 0, 5, weighted(100, 10, 100));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        user("u2", ""),
        agent("a2", vec![call_block("c2")]),
        tool_message("t2", "c2"),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let first = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("first plan");
    assert_eq!(
        first.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("u2"),
        }
    );
    let (checkpoint1, _) = engine
        .apply_compaction(&conversation(), &history, None, &first, "s1", &[])
        .expect("first apply");

    let projection2 = engine
        .build_projection(&history, Some(&checkpoint1), &[], None, None)
        .expect("second projection");
    let second = engine
        .plan_compaction(
            &history,
            Some(&checkpoint1),
            &projection2,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("second plan");
    let request = engine
        .summary_request(&history, Some(&checkpoint1), &second)
        .expect("summary request");
    assert_eq!(request.previous_summary.as_deref(), Some("s1"));
    // Only the newly retired material is fed: not the raw prefix covered by
    // checkpoint 1.
    let newly: Vec<String> = request.newly_retired.iter().map(newly_retired_id).collect();
    assert!(!newly.contains(&"u1".to_owned()));
    assert!(!newly.contains(&"a1".to_owned()));
    assert!(!newly.contains(&"t1".to_owned()));
    assert!(!newly.contains(&"u2".to_owned()));
    assert!(newly.contains(&"a2".to_owned()));
    assert!(newly.contains(&"t2".to_owned()));
    let (checkpoint2, _) = engine
        .apply_compaction(
            &conversation(),
            &history,
            Some(&checkpoint1),
            &second,
            "s2",
            &[],
        )
        .expect("second apply");
    assert_eq!(checkpoint2.generation, 2);
    assert_eq!(
        checkpoint2.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("t2"),
        }
    );
}

/// A summary at least as large as the replaced context makes no progress:
/// no checkpoint, explicit error.
#[test]
fn no_progress_compaction_is_rejected() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = vec![user("u1", "")];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // A summary text of 400 bytes estimates 101 tokens >= the 100 before.
    let error = engine
        .apply_compaction(
            &conversation(),
            &history,
            None,
            &plan,
            &"x".repeat(400),
            &[],
        )
        .expect_err("no progress");
    assert_eq!(error.kind, ContextErrorKind::NoProgress);
}

/// The anti-loop progress rule never compares a provider-reported
/// before-count against an estimated after-count: both sides of the
/// comparison are deterministic estimates of the actual projection content.
/// A provider-reported number far above the deterministic estimate must not
/// mask an estimate that grew.
#[test]
fn progress_rule_rejects_growth_even_when_provider_reported_before_is_larger() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = vec![user("u1", ""), agent("a1", vec![text_block("x")])];
    // Provider-reported before = 1000; the deterministic estimate of the
    // same projection is 20.
    let plain_projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: plain_projection.fingerprint(),
        input_tokens: 1_000,
    };
    let projection = engine
        .build_projection(&history, None, &[], Some(&observed), None)
        .expect("provider-reported projection");
    assert_eq!(
        projection.estimated_input.source,
        TokenMeasurementSource::ProviderReported
    );
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(
        plan.estimated_before.input_tokens, 1_000,
        "the provider-reported measurement is preserved as metadata"
    );
    assert_eq!(
        plan.estimated_before_tokens, 20,
        "the progress comparison uses the deterministic estimate"
    );
    // The after estimate (31) grew relative to the deterministic before
    // (20): rejected, even though it is far below the provider-reported 1000.
    let error = engine
        .apply_compaction(
            &conversation(),
            &history,
            None,
            &plan,
            &"x".repeat(120),
            &[],
        )
        .expect_err("no progress");
    assert_eq!(error.kind, ContextErrorKind::NoProgress);
}

/// The reverse direction: a provider-reported before-count below the
/// deterministic estimate must not reject a compaction whose estimated
/// after-count decreased relative to the deterministic before.
#[test]
fn progress_rule_accepts_decrease_even_when_provider_reported_before_is_smaller() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = vec![
        user("u1", ""),
        user("u2", ""),
        user("u3", ""),
        user("u4", ""),
        user("u5", ""),
        user("u6", ""),
    ];
    // Provider-reported before = 50; the deterministic estimate of the same
    // projection is 60.
    let plain_projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: plain_projection.fingerprint(),
        input_tokens: 50,
    };
    let projection = engine
        .build_projection(&history, None, &[], Some(&observed), None)
        .expect("provider-reported projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.estimated_before_tokens, 60);
    // The after estimate (50) decreased from the deterministic before (60)
    // but is above the provider-reported 50-boundary of this test's before
    // measurement: progress must be accepted. A 200-byte summary weighs
    // exactly 50 tokens under the corrected ceiling division.
    let (checkpoint, _) = engine
        .apply_compaction(
            &conversation(),
            &history,
            None,
            &plan,
            &"x".repeat(200),
            &[],
        )
        .expect("progress accepted");
    assert_eq!(checkpoint.generation, 1);
    assert_eq!(
        checkpoint.tokens_before,
        TokenMeasurement {
            input_tokens: 50,
            source: TokenMeasurementSource::ProviderReported,
        },
        "the provider-reported measurement is preserved as checkpoint metadata"
    );
    assert_eq!(checkpoint.estimated_tokens_after, 50);
}

/// Empty and whitespace-only summaries are rejected at the application
/// boundary: no summarizer can erase history through an empty summary.
#[test]
fn empty_and_whitespace_summaries_are_rejected() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = vec![user("u1", ""), user("u2", "")];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    for bad in ["", "   ", "\n\t "] {
        let error = engine
            .apply_compaction(&conversation(), &history, None, &plan, bad, &[])
            .expect_err("empty summary must be rejected");
        assert_eq!(error.kind, ContextErrorKind::SummaryFailed);
    }
}

// ---------------------------------------------------------------------------
// Continuation constraint
// ---------------------------------------------------------------------------

/// The continuation constraint retires the continuation-owning turn
/// completely; the owning agent message is never split.
#[test]
fn continuation_constraint_covers_the_owning_turn_completely() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints {
                must_cover_through: Some(&MessageId::new("a1")),
                fresh_inbound: None,
            },
        )
        .expect("plan");
    assert_eq!(
        plan.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("t1"),
        }
    );
    // The continuation-owning agent message and its complete tool-result
    // portion are both retired into the summary input.
    let retired_ids: Vec<String> = plan.newly_retired.iter().map(newly_retired_id).collect();
    assert!(retired_ids.contains(&"a1".to_owned()));
    assert!(retired_ids.contains(&"t1".to_owned()));
    assert!(plan.split_turn_prefix.is_none());
}

/// A continuation-owning oversized turn is retired whole, never split.
#[test]
fn continuation_owner_is_never_split() {
    let engine = engine(60, 0, 5, weighted(10, 10, 10));
    let history = vec![
        user("u1", ""),
        agent(
            "a1",
            vec![
                text_block("intro"),
                call_block("c1"),
                text_block("middle"),
                call_block("c2"),
                text_block("outro"),
            ],
        ),
        tool_message("t1", "c1"),
        tool_message("t2", "c2"),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    // Without the constraint this turn would split (see the split test);
    // with the constraint it must be retired whole.
    let plan = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints {
                must_cover_through: Some(&MessageId::new("a1")),
                fresh_inbound: None,
            },
        )
        .expect("plan");
    assert_eq!(
        plan.boundary,
        ContextBoundary::AfterMessage {
            message_id: MessageId::new("t2"),
        }
    );
    assert!(plan.split_turn_prefix.is_none());
}

/// When a later `SystemMessage` pins the continuation-owning turn, the
/// constraint is unsatisfiable: compaction cannot retire the owner, so the
/// plan fails explicitly instead of clearing the continuation while leaving
/// its boundary literal.
#[test]
fn pinned_continuation_owner_makes_the_constraint_unsatisfiable() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = vec![
        user("u1", ""),
        agent("a1", vec![text_block("x")]),
        system("sys-2", "trusted"),
        user("u2", ""),
    ];
    let projection = engine
        .build_projection(&history, None, &[], None, None)
        .expect("projection");
    let error = engine
        .plan_compaction(
            &history,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints {
                must_cover_through: Some(&MessageId::new("a1")),
                fresh_inbound: None,
            },
        )
        .expect_err("pinned continuation owner cannot be retired");
    assert_eq!(error.kind, ContextErrorKind::NoProgress);
    assert!(
        error.message.contains("pinned"),
        "the error explains the pinned constraint: {}",
        error.message
    );
    // The same history without the pinning system message satisfies the
    // constraint: the check is specific to the pinned prefix.
    let unpinned = vec![
        user("u1", ""),
        agent("a1", vec![text_block("x")]),
        user("u2", ""),
    ];
    let projection = engine
        .build_projection(&unpinned, None, &[], None, None)
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &unpinned,
            None,
            &projection,
            &[],
            0,
            &rustx::context::CompactionConstraints {
                must_cover_through: Some(&MessageId::new("a1")),
                fresh_inbound: None,
            },
        )
        .expect("unpinned continuation owner is retired");
    assert!(matches!(
        plan.boundary,
        ContextBoundary::AfterMessage { .. }
    ));
    assert!(plan.split_turn_prefix.is_none());
}

// ---------------------------------------------------------------------------
// Agent loop integration: proactive compaction
// ---------------------------------------------------------------------------

/// A long history compacts proactively before the next model request: the
/// trace order is `TurnStarted`, `CompactionStarted`, `CompactionCompleted`,
/// `ModelRequestStarted`; the request receives the projection, and the
/// committed result remains canonical history.
#[tokio::test]
#[allow(clippy::too_many_lines)] // the full expected trace is asserted verbatim
async fn proactive_compaction_before_the_next_turn() {
    let scripted = scripted_call();
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &scripted)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &scripted)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &scripted)[2].clone()),
            FakeStep::Emit(done_with_usage(ModelFinishReason::ToolCalls, 15)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "answer")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = tool_registry_with_alpha();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer =
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
    let runtime = runtime_with(200, 0, 5, weighted(100, 10, 0), summarizer, store.clone());
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

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
        RuntimeEvent::ToolCallStarted {
            message_id: agent_message_id(1),
            block_index: ContentBlockIndex::new(0),
            call: call_start(),
        },
        RuntimeEvent::ToolCallArgumentsDelta {
            message_id: agent_message_id(1),
            block_index: ContentBlockIndex::new(0),
            call_id: ToolCallId::new("call-1"),
            arguments_delta: "{}".to_owned(),
        },
        RuntimeEvent::ToolCallCompleted {
            message_id: agent_message_id(1),
            block_index: ContentBlockIndex::new(0),
            call: call_done(),
        },
        RuntimeEvent::ModelRequestCompleted {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: Some(ModelUsage {
                input_tokens: 15,
                output_tokens: 4,
                total_tokens: 19,
                details: None,
            }),
        },
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-alpha"),
        },
        RuntimeEvent::ToolExecutionCompleted {
            tool_call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-alpha"),
            result: success_result("ok"),
        },
        RuntimeEvent::TurnCompleted,
        RuntimeEvent::TurnStarted,
        RuntimeEvent::CompactionStarted,
        RuntimeEvent::CompactionCompleted,
        RuntimeEvent::ModelRequestStarted {
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::AgentMessageStarted {
            message_id: agent_message_id(2),
        },
        RuntimeEvent::AgentTextDelta {
            message_id: agent_message_id(2),
            block_index: ContentBlockIndex::new(0),
            delta: "answer".to_owned(),
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
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );

    // The second request receives the compiled projection, not a mutated
    // history: summary + retained suffix.
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].messages.len(),
        1,
        "first request carries the initial history"
    );
    assert_eq!(requests[1].messages.len(), 3);
    let MessageBlock::User(summary) = &requests[1].messages[0] else {
        panic!("first projected message must be the summary");
    };
    assert_eq!(summary.id, summary_id(1));
    assert_eq!(summary.source, UserSource::Runtime);
    assert_eq!(summary.kind, InboundKind::CompactionSummary);
    assert!(matches!(
        &requests[1].messages[1],
        MessageBlock::Agent(agent) if agent.id.as_str() == "attempt-1-agent-1"
    ));
    assert!(matches!(
        &requests[1].messages[2],
        MessageBlock::Tool(tool) if tool.id.as_str() == "attempt-1-tool-1-call-1"
    ));

    // Committed canonical history stays canonical: no summary ever appears.
    assert_eq!(result.messages.len(), 4);
    assert_eq!(result.messages[0].clone(), user("msg-user-1", "hi"));
    assert!(matches!(
        &result.messages[1],
        MessageBlock::Agent(agent) if agent.id.as_str() == "attempt-1-agent-1"
    ));
    assert!(matches!(
        &result.messages[2],
        MessageBlock::Tool(tool) if tool.id.as_str() == "attempt-1-tool-1-call-1"
    ));
    assert!(matches!(
        &result.messages[3],
        MessageBlock::Agent(agent) if agent.id.as_str() == "attempt-1-agent-2"
    ));
    assert!(!result.messages.iter().any(|message| matches!(
        message,
        MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary
    )));

    // The checkpoint was committed with generation 1.
    let checkpoint = store
        .load(&conversation())
        .expect("store")
        .expect("checkpoint");
    assert_eq!(checkpoint.generation, 1);
    assert_eq!(
        checkpoint.tokens_before.source,
        TokenMeasurementSource::Estimated
    );
}

/// Below the threshold, the loop never compacts and preserves M3 behavior.
#[tokio::test]
async fn below_threshold_runs_without_compaction() {
    let scripted = scripted_call();
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &scripted)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &scripted)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &scripted)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = tool_registry_with_alpha();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Return("s".to_owned())]);
    let runtime = runtime_with(
        10_000,
        0,
        5,
        weighted(100, 10, 0),
        summarizer,
        store.clone(),
    );
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted)),
        "no compaction below the threshold"
    );
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert!(store.load(&conversation()).expect("store").is_none());
}

// ---------------------------------------------------------------------------
// Overflow compact-and-retry
// ---------------------------------------------------------------------------

/// A context overflow compacts once and retries with the smaller projection
/// and a cleared continuation; the retry succeeds and the attempt emits
/// exactly one terminal event.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn overflow_compact_and_retry_succeeds() {
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "provisional")),
            FakeStep::Emit(overflow_event()),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "retry ok")),
            FakeStep::Emit(done_with_usage(ModelFinishReason::Stop, 4)),
        ],
    ]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer =
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer, store.clone());
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

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
            block_index: ContentBlockIndex::new(0),
            delta: "provisional".to_owned(),
        },
        RuntimeEvent::ModelRequestFailed {
            error: overflow_error(),
        },
        RuntimeEvent::CompactionStarted,
        RuntimeEvent::CompactionCompleted,
        RuntimeEvent::ModelRetryScheduled {
            attempt_number: 1,
            retry_delay_ms: None,
        },
        RuntimeEvent::ModelRequestStarted {
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::AgentMessageStarted {
            message_id: retry_message_id(1),
        },
        RuntimeEvent::AgentTextDelta {
            message_id: retry_message_id(1),
            block_index: ContentBlockIndex::new(0),
            delta: "retry ok".to_owned(),
        },
        RuntimeEvent::ModelRequestCompleted {
            finish_reason: ModelFinishReason::Stop,
            usage: Some(ModelUsage {
                input_tokens: 4,
                output_tokens: 4,
                total_tokens: 8,
                details: None,
            }),
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
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );

    // The retry request uses the smaller projection with the checkpoint
    // summary and no continuation.
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        &requests[1].messages[0],
        MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary
    ));
    assert_eq!(requests[1].messages.len(), 1);
    assert_eq!(requests[1].continuation, None);
    // Only the successful invocation is committed.
    assert_eq!(result.messages.len(), 2);
    assert!(matches!(
        &result.messages[1],
        MessageBlock::Agent(agent) if agent.id == retry_message_id(1)
    ));
}

/// A second overflow after the retry settles the attempt with the second
/// overflow; no second compaction and no third request occur.
#[tokio::test]
async fn overflow_retry_exhausted_after_one_retry() {
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "first")),
            FakeStep::Emit(overflow_event()),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "second")),
            FakeStep::Emit(overflow_event()),
        ],
    ]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer =
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer, store.clone());
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

    let started = result
        .events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
        .count();
    assert_eq!(started, 2, "exactly two provider requests");
    let retries = result
        .events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
        .count();
    assert_eq!(retries, 1, "exactly one retry");
    let compactions = result
        .events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::CompactionStarted))
        .count();
    assert_eq!(compactions, 1, "no second overflow compaction");

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Failed {
            error: AttemptFailure::Model {
                error: overflow_error(),
            },
        },
    );
    assert!(matches!(
        result.events.last(),
        Some(RuntimeEvent::AttemptFailed {
            error: AttemptFailure::Model { .. },
            ..
        })
    ));
}

/// An overflow retry replaces the complete failed invocation: the retry's
/// output commits under the retry identity, and provisional content from
/// the failed request never enters the committed message.
#[tokio::test]
async fn overflow_retry_never_commits_provisional_failed_content() {
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "PROVISIONAL")),
            FakeStep::Emit(overflow_event()),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "RETRY")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer =
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer, store.clone());
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert_eq!(
        result.messages.len(),
        2,
        "input + one committed agent message"
    );
    let MessageBlock::Agent(agent) = &result.messages[1] else {
        panic!("the committed message must be the retry agent message");
    };
    assert_eq!(
        agent.id,
        retry_message_id(1),
        "the committed message carries the retry identity"
    );
    let texts: Vec<String> = agent
        .content
        .iter()
        .filter_map(|block| match block {
            AgentContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["RETRY".to_owned()], "exactly the retry output");
    let serialized = serde_json::to_string(&result.messages).expect("serialize messages");
    assert!(
        !serialized.contains("PROVISIONAL"),
        "the failed request's provisional content must never be committed"
    );
}

/// The failed overflow request's complete provisional tool call is never
/// committed and never executed: the retry replaces the whole invocation.
#[tokio::test]
async fn overflow_retry_never_commits_or_executes_failed_tool_calls() {
    let scripted = scripted_call();
    let mut first = vec![FakeStep::Emit(started())];
    first.extend(
        tool_call_events(0, &scripted)
            .into_iter()
            .map(FakeStep::Emit),
    );
    first.push(FakeStep::Emit(overflow_event()));
    let model = FakeModel::new(vec![
        first,
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "plain answer")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = tool_registry_with_alpha();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer =
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer, store.clone());
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert!(
        result.events.iter().all(|event| {
            !matches!(
                event,
                RuntimeEvent::ToolExecutionStarted { .. }
                    | RuntimeEvent::ToolExecutionCompleted { .. }
            )
        }),
        "the failed request's tool call is never executed"
    );
    assert!(
        result
            .messages
            .iter()
            .all(|message| !matches!(message, MessageBlock::Tool(_))),
        "no tool message is committed for the failed request's call"
    );
    let MessageBlock::Agent(agent) = &result.messages[1] else {
        panic!("the committed message must be the retry agent message");
    };
    assert_eq!(agent.id, retry_message_id(1));
    assert_eq!(agent.content.len(), 1, "only the retry text block");
}

/// The overflow retry budget is genuinely per model turn: both turns are
/// entitled to their own single retry, and the budget never persists across
/// turns.
#[tokio::test]
async fn overflow_retry_budget_is_per_model_turn() {
    let scripted = scripted_call();
    let model = FakeModel::new(vec![
        // Turn 1: overflow, then its own retry → ToolCalls.
        vec![FakeStep::Emit(started()), FakeStep::Emit(overflow_event())],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &scripted)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &scripted)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &scripted)[2].clone()),
            FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
        ],
        // Turn 2: overflow, then its own retry → Stop.
        vec![FakeStep::Emit(started()), FakeStep::Emit(overflow_event())],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "done")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = tool_registry_with_alpha();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer = FakeContextSummarizer::new(vec![
        FakeSummaryStep::Return("summary-1".to_owned()),
        FakeSummaryStep::Return("summary-2".to_owned()),
    ]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer, store.clone());
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

    let requests = model.requests();
    assert_eq!(
        requests.len(),
        4,
        "two invocations per turn: request + retry"
    );
    let retries = result
        .events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
        .count();
    assert_eq!(retries, 2, "each turn gets exactly one retry");
    let compactions = result
        .events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::CompactionStarted))
        .count();
    assert_eq!(compactions, 2, "one compaction per overflow");
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    // The committed conversation holds both turns, each with the retry
    // identity of its own turn.
    assert_eq!(result.messages.len(), 4, "input + agent1 + tool + agent2");
    assert!(matches!(
        &result.messages[1],
        MessageBlock::Agent(agent) if agent.id == retry_message_id(1)
    ));
    assert!(matches!(
        &result.messages[3],
        MessageBlock::Agent(agent) if agent.id == retry_message_id(2)
    ));
}

/// An invalid (empty or whitespace-only) summary from a custom/fake
/// summarizer fails the compaction: no checkpoint is saved and no overflow
/// retry follows.
#[tokio::test]
async fn invalid_summary_fails_without_checkpoint_or_retry() {
    for bad_summary in ["", "   "] {
        let model = FakeModel::new(vec![vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(overflow_event()),
        ]]);
        let tools = ToolRegistry::new();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let store = InMemoryCheckpointStore::new().shared();
        let summarizer =
            FakeContextSummarizer::new(vec![FakeSummaryStep::Return(bad_summary.to_owned())]);
        let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer, store.clone());
        let tool_runtime = common::tool_runtime("conv-1");
        let result = AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0),
            &model,
            &tools,
            &cancellation,
            runtime,
            &tool_runtime,
        )
        .run()
        .await;

        assert_single_terminal(&result.events);
        assert_outcome(
            &result,
            &AttemptOutcome::Failed {
                error: AttemptFailure::Model {
                    error: overflow_error(),
                },
            },
        );
        assert!(
            result
                .events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. })),
            "the invalid summary is a compaction failure"
        );
        assert!(
            result
                .events
                .iter()
                .all(|event| !matches!(event, RuntimeEvent::CompactionCompleted)),
            "no checkpoint may be committed"
        );
        assert!(
            result
                .events
                .iter()
                .all(|event| !matches!(event, RuntimeEvent::ModelRetryScheduled { .. })),
            "no overflow retry may follow an invalid summary"
        );
        assert!(
            store.load(&conversation()).expect("store").is_none(),
            "no checkpoint is saved after an invalid summary"
        );
        assert_eq!(
            model.requests().len(),
            1,
            "exactly the overflowing request, no retry request"
        );
    }
}

/// A compaction failure after an overflow preserves the original normalized
/// overflow as the final model failure.
#[tokio::test]
async fn compaction_failure_after_overflow_preserves_the_overflow() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "provisional")),
        FakeStep::Emit(overflow_event()),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Fail(ContextError::new(
        ContextErrorKind::SummaryFailed,
        "summary generation refused",
    ))]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer, store.clone());
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

    let expected_tail = vec![
        RuntimeEvent::ModelRequestFailed {
            error: overflow_error(),
        },
        RuntimeEvent::CompactionStarted,
        RuntimeEvent::CompactionFailed {
            error: "summary generation refused".to_owned(),
        },
        RuntimeEvent::AttemptFailed {
            attempt_id: AttemptId::new("attempt-1"),
            error: AttemptFailure::Model {
                error: overflow_error(),
            },
        },
    ];
    assert_eq!(
        &result.events[result.events.len() - expected_tail.len()..],
        &expected_tail
    );
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Failed {
            error: AttemptFailure::Model {
                error: overflow_error(),
            },
        },
    );
    assert!(
        store.load(&conversation()).expect("store").is_none(),
        "no checkpoint may be saved after a failed compaction"
    );
}

// ---------------------------------------------------------------------------
// Context failure classification (preparation vs compaction)
// ---------------------------------------------------------------------------

/// A fixed deterministic UTC clock for status composition in these
/// classification regressions.
#[derive(Debug, Clone, Copy)]
struct FixedClock(chrono::DateTime<chrono::Utc>);

impl rustx::context::AgentStatusClock for FixedClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.0
    }
}

fn fixed_time() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
        .expect("fixed timestamp")
        .with_timezone(&chrono::Utc)
}

/// A scripted status provider that always fails.
struct FailingStatusProvider;

impl rustx::context::AgentStatusSectionProvider for FailingStatusProvider {
    fn section_id(&self) -> rustx::context::AgentStatusSectionId {
        rustx::context::AgentStatusSectionId::new("broken")
    }

    fn section(
        &self,
        _context: &rustx::context::AgentStatusRenderContext,
    ) -> Result<Option<Vec<rustx::context::AgentStatusFact>>, ContextError> {
        Err(ContextError::new(
            ContextErrorKind::StatusFailed,
            "test provider exploded",
        ))
    }
}

/// A fresh-inbound request: the first turn carries a pending fresh inbound
/// turn, so Agent Status composition is mandatory.
fn fresh_request(attempt: &str, initial_messages: Vec<MessageBlock>) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id: conversation(),
        attempt_id: AttemptId::new(attempt),
        initial_messages,
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::FreshInbound(
            rustx::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("msg-inbound-1")])
                .expect("valid fresh turn"),
        ),
        timezone: None,
        model: "fake-model".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 0,
    }
}

/// A timestamped ordinary inbound user message.
fn fresh_user(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: Some(fixed_time()),
    })
}

/// A deterministic failing Agent Status provider is a context **preparation**
/// failure, never a compaction failure: no provider request is sent, no
/// `CompactionStarted` is emitted, the terminal is exactly one
/// `AttemptFailed`, and the error classifies as
/// `Runtime(ContextPreparationFailed { .. })`.
#[tokio::test]
async fn failing_status_provider_is_preparation_failure_not_compaction() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "ok")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let mut composer = rustx::context::AgentStatusComposer::new(Arc::new(FixedClock(fixed_time())));
    composer
        .register(Arc::new(FailingStatusProvider))
        .expect("register");
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        fresh_request("attempt-1", vec![fresh_user("msg-inbound-1", "deploy it")]),
        &model,
        &tools,
        &cancellation,
        rustx::context::ContextRuntime::with_status_composer(
            engine(10_000_000, 0, 0, weighted(10, 10, 10)),
            Arc::new(FakeContextSummarizer::new(Vec::new())),
            store,
            composer,
        ),
        &tool_runtime,
    )
    .run()
    .await;

    assert_eq!(
        model.requests().len(),
        0,
        "no provider request may be sent when status composition fails"
    );
    let terminals: Vec<&RuntimeEvent> = result
        .events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::AttemptFailed { .. }))
        .collect();
    assert_eq!(terminals.len(), 1, "exactly one terminal event");
    assert_eq!(
        result.events.last(),
        Some(terminals[0]),
        "the terminal event is last"
    );
    assert!(
        result
            .events
            .iter()
            .all(|event| !matches!(event, RuntimeEvent::CompactionStarted)),
        "no compaction pipeline may start for a preparation failure"
    );
    let RuntimeEvent::AttemptFailed { error, .. } = terminals[0] else {
        unreachable!("terminal matched above");
    };
    let AttemptFailure::Runtime { error } = error else {
        panic!("the terminal must be a runtime failure, got {error:?}");
    };
    assert!(
        matches!(
            error,
            rustx::runtime::types::RuntimeError::ContextPreparationFailed { .. }
        ),
        "a status provider failure classifies as context preparation failure"
    );
    assert!(
        !matches!(
            error,
            rustx::runtime::types::RuntimeError::ContextCompactionFailed { .. }
        ),
        "a status provider failure must never be mislabeled as a compaction failure"
    );
    if let rustx::runtime::types::RuntimeError::ContextPreparationFailed { message } = error {
        assert!(
            message.contains("broken"),
            "the diagnostic names the failing provider: {message}"
        );
    }
}

/// An actual proactive compaction pipeline failure still classifies as
/// `Runtime(ContextCompactionFailed { .. })`, distinct from a preparation
/// failure: no provider request follows, but the compaction pipeline
/// genuinely started and failed.
#[tokio::test]
async fn proactive_compaction_failure_is_context_compaction_failed() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "ok")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Fail(ContextError::new(
        ContextErrorKind::SummaryFailed,
        "summary generation refused",
    ))]);
    let initial = vec![
        user("msg-old-1", "old"),
        user("msg-old-2", "older"),
        fresh_user("msg-inbound-1", "fresh instruction"),
    ];
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        fresh_request("attempt-1", initial),
        &model,
        &tools,
        &cancellation,
        runtime_with(250, 0, 0, weighted(100, 10, 0), summarizer, store.clone()),
        &tool_runtime,
    )
    .run()
    .await;

    assert_eq!(
        model.requests().len(),
        0,
        "no provider request follows a failed proactive compaction"
    );
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted)),
        "a proactive compaction pipeline must actually start"
    );
    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. })),
        "the compaction failure event carries the diagnostic"
    );
    let RuntimeEvent::AttemptFailed { error, .. } = result.events.last().expect("terminal") else {
        panic!("the terminal must be an AttemptFailed");
    };
    let AttemptFailure::Runtime { error } = error else {
        panic!("the terminal must be a runtime failure");
    };
    assert!(
        matches!(
            error,
            rustx::runtime::types::RuntimeError::ContextCompactionFailed { .. }
        ),
        "an actual compaction pipeline failure keeps the compaction classification"
    );
    assert!(
        !matches!(
            error,
            rustx::runtime::types::RuntimeError::ContextPreparationFailed { .. }
        ),
        "an actual compaction failure is not a preparation failure"
    );
    assert!(
        store.load(&conversation()).expect("store").is_none(),
        "no checkpoint may be saved after a failed compaction"
    );
}

/// A no-progress compaction (summary not smaller than the replaced
/// context) fails explicitly: no checkpoint, no retry, no loop, one
/// terminal event.
#[tokio::test]
async fn no_progress_compaction_fails_without_retry() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "provisional")),
        FakeStep::Emit(overflow_event()),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    // 400 bytes estimate 101 tokens >= the 100-token replaced context.
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Return("x".repeat(400))]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer, store.clone());
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

    assert!(
        result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. }))
    );
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. })),
        "no overflow retry after a failed compaction"
    );
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionCompleted)),
        "no compaction completion without progress"
    );
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Failed {
            error: AttemptFailure::Model {
                error: overflow_error(),
            },
        },
    );
    assert!(store.load(&conversation()).expect("store").is_none());
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

/// Cancellation before proactive compaction begins: no `CompactionStarted`,
/// no summary, no checkpoint, no retry.
#[tokio::test]
async fn cancel_before_proactive_compaction() {
    let scripted = scripted_call();
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(tool_call_events(0, &scripted)[0].clone()),
        FakeStep::Emit(tool_call_events(0, &scripted)[1].clone()),
        FakeStep::Emit(tool_call_events(0, &scripted)[2].clone()),
        FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
    ]]);
    let (tool, _release) =
        FakeTool::parking(common::tool("alpha", "tool-alpha"), success_result("ok"));
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Return("s".to_owned())]);
    // Window 200: turn 1 fits (100 tokens), but after turn 1 the history
    // (210 tokens) would require proactive compaction at turn 2 — which
    // never starts because cancellation settles the attempt first.
    let runtime = runtime_with(200, 0, 5, weighted(100, 10, 0), summarizer, store.clone());
    let tool_runtime = common::tool_runtime("conv-1");
    let execution = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    );
    // Wait until the model stream was fully consumed, then cancel while the
    // tool is parked: the loop settles cancelled before any later turn could
    // compact.
    let controller_cancellation = cancellation.clone();
    let mut emitted = model.emitted();
    let controller = tokio::spawn(async move {
        loop {
            if *emitted.borrow() >= 5 {
                break;
            }
            if emitted.changed().await.is_err() {
                break;
            }
        }
        controller_cancellation.cancel();
    });
    let result = execution.run().await;
    controller.await.expect("controller task");
    assert_single_terminal(&result.events);
    assert!(matches!(
        result.events.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted)),
        "no compaction may begin after cancellation"
    );
    assert!(store.load(&conversation()).expect("store").is_none());
}

/// Cancellation while the summary is parked: the pending summary future is
/// dropped, no completion, no failure, no retry, and no checkpoint.
#[tokio::test]
async fn cancel_while_summary_generation_is_pending() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "provisional")),
        FakeStep::Emit(overflow_event()),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::ParkUntilCancelled]);
    let parked = summarizer.parked();
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer, store.clone());
    let tool_runtime = common::tool_runtime("conv-1");
    let execution = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    );
    // Wait until the summarizer parked, then cancel.
    let mut parked = parked;
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        parked
            .wait_for(|value| *value)
            .await
            .expect("summarizer parked");
        controller_cancellation.cancel();
    });
    let result = execution.run().await;
    controller.await.expect("controller task");
    assert_single_terminal(&result.events);
    assert!(matches!(
        result.events.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
    assert!(
        !result.events.iter().any(|event| matches!(
            event,
            RuntimeEvent::CompactionCompleted | RuntimeEvent::CompactionFailed { .. }
        )),
        "no post-cancel compaction facts"
    );
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. })),
        "no retry after cancellation"
    );
    let started_requests = result
        .events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
        .count();
    assert_eq!(
        started_requests, 1,
        "no new model request after cancellation"
    );
    assert!(store.load(&conversation()).expect("store").is_none());
}

// ---------------------------------------------------------------------------
// Continuation policy
// ---------------------------------------------------------------------------

async fn run_continuation_case(
    emit_continuation: bool,
    state: ProviderContinuationState,
    window: u64,
    summarizer: FakeContextSummarizer,
) -> (AgentExecutionResult, Vec<ModelRequest>) {
    let scripted = scripted_call();
    let call_block_index = u32::from(emit_continuation);
    let mut turn1 = vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(tool_call_events(call_block_index, &scripted)[0].clone()),
        FakeStep::Emit(tool_call_events(call_block_index, &scripted)[1].clone()),
        FakeStep::Emit(tool_call_events(call_block_index, &scripted)[2].clone()),
        FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
    ];
    if emit_continuation {
        turn1.insert(
            1,
            FakeStep::Emit(ModelEvent::ContinuationState {
                block_index: ContentBlockIndex::new(0),
                state,
            }),
        );
    }
    let model = FakeModel::new(vec![
        turn1,
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "final")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = tool_registry_with_alpha();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let runtime = runtime_with(
        window,
        0,
        5,
        weighted(100, 10, 0),
        summarizer,
        InMemoryCheckpointStore::new().shared(),
    );
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;
    let requests = model.requests();
    (result, requests)
}

fn anthropic_state() -> ProviderContinuationState {
    ProviderContinuationState::Anthropic(AnthropicContinuation {
        opaque: serde_json::json!({"signature": "sig-1"}),
    })
}

fn stored_state() -> ProviderContinuationState {
    ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stored {
        previous_response_id: "resp_abc".to_owned(),
    })
}

fn stateless_state() -> ProviderContinuationState {
    ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {
        items: vec![serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "encrypted_content": "opaque"
        })],
    })
}

/// Without compaction the pending continuation is preserved exactly, for
/// every opaque provider shape.
#[tokio::test]
async fn continuation_is_preserved_without_compaction() {
    for state in [anthropic_state(), stored_state(), stateless_state()] {
        let (result, requests) = run_continuation_case(
            true,
            state.clone(),
            10_000,
            FakeContextSummarizer::new(vec![FakeSummaryStep::Return("s".to_owned())]),
        )
        .await;
        assert_single_terminal(&result.events);
        assert!(
            !result
                .events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::CompactionStarted)),
            "no compaction below the threshold"
        );
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].continuation,
            Some(state),
            "the opaque continuation must be preserved byte-for-byte"
        );
    }
}

/// Successful compaction invalidates the continuation for every opaque
/// provider shape, and the continuation-owning turn is retired completely.
#[tokio::test]
async fn continuation_is_invalidated_by_compaction() {
    for state in [anthropic_state(), stored_state(), stateless_state()] {
        let summarizer =
            FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
        let (result, requests) = run_continuation_case(true, state.clone(), 200, summarizer).await;
        assert_single_terminal(&result.events);
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].continuation, None,
            "a changed projection must never pair with the old opaque continuation"
        );
        // The continuation-owning turn was fully retired into the summary:
        // the projected request contains no literal part of it.
        assert!(requests[1].messages.iter().any(|message| matches!(
            message,
            MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary
        )));
        assert!(
            !requests[1].messages.iter().any(|message| matches!(
                message,
                MessageBlock::Agent(agent) if agent.id == agent_message_id(1)
            )),
            "the continuation-owning agent message may not remain literal"
        );
    }
}

/// Continuation state is never fabricated: a turn without reported
/// continuation state propagates `None` into the next request.
#[tokio::test]
async fn no_continuation_is_fabricated() {
    let (result, requests) = run_continuation_case(
        false,
        anthropic_state(),
        10_000,
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("s".to_owned())]),
    )
    .await;
    assert_single_terminal(&result.events);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].continuation, None);
    assert_eq!(requests[1].continuation, None);
}

// ---------------------------------------------------------------------------
// Adapter-backed summarizer
// ---------------------------------------------------------------------------

/// The model-backed summarizer issues a canonical one-off request with no
/// tools, no continuation, the configured model/protocol/reasoning/output
/// budget, and deterministic input.
#[tokio::test]
async fn model_backed_summarizer_issues_a_canonical_request() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "summary ")),
        FakeStep::Emit(text_delta(0, "text")),
        FakeStep::Emit(done_with_usage(ModelFinishReason::Stop, 9)),
    ]]);
    let summarizer = ModelBackedSummarizer::new(
        &model,
        SummaryModelConfig {
            model: "fake-model".to_owned(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            reasoning: ReasoningEffort::High,
            max_output_tokens: 128,
        },
    );
    let request = SummaryRequest {
        previous_summary: Some("s1".to_owned()),
        newly_retired: vec![SummaryInputItem::Message(user("u1", "hi"))],
        split_turn_prefix: None,
    };
    let text = summarizer
        .summarize(request.clone(), rustx::runtime::CancellationSignal::new())
        .await
        .expect("summary");
    assert_eq!(text, "summary text");

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model, "fake-model");
    assert_eq!(requests[0].protocol, ModelProtocol::OpenAiChatCompletions);
    assert_eq!(requests[0].reasoning, ReasoningEffort::High);
    assert_eq!(requests[0].max_output_tokens, 128);
    assert!(requests[0].tools.is_empty());
    assert_eq!(requests[0].continuation, None);
    let MessageBlock::User(user) = &requests[0].messages[0] else {
        panic!("summary instruction must be a user message");
    };
    let text = match &user.content[0] {
        UserContentBlock::Text(block) => &block.text,
        _ => panic!("summary instruction must be text"),
    };
    assert!(text.contains("Summarize the following conversation history"));
    // The serialized input is deterministic and embedded verbatim.
    let serialized = serde_json::to_string(&request).expect("serialize");
    assert!(text.contains(&serialized));
}

/// A refusal, a tool request, and a model failure are compaction failures,
/// never summaries.
#[tokio::test]
async fn model_backed_summarizer_rejects_invalid_streams() {
    let scripted = scripted_call();
    let cases: Vec<(Vec<FakeStep>, ContextErrorKind)> = vec![
        (
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(ModelEvent::RefusalDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "I cannot".to_owned(),
                }),
                FakeStep::Emit(done(ModelFinishReason::Refusal)),
            ],
            ContextErrorKind::SummaryFailed,
        ),
        (
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(tool_call_events(0, &scripted)[0].clone()),
                FakeStep::Emit(tool_call_events(0, &scripted)[1].clone()),
                FakeStep::Emit(tool_call_events(0, &scripted)[2].clone()),
                FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
            ],
            ContextErrorKind::SummaryFailed,
        ),
        (
            vec![FakeStep::Emit(fail(
                rustx::model::ModelErrorKind::ProviderError,
                "provider down",
            ))],
            ContextErrorKind::SummaryFailed,
        ),
    ];
    for (events, expected) in cases {
        let model = FakeModel::new(vec![events]);
        let summarizer = ModelBackedSummarizer::new(
            &model,
            SummaryModelConfig {
                model: "fake-model".to_owned(),
                protocol: ModelProtocol::OpenAiChatCompletions,
                reasoning: ReasoningEffort::Medium,
                max_output_tokens: 64,
            },
        );
        let request = SummaryRequest {
            previous_summary: None,
            newly_retired: vec![],
            split_turn_prefix: None,
        };
        let error = summarizer
            .summarize(request, rustx::runtime::CancellationSignal::new())
            .await
            .expect_err("must fail");
        assert_eq!(error.kind, expected);
    }
}

/// A refusal with no `RefusalDelta`, an empty `Stop` output, and a
/// whitespace-only output are compaction failures: the terminal finish
/// reason is authoritative and empty output can never be a summary.
#[tokio::test]
async fn model_backed_summarizer_rejects_refusal_without_delta_and_empty_output() {
    let cases: Vec<Vec<FakeStep>> = vec![
        // Refusal with no RefusalDelta: the finish reason alone must fail.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Refusal)),
        ],
        // Completed(Stop) with no TextDelta at all.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        // Whitespace-only output.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "   \n\t ")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ];
    for events in cases {
        let model = FakeModel::new(vec![events]);
        let summarizer = ModelBackedSummarizer::new(
            &model,
            SummaryModelConfig {
                model: "fake-model".to_owned(),
                protocol: ModelProtocol::OpenAiChatCompletions,
                reasoning: ReasoningEffort::Medium,
                max_output_tokens: 64,
            },
        );
        let request = SummaryRequest {
            previous_summary: None,
            newly_retired: vec![],
            split_turn_prefix: None,
        };
        let error = summarizer
            .summarize(request, rustx::runtime::CancellationSignal::new())
            .await
            .expect_err("invalid summary must fail");
        assert_eq!(error.kind, ContextErrorKind::SummaryFailed);
    }
}

/// Malformed canonical stream orderings are compaction failures: content
/// before `Started`, a duplicate `Started`, `Completed` without `Started`,
/// and events after the terminal are never folded into a summary.
#[tokio::test]
async fn model_backed_summarizer_rejects_malformed_stream_orderings() {
    let cases: Vec<Vec<FakeStep>> = vec![
        // Content before Started.
        vec![
            FakeStep::Emit(text_delta(0, "early")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        // Duplicate Started.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "x")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        // Completed without Started.
        vec![FakeStep::Emit(done(ModelFinishReason::Stop))],
        // Events after the terminal event.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
            FakeStep::Emit(text_delta(0, "late")),
        ],
    ];
    for events in cases {
        let model = FakeModel::new(vec![events]);
        let summarizer = ModelBackedSummarizer::new(
            &model,
            SummaryModelConfig {
                model: "fake-model".to_owned(),
                protocol: ModelProtocol::OpenAiChatCompletions,
                reasoning: ReasoningEffort::Medium,
                max_output_tokens: 64,
            },
        );
        let request = SummaryRequest {
            previous_summary: None,
            newly_retired: vec![],
            split_turn_prefix: None,
        };
        let error = summarizer
            .summarize(request, rustx::runtime::CancellationSignal::new())
            .await
            .expect_err("malformed stream must fail");
        assert_eq!(error.kind, ContextErrorKind::SummaryFailed);
    }
}

/// Cancellation aborts the summary.
#[tokio::test]
async fn model_backed_summarizer_aborts_on_cancellation() {
    let model = FakeModel::new(vec![vec![FakeStep::ParkUntilCancelled]]);
    let summarizer = ModelBackedSummarizer::new(
        &model,
        SummaryModelConfig {
            model: "fake-model".to_owned(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            reasoning: ReasoningEffort::Medium,
            max_output_tokens: 64,
        },
    );
    let cancellation = rustx::runtime::CancellationSignal::new();
    let request = SummaryRequest {
        previous_summary: None,
        newly_retired: vec![],
        split_turn_prefix: None,
    };
    let mut parked = model.parked();
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        parked.wait_for(|value| *value).await.expect("model parked");
        controller_cancellation.cancel();
    });
    let error = summarizer
        .summarize(request, cancellation)
        .await
        .expect_err("cancelled");
    controller.await.expect("controller task");
    assert_eq!(error.kind, ContextErrorKind::Cancelled);
}

/// A model-backed summarizer inside an execution uses its own one-off
/// request: its usage and continuation never contaminate the attempt's
/// request state.
#[tokio::test]
async fn model_backed_summarizer_does_not_contaminate_the_execution() {
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "provisional")),
            FakeStep::Emit(overflow_event()),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "summary")),
            FakeStep::Emit(done_with_usage(ModelFinishReason::Stop, 3)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "ok")),
            FakeStep::Emit(done_with_usage(ModelFinishReason::Stop, 2)),
        ],
    ]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let store = InMemoryCheckpointStore::new().shared();
    let runtime = ContextRuntime::model_backed(
        ContextConfig {
            context_window_tokens: 500,
            reserve_tokens: 0,
            keep_recent_tokens: 5,
        },
        weighted(100, 10, 0),
        &model,
        SummaryModelConfig {
            model: "fake-model".to_owned(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            reasoning: ReasoningEffort::Medium,
            max_output_tokens: 0,
        },
        store.clone(),
    )
    .expect("runtime");
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .run()
    .await;

    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    // The summary request is the canonical one-off: no tools, no
    // continuation.
    assert!(requests[1].tools.is_empty());
    assert_eq!(requests[1].continuation, None);
    // The retry request carries the summary projection and no continuation.
    assert_eq!(requests[2].continuation, None);
    assert!(matches!(
        &requests[2].messages[0],
        MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary
    ));
    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

// ---------------------------------------------------------------------------
// Provider isolation
// ---------------------------------------------------------------------------

/// `src/context/` is source-level isolated from provider SDK/wire
/// dependencies: no provider-private module or crate leaks into the context
/// plane.
#[test]
fn context_sources_contain_no_provider_dependencies() {
    let banned = [
        "async_openai",
        "reqwest",
        "adapter::openai",
        "adapter::anthropic",
        "OpenAiResponsesAdapter",
        "OpenAiChatCompletionsAdapter",
        "AnthropicMessagesAdapter",
        "eventsource_stream",
    ];
    let mut files = std::fs::read_dir("src/context")
        .expect("context directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect::<Vec<_>>();
    files.sort();
    assert!(!files.is_empty(), "context sources must exist");
    for file in files {
        let source = std::fs::read_to_string(&file).expect("read source");
        for pattern in banned {
            assert!(
                !source.contains(pattern),
                "{} contains provider-private dependency {:?}",
                file.display(),
                pattern
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn newly_retired_id(item: &SummaryInputItem) -> String {
    match item {
        SummaryInputItem::Message(message) => message_id_of(message),
        SummaryInputItem::AgentSlice { message_id, .. } => message_id.as_str().to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Issue #22 — drained inbound batches before M4 projection/compaction
// ---------------------------------------------------------------------------

/// A timestamped ordinary inbound message for mailbox enqueue.
fn inbound_user(id: &str, text: &str, source: UserSource) -> UserMessageBlock {
    UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source,
        kind: InboundKind::Message,
        timestamp: Some(
            chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("parse fixed timestamp")
                .with_timezone(&chrono::Utc),
        ),
    }
}

fn block_id(block: &MessageBlock) -> String {
    match block {
        MessageBlock::System(system) => system.id.to_string(),
        MessageBlock::User(user) => user.id.to_string(),
        MessageBlock::Agent(agent) => agent.id.to_string(),
        MessageBlock::Tool(tool) => tool.id.to_string(),
    }
}

/// Scripts a two-turn model whose first turn parks (with a released text
/// delta) and completes with Stop; the second turn completes immediately.
fn parked_two_turn_model(release: tokio::sync::watch::Receiver<bool>) -> FakeModel {
    FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "doing")),
            FakeStep::ParkUntilReleased(release),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ])
}

/// Enqueues human A and runtime B while turn 1 is parked, then releases the
/// turn; returns the spawned controller task.
fn controller_enqueue_a_and_b(
    model: &FakeModel,
    mailbox: &ConversationInboundMailbox,
    release: tokio::sync::watch::Sender<bool>,
) -> tokio::task::JoinHandle<()> {
    let controller_mailbox = mailbox.clone();
    let mut model_parked = model.parked();
    tokio::spawn(async move {
        model_parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("model parked");
        controller_mailbox
            .enqueue(inbound_user("msg-inbound-a", "human A", UserSource::Human))
            .expect("enqueue human A");
        controller_mailbox
            .enqueue(inbound_user(
                "msg-inbound-b",
                "runtime B",
                UserSource::Runtime,
            ))
            .expect("enqueue runtime B");
        release.send(true).expect("release turn 1");
    })
}

/// A drained batch is appended to canonical history before the next
/// projection: the `ContextProjection` used by the next request and the
/// captured `ModelRequest` both contain every drained message.
#[tokio::test]
async fn m4_projection_contains_drained_batch_before_request() {
    let (release, parked) = model_release();
    let model = parked_two_turn_model(parked.clone());
    let tools = tool_registry_with_alpha();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    // A window far above the projected input: no compaction may interfere.
    let runtime = runtime_with(
        10_000,
        0,
        0,
        weighted(100, 10, 0),
        FakeContextSummarizer::new(Vec::new()),
        InMemoryCheckpointStore::new().shared(),
    );
    let mailbox = ConversationInboundMailbox::new(conversation());
    let controller = controller_enqueue_a_and_b(&model, &mailbox, release);
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-u0", "start")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .with_inbound_mailbox(mailbox)
    .expect("mailbox belongs to the request conversation")
    .run()
    .await;
    controller.await.expect("controller task");

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted)),
        "no compaction below the threshold"
    );
    // Canonical history contains the distinct inbound messages.
    let ids: Vec<String> = result.messages.iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec![
            "msg-u0".to_owned(),
            agent_message_id(1).to_string(),
            "msg-inbound-a".to_owned(),
            "msg-inbound-b".to_owned(),
            agent_message_id(2).to_string(),
        ]
    );
    // The captured ModelRequest of the next model turn contains A and B.
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let request_ids: Vec<String> = requests[1].messages.iter().map(block_id).collect();
    assert_eq!(
        request_ids,
        vec![
            "msg-u0".to_owned(),
            agent_message_id(1).to_string(),
            "msg-inbound-a".to_owned(),
            "msg-inbound-b".to_owned(),
        ],
        "the projection was built after the batch drain, never before"
    );
}

/// Appending a drained batch may cross the proactive compaction threshold:
/// the M4 engine compacts the projection while the inbound messages remain
/// canonical history.
#[tokio::test]
#[allow(clippy::too_many_lines)] // the full compaction-after-drain contract is asserted verbatim
async fn m4_compaction_after_drain_preserves_canonical_inbound() {
    let (release, parked) = model_release();
    let model = parked_two_turn_model(parked.clone());
    let tools = tool_registry_with_alpha();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    // per-message = 100, per-block = 10, window = 250:
    // before the drain the projection is 100 tokens (below the threshold);
    // after the drain [u0, agent, A, B] is 310 tokens (at/above it), so the
    // drained batch deterministically triggers proactive compaction.
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Return("S".to_owned())]);
    let store = InMemoryCheckpointStore::new().shared();
    let runtime = runtime_with(250, 0, 0, weighted(100, 10, 0), summarizer, store.clone());
    let mailbox = ConversationInboundMailbox::new(conversation());
    let controller = controller_enqueue_a_and_b(&model, &mailbox, release);
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-u0", "start")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .with_inbound_mailbox(mailbox)
    .expect("mailbox belongs to the request conversation")
    .run()
    .await;
    controller.await.expect("controller task");

    assert_single_terminal(&result.events);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    // The drained batch pushed the projection over the threshold: exactly
    // one proactive compaction ran.
    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::CompactionStarted))
            .count(),
        1,
        "the drained batch must cross the compaction threshold exactly once"
    );
    // Canonical history still contains the original inbound UserMessageBlocks
    // even though the model-facing history was summarized.
    let ids: Vec<String> = result.messages.iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec![
            "msg-u0".to_owned(),
            agent_message_id(1).to_string(),
            "msg-inbound-a".to_owned(),
            "msg-inbound-b".to_owned(),
            agent_message_id(2).to_string(),
        ],
        "canonical history preserves the drained inbound messages"
    );
    // The request continues on the compacted projection: the summary stands
    // for the older model-facing history, while the drained batch — now a
    // fresh inbound turn that the model has not yet observed — is protected
    // from compaction and remains literal.
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let request_ids: Vec<String> = requests[1].messages.iter().map(block_id).collect();
    assert_eq!(
        request_ids,
        vec![
            summary_id(1).to_string(),
            "msg-inbound-a".to_owned(),
            "msg-inbound-b".to_owned(),
        ],
        "the continuation request uses the compacted projection with the \
         unobserved fresh inbound preserved literally"
    );
    // Exactly one Agent Status snapshot accompanies the fresh inbound turn,
    // targeting its final message; the attachment is never a canonical
    // message.
    let status = requests[1]
        .agent_status
        .as_ref()
        .expect("one status snapshot");
    assert_eq!(
        status.target_message_id,
        MessageId::new("msg-inbound-b"),
        "the status targets the final fresh inbound message"
    );
    assert!(
        status.rendered.contains("<system-reminder>")
            && status.rendered.contains("Inbound message time"),
        "the rendered status is the canonical system-reminder footer"
    );
    let serialized = serde_json::to_string(&requests[1].messages).expect("serialize");
    assert!(
        serialized.contains('S'),
        "the summary reaches the projection"
    );
    // The stored checkpoint summary is a derived compaction summary: no
    // fabricated wall-clock timestamp.
    let checkpoint = store
        .load(&conversation())
        .expect("store")
        .expect("checkpoint");
    assert!(
        matches!(
            &checkpoint.summary,
            UserMessageBlock {
                source: UserSource::Runtime,
                kind: InboundKind::CompactionSummary,
                timestamp: None,
                ..
            }
        ),
        "a compaction summary never carries a fabricated timestamp"
    );
    // The status is projection-only: it never appears in canonical history
    // or in the checkpoint.
    let history_serialized = serde_json::to_string(&result.messages).expect("serialize");
    assert!(
        !history_serialized.contains("<system-reminder>"),
        "canonical history must never contain the Agent Status footer"
    );
    let checkpoint_serialized = serde_json::to_string(&checkpoint).expect("serialize");
    assert!(
        !checkpoint_serialized.contains("<system-reminder>"),
        "the checkpoint must never contain the Agent Status footer"
    );
}

/// Without compaction, an ordinary inbound drain retains the pending
/// provider continuation through the M4 projection path.
#[tokio::test]
async fn m4_drain_retains_continuation_without_compaction() {
    let state = anthropic_state();
    let (release, parked) = model_release();
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(ModelEvent::ContinuationState {
                block_index: ContentBlockIndex::new(0),
                state: state.clone(),
            }),
            FakeStep::Emit(text_delta(1, "doing")),
            FakeStep::ParkUntilReleased(parked.clone()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = tool_registry_with_alpha();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let runtime = runtime_with(
        10_000,
        0,
        0,
        weighted(100, 10, 0),
        FakeContextSummarizer::new(Vec::new()),
        InMemoryCheckpointStore::new().shared(),
    );
    let mailbox = ConversationInboundMailbox::new(conversation());
    let controller_mailbox = mailbox.clone();
    let mut model_parked = model.parked();
    let controller = tokio::spawn(async move {
        model_parked
            .wait_for(|is_parked| *is_parked)
            .await
            .expect("model parked");
        controller_mailbox
            .enqueue(inbound_user("msg-inbound-a", "continue", UserSource::Human))
            .expect("enqueue inbound message");
        release.send(true).expect("release turn 1");
    });
    let tool_runtime = common::tool_runtime("conv-1");
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-u0", "hi")], 0),
        &model,
        &tools,
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .with_inbound_mailbox(mailbox)
    .expect("mailbox belongs to the request conversation")
    .run()
    .await;
    controller.await.expect("controller task");

    assert_single_terminal(&result.events);
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted)),
        "no compaction below the threshold"
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].continuation,
        Some(state),
        "the ordinary inbound drain does not invalidate the continuation"
    );
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|block| matches!(block, MessageBlock::User(user) if user.id == MessageId::new("msg-inbound-a"))),
        "the drained message is part of the projection"
    );
}
