//! M7.5 deterministic context engine tests (Issue #54).
//!
//! Every test is deterministic and network-free. Engine-level tests drive
//! `ContextEngine` directly over a real `ConversationState` (Message Ledger
//! plus Conversation Surface) with scripted estimators. Agent-level tests
//! drive `AgentExecution` with the `ContextRuntime` bundle over scripted
//! fixture models, tools, and summarizers, and assert behavior through the
//! recorded `RuntimeEvent` trace, the platform outcome, the committed
//! Message Ledger, the Conversation Surface, and the recorded requests.
//!
//! These tests protect the Issue #54 contracts: complete-message projection,
//! canonical runtime summaries, current-Surface planning, immutable Ledger
//! facts, exact `SurfaceRevision` reconstruction, and bounded provider input.

use super::{common, support};

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionRequest, AgentExecutionResult,
};
use rustx::context::{
    CompactionBudgets, ContextAssembly, ContextConfig, ContextEngine, ContextError,
    ContextErrorKind, ContextProposal, ContextRuntime, ContextSummarizer, DefaultTokenEstimator,
    ModelBackedSummarizer, ProviderObservedInput, SummaryRequest, TokenEstimator,
    UserMessageProposal,
};
use rustx::conversation::{
    ConversationState, SurfaceOp, SurfaceRevision, SurfaceSpan, summary_message_id,
};
use rustx::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, InboundKind, MessageBlock,
    ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::{ModelRequest, ModelUsage};
use rustx::runtime::continuation::{
    AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
};
use rustx::runtime::identity::{
    AgentId, AttemptId, ConversationId, MessageId, RequestId, ToolCallId, ToolId,
};
use rustx::runtime::inbound::ConversationInboundMailbox;
use rustx::runtime::types::{CancellationReason, TokenMeasurement, TokenMeasurementSource};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{ToolCall, ToolCallStart, ToolExecutionResult, ToolExecutionStatus};
use support::context::{FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator};
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, fake_model, model_release, success_result,
    tool_call_events,
};

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

fn text_block(text: &str) -> AssistantContentBlock {
    AssistantContentBlock::Text(TextBlock {
        text: text.to_owned(),
    })
}

fn call_block(id: &str) -> AssistantContentBlock {
    AssistantContentBlock::ToolCall(ToolCall {
        id: ToolCallId::new(id),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".to_owned(),
        arguments: serde_json::json!({}),
    })
}

fn assistant(id: &str, blocks: Vec<AssistantContentBlock>) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
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
    summary_message_id(&conversation(), generation)
}

/// One conversation state bootstrapped from ordered canonical messages.
fn state(messages: Vec<MessageBlock>) -> ConversationState {
    ConversationState::from_messages(messages).expect("bootstrap conversation")
}

/// The active Surface identities of a conversation state, as strings.
fn active_ids(state: &ConversationState) -> Vec<String> {
    state
        .active_ids()
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect()
}

/// The committed Message Ledger identities, in commit order, as strings.
fn ledger_ids(state: &ConversationState) -> Vec<String> {
    state
        .ledger()
        .audit_records()
        .iter()
        .map(message_id_of)
        .collect()
}

/// The projected model-visible identities of a projection, as strings.
fn projected_ids(projection: &rustx::context::ContextProjection) -> Vec<String> {
    projection.messages.iter().map(message_id_of).collect()
}

/// Plans, summarizes, and applies one compaction against a conversation
/// state, returning the committed record.
fn compact(
    engine: &ContextEngine,
    state: &mut ConversationState,
    summary_text: &str,
    budgets: CompactionBudgets,
) -> Result<rustx::conversation::CompactionRecord, ContextError> {
    compact_with(
        engine,
        state,
        summary_text,
        budgets,
        &rustx::context::CompactionConstraints::default(),
        &[],
    )
}

/// The same, with explicit constraints and tool definitions.
fn compact_with(
    engine: &ContextEngine,
    state: &mut ConversationState,
    summary_text: &str,
    budgets: CompactionBudgets,
    constraints: &rustx::context::CompactionConstraints<'_>,
    tools: &[rustx::tools::types::ModelToolDefinition],
) -> Result<rustx::conversation::CompactionRecord, ContextError> {
    let projection = engine.build_projection(state, tools, None, "")?;
    let plan = engine.plan_compaction(state, &projection, tools, budgets, constraints)?;
    let (commit, _) =
        engine.prepare_compaction(state, &conversation(), &plan, summary_text, tools)?;
    state
        .commit_compaction(commit)
        .map_err(|error| ContextError::new(ContextErrorKind::Internal, error.to_string()))
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
    model: &Arc<FakeModel>,
) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id: conversation(),
        attempt_id: AttemptId::new(attempt),
        conversation: state(initial_messages),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: support::attempt_model_with_window(
            model.clone(),
            "fake-model",
            10_000_000,
            max_output_tokens.max(1),
        ),
    }
}

fn assistant_message_id(turn: u32) -> MessageId {
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

/// The resolved summary invocation of a scripted model, with an explicit
/// output budget.
///
/// It is built through the same catalog resolution as a primary model, so a
/// summarizer test exercises the real binding path.
fn summary_invocation(
    model: &Arc<FakeModel>,
    max_output_tokens: u32,
) -> rustx::model::ResolvedModelInvocation {
    support::attempt_model_with_window(model.clone(), "fake-model", 10_000_000, max_output_tokens)
        .summary_invocation()
        .clone()
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
) -> ContextRuntime {
    ContextRuntime::with_scripted_summarizer(
        engine(window, reserve, keep_recent, estimator),
        Arc::new(summarizer),
        rustx::context::AgentStatusComposer::default(),
        CompactionBudgets::new(1, 1, 1_000_000),
    )
}

fn runtime_with_assembly(
    window: u64,
    reserve: u64,
    keep_recent: u64,
    estimator: Arc<dyn TokenEstimator>,
    summarizer: FakeContextSummarizer,
    assembly: ContextAssembly,
) -> ContextRuntime {
    ContextRuntime::with_scripted_summarizer_and_assembly(
        engine(window, reserve, keep_recent, estimator),
        Arc::new(summarizer),
        rustx::context::AgentStatusComposer::default(),
        assembly,
        CompactionBudgets::new(1, 1, 1_000_000),
    )
}

/// A pre-step policy that only counts how often it was evaluated.
///
/// It exists to prove the Issue #56 admission contract inside the Issue #55
/// overflow-retry regression: an overflow retry is not a new model-step
/// admission, so the policy must not run a second time.
struct CountingPreStepPolicy {
    evaluations: Arc<AtomicUsize>,
}

impl rustx::agent::PreStepPolicy for CountingPreStepPolicy {
    fn evaluate<'a>(
        &'a self,
        _batch: &'a rustx::agent::PreStepBatch<'a>,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<rustx::agent::PreStepDecision, rustx::agent::LifecycleError>,
    > {
        self.evaluations.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(rustx::agent::PreStepDecision::Enter) })
    }
}

/// Whether a canonical message is a runtime compaction summary.
fn is_summary(message: &MessageBlock) -> bool {
    matches!(message, MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary)
}

/// Asserts that the attempt committed no compaction at all: no canonical
/// runtime summary joined the Message Ledger and the Conversation Surface
/// performed no replacement.
fn assert_no_compaction_committed(result: &AgentExecutionResult) {
    assert_eq!(
        result.conversation.surface().compaction_generation(),
        0,
        "no surface replacement may have been applied"
    );
    assert!(
        !result.messages().iter().any(is_summary),
        "no canonical compaction summary may have been committed"
    );
}

/// The committed canonical compaction summary of a settled attempt.
fn committed_summary(result: &AgentExecutionResult) -> &UserMessageBlock {
    result
        .messages()
        .iter()
        .find_map(|message| match message {
            MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary => Some(user),
            _ => None,
        })
        .expect("a successful compaction commits one canonical summary")
}

fn message_id_of(message: &MessageBlock) -> String {
    match message {
        MessageBlock::System(system) => system.id.as_str().to_owned(),
        MessageBlock::User(user) => user.id.as_str().to_owned(),
        MessageBlock::Assistant(assistant) => assistant.id.as_str().to_owned(),
        MessageBlock::Tool(tool) => tool.id.as_str().to_owned(),
    }
}

/// A conversation state that has already compacted once: the `span` is
/// replaced by the canonical generation-1 runtime summary.
fn compacted_state(
    messages: Vec<MessageBlock>,
    span: SurfaceSpan,
    summary_text: &str,
) -> ConversationState {
    let mut state = state(messages);
    let summary = UserMessageBlock {
        id: summary_id(1),
        content: vec![UserContentBlock::Text(TextBlock {
            text: summary_text.to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::CompactionSummary,
        timestamp: None,
    };
    let commit = state
        .prepare_compaction(summary, span)
        .expect("a valid compaction span");
    state.commit_compaction(commit).expect("commit compaction");
    state
}

// ---------------------------------------------------------------------------
// Context assembly
// ---------------------------------------------------------------------------

/// A short conversation stays below the threshold: no compaction.
#[test]
fn short_history_requires_no_compaction() {
    let engine = engine(100, 10, 5, weighted(10, 10, 10));
    let state = state(vec![user("u1", "hi"), user("u2", "bye")]);
    let projection = engine
        .build_projection(&state, &[], None, "")
        .expect("projection");
    assert!(
        !engine
            .should_compact(&projection, 0)
            .expect("threshold decision")
    );
    assert!(engine.fits_under_soft_limit(&projection, 0).expect("fits"));
}

/// The projection is exactly the current Surface, in Surface order, as
/// complete canonical messages — and it is a pure function of that Surface
/// revision.
#[test]
fn projection_is_the_current_surface_in_order() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let state = compacted_state(
        vec![
            system("sys-1", "Be concise."),
            user("u1", "hi"),
            assistant("a1", vec![text_block("ok")]),
            user("u2", "more"),
        ],
        SurfaceSpan::new(MessageId::new("u1"), MessageId::new("a1")),
        "earlier summary",
    );
    let first = engine
        .build_projection(&state, &[], None, "")
        .expect("projection");
    let second = engine
        .build_projection(&state, &[], None, "")
        .expect("projection again");
    assert_eq!(first, second, "projection must be a pure function");
    assert_eq!(first.surface_revision, state.revision());
    assert_eq!(
        projected_ids(&first),
        vec![
            "sys-1".to_owned(),
            summary_id(1).as_str().to_owned(),
            "u2".to_owned(),
        ],
        "the projection is exactly the active surface"
    );
    assert!(
        first.messages.iter().all(|message| matches!(
            message,
            MessageBlock::System(_)
                | MessageBlock::User(_)
                | MessageBlock::Assistant(_)
                | MessageBlock::Tool(_)
        )),
        "every projected item is a complete canonical message"
    );
    // Compaction never rewrote the ledger.
    assert_eq!(
        ledger_ids(&state),
        vec![
            "sys-1".to_owned(),
            "u1".to_owned(),
            "a1".to_owned(),
            "u2".to_owned(),
            summary_id(1).as_str().to_owned(),
        ]
    );
}

/// The same history produces the same estimate.
#[test]
fn same_context_produces_same_estimate() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let state = state(vec![
        user("u1", "hi"),
        assistant("a1", vec![text_block("ok")]),
    ]);
    let first = engine
        .build_projection(&state, &[], None, "")
        .expect("projection");
    let second = engine
        .build_projection(&state, &[], None, "")
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
    let state = state(vec![user("u1", "hi")]);
    let tools = vec![
        common::model_tool("alpha", "tool-alpha"),
        common::model_tool("beta", "tool-beta"),
    ];
    let without_tools = engine
        .build_projection(&state, &[], None, "")
        .expect("projection without tools");
    let with_tools = engine
        .build_projection(&state, &tools, None, "")
        .expect("projection with tools");
    assert_eq!(with_tools.estimated_input.input_tokens, 30);
    assert_eq!(without_tools.estimated_input.input_tokens, 10);
}

/// Tool definitions never satisfy the recent-conversation retention target:
/// the retention decision is a pure function of conversation content, while
/// the full request estimate still includes the tool overhead.
#[test]
fn tool_definitions_never_satisfy_the_recent_retention_target() {
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![text_block("x")]),
        user("u2", ""),
        assistant("a2", vec![text_block("y")]),
    ]);
    let tools = vec![common::model_tool("alpha", "tool-alpha")];
    // Target 20: with conversation weights of 10/10, retiring u1 and a1
    // retains exactly u2+a2 = 20. If the huge tool weight counted toward the
    // target, the engine would retire everything instead.
    let cheap = engine(10_000_000, 0, 20, weighted(10, 10, 0));
    let expensive = engine(10_000_000, 0, 20, weighted(10, 10, 1_000_000));
    let projection_cheap = cheap
        .build_projection(&history, &tools, None, "")
        .expect("projection");
    let projection_expensive = expensive
        .build_projection(&history, &tools, None, "")
        .expect("projection");
    let plan_cheap = cheap
        .plan_compaction(
            &history,
            &projection_cheap,
            &tools,
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    let plan_expensive = expensive
        .plan_compaction(
            &history,
            &projection_expensive,
            &tools,
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // Identical retention decision: the tool weight changes the full request
    // estimate but never the recent-conversation target.
    assert_eq!(plan_cheap.span.end, MessageId::new("a1"));
    assert_eq!(plan_cheap.span.end, plan_expensive.span.end);
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
    let history = state(vec![user("u1", "hi")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: projection.fingerprint(),
        input_tokens: 42,
    };
    let measured = engine
        .build_projection(&history, &[], Some(&observed), "")
        .expect("projection with observed usage");
    assert_eq!(measured.estimated_input.input_tokens, 42);
    assert_eq!(
        measured.estimated_input.source,
        TokenMeasurementSource::ProviderReported
    );

    // A different history is a different projection: the observed
    // measurement does not apply, and the estimate is used instead.
    let grown = state(vec![user("u1", "hi"), user("u2", "more")]);
    let estimated = engine
        .build_projection(&grown, &[], Some(&observed), "")
        .expect("projection with stale observation");
    assert_eq!(estimated.estimated_input.input_tokens, 20);
    assert_eq!(
        estimated.estimated_input.source,
        TokenMeasurementSource::Estimated
    );
}

/// A Surface rewrite invalidates a stale provider-reported measurement: the
/// request context it measured no longer exists.
#[test]
fn a_surface_rewrite_invalidates_a_stale_observed_measurement() {
    let engine = engine(10_000, 0, 0, weighted(10, 10, 10));
    let mut history = state(vec![user("u1", ""), user("u2", ""), user("u3", "")]);
    let before = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: before.fingerprint(),
        input_tokens: 42,
    };
    // The measurement applies to exactly the context it measured.
    assert_eq!(
        engine
            .build_projection(&history, &[], Some(&observed), "")
            .expect("measured projection")
            .estimated_input
            .source,
        TokenMeasurementSource::ProviderReported
    );
    // A surface rewrite establishes a new revision and new content: the old
    // measurement can never apply to it.
    compact(
        &engine,
        &mut history,
        "s1",
        CompactionBudgets::new(0, 0, 1_000_000),
    )
    .expect("compact");
    let after = engine
        .build_projection(&history, &[], Some(&observed), "")
        .expect("projection after the rewrite");
    assert_ne!(after.surface_revision, before.surface_revision);
    assert_eq!(
        after.estimated_input.source,
        TokenMeasurementSource::Estimated,
        "a surface rewrite must invalidate the stale observed measurement"
    );
    // An ordinary append does the same.
    let mut appended = state(vec![user("u1", ""), user("u2", ""), user("u3", "")]);
    appended.commit(user("u4", "")).expect("append");
    assert_eq!(
        engine
            .build_projection(&appended, &[], Some(&observed), "")
            .expect("projection after the append")
            .estimated_input
            .source,
        TokenMeasurementSource::Estimated
    );
}

/// Missing provider usage means the deterministic estimate, never a
/// fabricated measurement.
#[test]
fn missing_usage_falls_back_to_the_estimate() {
    let engine = engine(1_000, 10, 5, weighted(10, 10, 10));
    let history = state(vec![user("u1", "hi")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
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
    let history = state(vec![user("u1", "hi")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
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
    let history = state(vec![user("u1", "hi")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let expected = rustx::context::bytes_to_tokens(
        serde_json::to_vec(&projection.messages)
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
        .build_projection(
            &state(vec![
                user("u1", ""),
                user("u2", ""),
                user("u3", ""),
                user("u4", ""),
                user("u5", ""),
            ]),
            &[],
            None,
            "",
        )
        .expect("projection");
    assert_eq!(at.estimated_input.input_tokens, 100);
    assert!(
        engine
            .should_compact(&at, 0)
            .expect("at threshold: compact")
    );

    let below = engine
        .build_projection(
            &state(vec![
                user("u1", ""),
                user("u2", ""),
                user("u3", ""),
                user("u4", ""),
            ]),
            &[],
            None,
            "",
        )
        .expect("projection");
    assert_eq!(below.estimated_input.input_tokens, 80);
    assert!(
        !engine
            .should_compact(&below, 0)
            .expect("below threshold: no compaction")
    );

    let above = engine
        .build_projection(
            &state(vec![
                user("u1", ""),
                user("u2", ""),
                user("u3", ""),
                user("u4", ""),
                user("u5", ""),
                user("u6", ""),
            ]),
            &[],
            None,
            "",
        )
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

/// The primary output budget owns the soft input limit while the summary
/// invocation owns the reservation used by the planner's hard-fit choice.
/// A smaller summary than the primary leaves the recent-turn boundary viable.
#[test]
fn compaction_uses_primary_budget_for_soft_limit_and_smaller_summary_reservation() {
    let engine = engine(40, 0, 20, weighted(10, 10, 0));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![text_block("x")]),
        user("u2", ""),
        assistant("a2", vec![text_block("y")]),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(10, 5, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan fits with the smaller summary reservation");

    assert_eq!(plan.summary_reservation, 5);
    assert_eq!(plan.span.end, MessageId::new("a1"));
    assert!(plan.planned_estimate_after <= 30);
}

/// An explicit summary model with a larger output budget can force the
/// planner to retire a whole additional turn, even though the primary soft
/// input limit is unchanged. This proves the hard-fit decision uses the
/// summary reservation rather than merely observing a provider request.
#[test]
fn compaction_uses_larger_explicit_summary_reservation_for_hard_fit() {
    let engine = engine(40, 0, 20, weighted(10, 10, 0));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![text_block("x")]),
        user("u2", ""),
        assistant("a2", vec![text_block("y")]),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(10, 25, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("full compaction fits with the larger summary reservation");

    assert_eq!(plan.summary_reservation, 25);
    assert_eq!(plan.span.end, MessageId::new("a2"));
    assert!(plan.planned_estimate_after <= 30);
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
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.span.start, MessageId::new("u1"));
    assert_eq!(plan.span.end, MessageId::new("t1"));
    assert_eq!(plan.retired.len(), 3, "the complete turn is retired whole");
}

/// Multiple tool calls of one Assistant message are never separated from their
/// results, and the Assistant message is never split: a span that would retire
/// one call without its result is structurally rejected, and the only
/// admissible spans keep every call together with its result.
#[test]
fn multiple_tool_calls_stay_with_their_results() {
    let engine = engine(10_000, 0, 0, weighted(100, 10, 100));
    let mut conversation_state = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1"), call_block("c2")]),
        tool_message("t1", "c1"),
        tool_message("t2", "c2"),
    ]);
    // The Assistant message can never be replaced without its results.
    for end in ["a1", "t1"] {
        let error = conversation_state
            .prepare_compaction(
                UserMessageBlock {
                    id: summary_id(1),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "s".to_owned(),
                    })],
                    source: UserSource::Runtime,
                    kind: InboundKind::CompactionSummary,
                    timestamp: None,
                },
                SurfaceSpan::new(MessageId::new("u1"), MessageId::new(end)),
            )
            .expect_err("a tool pair may never be split");
        assert!(
            format!("{error}").contains("separate tool call"),
            "unexpected error: {error}"
        );
    }
    // The engine only ever plans a structurally complete span.
    let projection = engine
        .build_projection(&conversation_state, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &conversation_state,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.span.end, MessageId::new("t2"));
    compact(
        &engine,
        &mut conversation_state,
        "s1",
        CompactionBudgets::new(0, 0, 1_000_000),
    )
    .expect("compact");
    assert_eq!(
        active_ids(&conversation_state),
        vec![summary_id(1).as_str()]
    );
    assert_eq!(
        ledger_ids(&conversation_state),
        vec![
            "u1".to_owned(),
            "a1".to_owned(),
            "t1".to_owned(),
            "t2".to_owned(),
            summary_id(1).as_str().to_owned(),
        ],
        "every retired original survives in the ledger"
    );
}

/// Orphan tool messages are malformed history, never guessed around.
#[test]
fn orphan_tool_message_is_rejected() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![user("u1", ""), tool_message("t1", "ghost")]);
    let error = engine
        .build_projection(&history, &[], None, "")
        .expect_err("malformed history");
    assert_eq!(error.kind, ContextErrorKind::MalformedHistory);
}

/// No tool-call/result edge crosses the chosen cut: turns are retired or
/// retained whole.
#[test]
fn no_edge_crosses_the_chosen_cut() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        assistant("a2", vec![call_block("c2")]),
        tool_message("t2", "c2"),
        user("u2", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let mut history = history;
    let record = compact(
        &engine,
        &mut history,
        "s1",
        CompactionBudgets::new(0, 0, 1_000_000),
    )
    .expect("compact");
    assert_eq!(record.generation, 1);
    // Only the summary and the final user message remain active: both turns
    // were retired whole, so no edge can cross the replacement boundary.
    assert_eq!(
        active_ids(&history),
        vec!["conv-1-summary-1".to_owned(), "u2".to_owned()]
    );
    let _ = projection;
}

/// Candidate selection is deterministic: the same plan twice.
#[test]
fn candidate_selection_is_deterministic() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        user("u2", ""),
        assistant("a2", vec![call_block("c2")]),
        tool_message("t2", "c2"),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let first = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    let second = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
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
    let history = state(vec![
        user("huge", ""),
        user("small1", ""),
        user("small2", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.span.end, MessageId::new("huge"));
}

// ---------------------------------------------------------------------------
// Recent suffix retention
// ---------------------------------------------------------------------------

/// The retained suffix approximates the recent-token target.
#[test]
fn retained_suffix_approximates_the_recent_target() {
    let engine = engine(1_000, 0, 25, weighted(10, 10, 10));
    let history = state(vec![
        user("u1", ""),
        user("u2", ""),
        user("u3", ""),
        user("u4", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // Latest boundary retaining at least 25 tokens: retire one, keep three.
    assert_eq!(plan.span.end, MessageId::new("u1"));
    assert_eq!(plan.planned_estimate_after, 30);
}

/// Structural safety wins over the recent-token target: a would-be cut
/// inside a turn is skipped and the whole turn is retained.
#[test]
fn structural_rule_may_force_extra_retention() {
    let engine = engine(1_000, 0, 20, scripted(10, 10, 10, &[("t1", 100)]));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        user("u2", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // The naive "keep the last two messages" cut would retire a1 but keep
    // t1, separating the call from its result; the valid cut retains the
    // whole turn (130 tokens) even though that exceeds the target.
    assert_eq!(plan.span.end, MessageId::new("u1"));
}

/// A token target may force retaining fewer messages when one message
/// dominates the token budget.
#[test]
fn token_target_may_retain_fewer_messages_than_recent() {
    let engine = engine(1_000, 0, 20, scripted(10, 10, 10, &[("big", 500)]));
    let history = state(vec![
        user("big", ""),
        user("m1", ""),
        user("m2", ""),
        user("m3", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // Target 20: retire the huge message and the next one, keeping exactly
    // the two recent small messages.
    assert_eq!(plan.span.end, MessageId::new("m1"));
}

// ---------------------------------------------------------------------------
// System authority
// ---------------------------------------------------------------------------

/// Trusted System content is never replaced by a runtime summary and never
/// enters the summary input; the summary itself is a runtime inbound user
/// message, never elevated to `System`.
#[test]
fn system_messages_are_never_replaced_or_summarized() {
    let engine = engine(300, 0, 0, weighted(100, 10, 100));
    let system_block = system("sys-1", "Trusted: be concise. Byte-for-byte.");
    let mut history = state(vec![
        system_block.clone(),
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    assert_eq!(projection.estimated_input.input_tokens, 310);
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(
        plan.span.start,
        MessageId::new("u1"),
        "the span skips system content"
    );
    assert!(
        !plan
            .retired
            .iter()
            .any(|message| matches!(message, MessageBlock::System(_))),
        "the system message never enters the summary input"
    );
    compact(
        &engine,
        &mut history,
        "summary",
        CompactionBudgets::new(0, 0, 1_000_000),
    )
    .expect("compact");
    // The surface leads with the trusted system message, byte-for-byte,
    // followed by the runtime summary.
    let active = history.active_messages().expect("hydrate");
    assert_eq!(&active[0], &system_block);
    assert!(matches!(
        &active[1],
        MessageBlock::User(user)
            if user.kind == InboundKind::CompactionSummary
                && user.source == UserSource::Runtime
    ));
    assert_eq!(active.len(), 2);
}

/// A later `System` message does not pin every older conversational message
/// and never resurrects a previously retired Surface span.
///
/// This is the direct regression for the bounded Issue #54 System rule.
#[test]
fn a_later_system_message_pins_nothing_and_resurrects_nothing() {
    let engine = engine(10_000, 0, 0, weighted(10, 10, 0));
    let mut history = state(vec![
        system("sys-1", "first"),
        user("u1", ""),
        user("u2", ""),
        user("u3", ""),
    ]);
    compact(
        &engine,
        &mut history,
        "s1",
        CompactionBudgets::new(0, 0, 1_000_000),
    )
    .expect("first compaction");
    assert_eq!(
        active_ids(&history),
        vec!["sys-1".to_owned(), summary_id(1).as_str().to_owned()]
    );

    // A later system message arrives, followed by more conversation.
    history
        .commit(system("sys-2", "second"))
        .expect("commit later system");
    history.commit(user("u4", "")).expect("commit u4");
    history.commit(user("u5", "")).expect("commit u5");
    assert_eq!(
        active_ids(&history),
        vec![
            "sys-1".to_owned(),
            summary_id(1).as_str().to_owned(),
            "sys-2".to_owned(),
            "u4".to_owned(),
            "u5".to_owned(),
        ],
        "a later system message never resurrects retired surface history"
    );

    // The still-active summary remains compactable: the later system
    // message pins nothing older than itself.
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("the older run is still compactable");
    assert_eq!(plan.span.start, summary_id(1));
    assert_eq!(plan.span.end, summary_id(1));
    assert!(
        !history
            .active_ids()
            .iter()
            .any(|id| matches!(id.as_str(), "u1" | "u2" | "u3")),
        "retired ledger history stays retired"
    );
    // Every original is still an addressable ledger fact.
    for id in ["u1", "u2", "u3"] {
        assert!(
            history.ledger().get(&MessageId::new(id)).is_some(),
            "retired message {id} stays addressable in the ledger"
        );
    }
}

/// If pinned context alone prevents fitting, compaction fails explicitly.
#[test]
fn pinned_context_alone_cannot_fit_fails_explicitly() {
    let engine = engine(120, 0, 5, scripted(10, 10, 10, &[("sys-1", 130)]));
    let history = state(vec![
        system("sys-1", "trusted"),
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let error = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect_err("cannot fit");
    assert_eq!(error.kind, ContextErrorKind::CannotFit);
}

// ---------------------------------------------------------------------------
// Complete-message compaction and repeated compaction
// ---------------------------------------------------------------------------

/// Compaction operates on complete canonical messages only: a giant tool
/// result is retired intact with its owning turn, never split, and the whole
/// span reaches the summarizer.
#[test]
fn oversized_material_is_retired_as_complete_messages() {
    let engine = engine(60, 0, 5, scripted(10, 10, 10, &[("t1", 1_000)]));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.span.start, MessageId::new("u1"));
    assert_eq!(plan.span.end, MessageId::new("t1"));
    // The giant tool result is retired intact, as a complete canonical
    // message.
    assert!(
        plan.retired
            .iter()
            .any(|message| matches!(message, MessageBlock::Tool(tool) if tool.id.as_str() == "t1"))
    );
    assert_eq!(plan.summary_request().retired, plan.retired);
}

/// A single oversized message that must stay active produces an explicit
/// `CannotFit`, never a half-message Surface node.
///
/// The oversized fresh inbound message may not be retired, and no
/// complete-message span leaves a fitting request, so planning fails rather
/// than compiling a partial message.
#[test]
fn a_single_oversized_message_cannot_fit_instead_of_splitting() {
    let engine = engine(60, 0, 5, scripted(10, 10, 10, &[("huge", 1_000)]));
    let history = state(vec![user("u1", ""), user("huge", "")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let fresh = rustx::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("huge")])
        .expect("fresh turn");
    let error = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints {
                must_cover_through: None,
                fresh_inbound: Some(&fresh),
            },
        )
        .expect_err("no complete-message span fits");
    assert_eq!(error.kind, ContextErrorKind::CannotFit);
}

/// The planner applies the summary-model limit to the assembled summary
/// input, rather than to the number of retired canonical messages.
#[test]
fn a_span_never_exceeds_the_summary_model_input_limit() {
    let engine = engine(10_000, 0, 0, weighted(10, 10, 0));
    let history = state(vec![user("u1", ""), user("u2", ""), user("u3", "")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    // The deterministic assembly is one wrapper message, so the complete
    // summary request weighs ten tokens under this estimator.
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 25),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.span.end, MessageId::new("u3"));
    assert_eq!(plan.summary_input_tokens, 10);
    // With no room for even one message, planning fails explicitly.
    let error = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 9),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect_err("no span fits the summary model");
    assert_eq!(error.kind, ContextErrorKind::CannotFit);
}

/// The summary-model bound is measured over the exact assembled User input,
/// not over the raw retired message serialization. Wrapper overhead can make a
/// raw span fit while the production summary request does not.
#[test]
fn summary_input_bound_accounts_for_instruction_json_and_wrapper_overhead() {
    let estimator = DefaultTokenEstimator;
    let engine = engine(1_000_000, 0, 0, Arc::new(estimator));
    let history = state(vec![user("u1", "raw retired content")]);
    let request = SummaryRequest {
        retired: vec![user("u1", "raw retired content")],
    };
    let raw_projection = rustx::context::ContextProjection {
        surface_revision: history.revision(),
        messages: request.retired.clone(),
        effective_system_prompt: String::new(),
        estimated_input: TokenMeasurement {
            input_tokens: 0,
            source: TokenMeasurementSource::Estimated,
        },
    };
    let raw_tokens = estimator.estimate_conversation_input(&raw_projection);
    let assembled = request.model_input();
    let assembled_projection = rustx::context::ContextProjection {
        messages: assembled.messages.clone(),
        ..raw_projection.clone()
    };
    let actual_tokens = estimator.estimate_conversation_input(&assembled_projection);
    assert!(
        actual_tokens > raw_tokens,
        "the canonical wrapper must cost tokens"
    );
    assert!(
        raw_tokens < actual_tokens,
        "the raw retired span must fit the deliberately one-token-too-small limit"
    );

    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let before_ids = history.active_ids().to_vec();
    let before_revision = history.revision();
    let before_ledger_len = history.ledger().len();
    let rejected = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, actual_tokens - 1),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect_err("raw fit must not hide the assembled request overflow");
    assert_eq!(rejected.kind, ContextErrorKind::CannotFit);
    assert_eq!(history.active_ids(), before_ids.as_slice());
    assert_eq!(history.revision(), before_revision);
    assert_eq!(history.ledger().len(), before_ledger_len);

    let accepted = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, actual_tokens),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("the exact assembled limit accepts the candidate");
    assert_eq!(accepted.summary_input_tokens, actual_tokens);
    assert_eq!(accepted.summary_request().model_input(), assembled);
}

/// A summary request that cannot fit its actual model input is rejected
/// before summary generation or retry state changes. The overflowing primary
/// request remains the only provider request, and the conversation stays
/// unchanged.
#[tokio::test]
async fn summary_model_cannot_fit_leaves_execution_uncommitted() {
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(overflow_event()),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "must not run".to_owned(),
    )]));
    let runtime = ContextRuntime::with_scripted_summarizer(
        engine(500, 0, 0, weighted(10, 10, 0)),
        summarizer.clone(),
        rustx::context::AgentStatusComposer::default(),
        CompactionBudgets::new(1, 1, 9),
    );
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;

    assert!(
        summarizer.requests().is_empty(),
        "the impossible summary is never invoked"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "no overflow retry follows CannotFit"
    );
    assert!(result.events.iter().any(|event| matches!(
        event,
        RuntimeEvent::CompactionFailed { error } if error.contains("no complete-message surface span")
    )));
    assert!(
        !result
            .events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
    );
    assert_no_compaction_committed(&result);
}

/// Repeated compaction operates from the **current** Surface and never
/// rediscovers retired Ledger history.
///
/// ```text
/// ledger:  A B C D            surface: A B C D
/// first:   A B C D S1         surface: S1 D
/// grow:    A B C D S1 E F     surface: S1 D E F
/// second:  A B C D S1 E F S2  surface: S2 F
/// ```
#[test]
fn repeated_compaction_never_resurrects_retired_history() {
    let engine = engine(10_000, 0, 0, weighted(10, 10, 0));
    let budgets = CompactionBudgets::new(0, 0, 1_000_000);
    let mut history = state(vec![
        user("A", ""),
        user("B", ""),
        user("C", ""),
        user("D", ""),
    ]);

    // First compaction: A B C -> S1, D retained.
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let first = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            budgets,
            &rustx::context::CompactionConstraints {
                must_cover_through: None,
                fresh_inbound: None,
            },
        )
        .expect("first plan");
    // Force the documented A B C -> S1 D shape by naming the span
    // explicitly; the planner's own choice is asserted separately.
    let _ = first;
    let summary1 = UserMessageBlock {
        id: summary_id(1),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "S1".to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::CompactionSummary,
        timestamp: None,
    };
    let commit = history
        .prepare_compaction(
            summary1,
            SurfaceSpan::new(MessageId::new("A"), MessageId::new("C")),
        )
        .expect("prepare first");
    let record1 = history.commit_compaction(commit).expect("commit first");
    assert_eq!(record1.generation, 1);
    assert_eq!(
        active_ids(&history),
        vec![summary_id(1).as_str().to_owned(), "D".to_owned()]
    );
    assert_eq!(
        ledger_ids(&history),
        vec![
            "A".to_owned(),
            "B".to_owned(),
            "C".to_owned(),
            "D".to_owned(),
            summary_id(1).as_str().to_owned(),
        ]
    );

    // The conversation grows.
    history.commit(user("E", "")).expect("commit E");
    history.commit(user("F", "")).expect("commit F");
    assert_eq!(
        active_ids(&history),
        vec![
            summary_id(1).as_str().to_owned(),
            "D".to_owned(),
            "E".to_owned(),
            "F".to_owned(),
        ]
    );

    // Second compaction: the plan is derived from the current Surface, so
    // its span starts at the active S1 — never at the retired A.
    let projection2 = engine
        .build_projection(&history, &[], None, "")
        .expect("second projection");
    let second = engine
        .plan_compaction(
            &history,
            &projection2,
            &[],
            budgets,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("second plan");
    assert_eq!(second.span.start, summary_id(1));
    let retired_ids: Vec<String> = second.retired.iter().map(message_id_of).collect();
    assert!(
        !retired_ids
            .iter()
            .any(|id| matches!(id.as_str(), "A" | "B" | "C")),
        "the second compaction must not rediscover retired ledger history, got {retired_ids:?}"
    );
    // The still-active previous summary is simply one canonical message of
    // the selected span; there is no separate previous-summary channel.
    assert!(
        second
            .retired
            .iter()
            .any(|message| matches!(message, MessageBlock::User(user)
                if user.kind == InboundKind::CompactionSummary)),
        "the previous summary is an ordinary canonical message of the span"
    );

    let summary2 = UserMessageBlock {
        id: summary_id(2),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "S2".to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::CompactionSummary,
        timestamp: None,
    };
    let commit2 = history
        .prepare_compaction(
            summary2,
            SurfaceSpan::new(summary_id(1), MessageId::new("E")),
        )
        .expect("prepare second");
    let record2 = history.commit_compaction(commit2).expect("commit second");
    assert_eq!(record2.generation, 2);
    assert_eq!(
        active_ids(&history),
        vec![summary_id(2).as_str().to_owned(), "F".to_owned()]
    );
    assert_eq!(
        ledger_ids(&history),
        vec![
            "A".to_owned(),
            "B".to_owned(),
            "C".to_owned(),
            "D".to_owned(),
            summary_id(1).as_str().to_owned(),
            "E".to_owned(),
            "F".to_owned(),
            summary_id(2).as_str().to_owned(),
        ],
        "every committed fact survives both compactions"
    );

    // Historical reconstruction is exact and stable.
    assert_eq!(
        history
            .reconstruct(record1.surface_revision)
            .expect("reconstruct generation 1"),
        vec![summary_id(1), MessageId::new("D")]
    );
    assert_eq!(
        history
            .reconstruct(SurfaceRevision::new(4))
            .expect("reconstruct the pre-compaction surface"),
        vec![
            MessageId::new("A"),
            MessageId::new("B"),
            MessageId::new("C"),
            MessageId::new("D"),
        ]
    );
    // The surface operation log carries only the minimal vocabulary.
    assert_eq!(
        history
            .surface()
            .ops()
            .iter()
            .filter(|op| matches!(op, SurfaceOp::Replace { .. }))
            .count(),
        2
    );
}

/// The keyed Ledger reads one full projection + plan + prepare cycle needs
/// over a conversation with `retired` retired messages and five active ones.
///
/// The helper asserts the hard invariant on the way: the cycle performs zero
/// full-Ledger enumerations.
fn finite_reads_for(
    retired: usize,
    engine: &ContextEngine,
    budgets: CompactionBudgets,
) -> (u64, u64, u64, u64) {
    let mut history = state(
        (0..retired + 4)
            .map(|index| user(&format!("m{index}"), ""))
            .collect(),
    );
    // Retire everything but the final four messages.
    let summary = UserMessageBlock {
        id: summary_id(1),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "S".to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::CompactionSummary,
        timestamp: None,
    };
    let commit = history
        .prepare_compaction(
            summary,
            SurfaceSpan::new(
                MessageId::new("m0"),
                MessageId::new(format!("m{}", retired - 1)),
            ),
        )
        .expect("prepare");
    history.commit_compaction(commit).expect("commit");
    assert_eq!(history.active_ids().len(), 5);

    // Only from here is the instrumentation meaningful.
    history.ledger_access().reset();
    history.surface_access().reset();
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    assert_eq!(projection.messages.len(), 5);
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            budgets,
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    let (_, _) = engine
        .prepare_compaction(&history, &conversation(), &plan, "S2", &[])
        .expect("prepare the semantic commit");
    assert_eq!(
        history.ledger_access().enumerations(),
        0,
        "normal projection and compaction must never enumerate the ledger"
    );
    (
        history.ledger_access().keyed_reads(),
        history.surface_access().current_head_reads(),
        history.surface_access().history_enumerations(),
        history.surface_access().history_steps(),
    )
}

/// Normal current-Surface projection, planning, and preparation never
/// enumerate the Message Ledger or Surface history and never depend on
/// retired-history size.
///
/// The proof is a deterministic instrumentation counter, not a memory
/// measurement: `LedgerAccess::enumerations` and Surface historical reads
/// must stay at zero, while keyed/current-head work is a function of the
/// active Surface alone.
#[test]
fn normal_compaction_reads_only_the_current_surface() {
    let engine = engine(10_000_000, 0, 0, weighted(10, 10, 0));
    let budgets = CompactionBudgets::new(0, 0, 1_000_000);

    let small = finite_reads_for(20, &engine, budgets);
    let large = finite_reads_for(2_000, &engine, budgets);
    assert_eq!(
        small, large,
        "the read cost is a function of the active surface alone, not of retired history"
    );
}

// ---------------------------------------------------------------------------
// The compaction semantic commit
// ---------------------------------------------------------------------------

/// The first compaction commits exactly one canonical runtime summary and
/// exactly one Surface replacement, with correct token provenance and an
/// untouched Message Ledger prefix.
#[test]
fn first_compaction_commits_one_summary_and_one_replacement() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
        user("u2", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.estimated_before.input_tokens, 310);
    assert_eq!(
        plan.estimated_before.source,
        TokenMeasurementSource::Estimated
    );
    let mut history = history;
    let before_revision = history.revision();
    let ledger_before = ledger_ids(&history);
    let (commit, rebuilt) = engine
        .prepare_compaction(&history, &conversation(), &plan, "s1", &[])
        .expect("prepare the semantic commit");
    // Preparation mutates nothing.
    assert_eq!(history.revision(), before_revision);
    assert_eq!(ledger_ids(&history), ledger_before);
    assert_eq!(commit.summary().id, summary_id(1));
    assert_eq!(commit.summary().source, UserSource::Runtime);
    assert_eq!(commit.summary().kind, InboundKind::CompactionSummary);
    assert_eq!(rebuilt.estimated_input.input_tokens, 101);
    assert_eq!(rebuilt.surface_revision, before_revision.next());

    let record = history.commit_compaction(commit).expect("commit");
    assert_eq!(record.generation, 1);
    assert_eq!(record.summary_message_id, summary_id(1));
    assert_eq!(record.surface_revision, before_revision.next());
    assert_eq!(record.replaced, plan.span);
    // Exactly one ledger append and exactly one surface replacement.
    assert_eq!(
        ledger_ids(&history),
        vec![
            "u1".to_owned(),
            "a1".to_owned(),
            "t1".to_owned(),
            "u2".to_owned(),
            summary_id(1).as_str().to_owned(),
        ],
        "compaction appends one canonical fact and rewrites nothing"
    );
    assert_eq!(
        history
            .surface()
            .ops()
            .iter()
            .filter(|op| matches!(op, SurfaceOp::Replace { .. }))
            .count(),
        1
    );
    // The summary is active exactly at the replaced span's position.
    assert_eq!(
        active_ids(&history),
        vec![summary_id(1).as_str().to_owned(), "u2".to_owned()]
    );
    // Every covered original is still an immutable, addressable ledger fact.
    assert!(matches!(
        history.ledger().get(&MessageId::new("a1")),
        Some(MessageBlock::Assistant(_))
    ));
}

/// The second compaction selects a span of the **current** Surface: the
/// still-active previous summary is simply one canonical message inside it,
/// and already-retired originals are never re-fed.
#[test]
fn second_compaction_selects_from_the_current_surface() {
    let engine = engine(10_000, 0, 150, weighted(100, 10, 100));
    let history = state(vec![
        user("u1", ""),
        user("u2", ""),
        user("u3", ""),
        user("u4", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let first = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("first plan");
    assert_eq!(first.span.end, MessageId::new("u2"));
    let mut history = history;
    let (commit1, _) = engine
        .prepare_compaction(&history, &conversation(), &first, "s1", &[])
        .expect("prepare first");
    history.commit_compaction(commit1).expect("commit first");
    assert_eq!(
        active_ids(&history),
        vec![
            summary_id(1).as_str().to_owned(),
            "u3".to_owned(),
            "u4".to_owned(),
        ]
    );
    history.commit(user("u5", "")).expect("commit u5");
    history.commit(user("u6", "")).expect("commit u6");

    let projection2 = engine
        .build_projection(&history, &[], None, "")
        .expect("second projection");
    let second = engine
        .plan_compaction(
            &history,
            &projection2,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("second plan");
    let request = second.summary_request();
    let selected: Vec<String> = request.retired.iter().map(newly_retired_id).collect();
    // Retired originals of the first compaction are never re-fed.
    for retired in ["u1", "u2"] {
        assert!(
            !selected.contains(&retired.to_owned()),
            "retired ledger history must never be rediscovered, got {selected:?}"
        );
    }
    assert_eq!(selected[0], summary_id(1).as_str());
    let (commit2, _) = engine
        .prepare_compaction(&history, &conversation(), &second, "s2", &[])
        .expect("prepare second");
    let record2 = history.commit_compaction(commit2).expect("commit second");
    assert_eq!(record2.generation, 2);
    assert_eq!(record2.summary_message_id, summary_id(2));
}

/// A summary at least as large as the replaced context makes no progress:
/// no canonical summary, no Surface rewrite, explicit error.
#[test]
fn no_progress_compaction_is_rejected() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![user("u1", "")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    // A summary text of 400 bytes estimates 101 tokens >= the 100 before.
    let error = engine
        .prepare_compaction(&history, &conversation(), &plan, &"x".repeat(400), &[])
        .expect_err("no progress");
    assert_eq!(error.kind, ContextErrorKind::NoProgress);
    assert_eq!(history.revision(), SurfaceRevision::new(1));
    assert_eq!(history.ledger().len(), 1, "nothing was committed");
}

/// The anti-loop progress rule never compares a provider-reported
/// before-count against an estimated after-count: both sides of the
/// comparison are deterministic estimates of the actual projection content.
/// A provider-reported number far above the deterministic estimate must not
/// mask an estimate that grew.
#[test]
fn progress_rule_rejects_growth_even_when_provider_reported_before_is_larger() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = state(vec![user("u1", ""), assistant("a1", vec![text_block("x")])]);
    // Provider-reported before = 1000; the deterministic estimate of the
    // same projection is 20.
    let plain_projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: plain_projection.fingerprint(),
        input_tokens: 1_000,
    };
    let projection = engine
        .build_projection(&history, &[], Some(&observed), "")
        .expect("provider-reported projection");
    assert_eq!(
        projection.estimated_input.source,
        TokenMeasurementSource::ProviderReported
    );
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
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
        .prepare_compaction(&history, &conversation(), &plan, &"x".repeat(120), &[])
        .expect_err("no progress");
    assert_eq!(error.kind, ContextErrorKind::NoProgress);
}

/// The reverse direction: a provider-reported before-count below the
/// deterministic estimate must not reject a compaction whose estimated
/// after-count decreased relative to the deterministic before.
#[test]
fn progress_rule_accepts_decrease_even_when_provider_reported_before_is_smaller() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = state(vec![
        user("u1", ""),
        user("u2", ""),
        user("u3", ""),
        user("u4", ""),
        user("u5", ""),
        user("u6", ""),
    ]);
    // Provider-reported before = 50; the deterministic estimate of the same
    // projection is 60.
    let plain_projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let observed = ProviderObservedInput {
        fingerprint: plain_projection.fingerprint(),
        input_tokens: 50,
    };
    let projection = engine
        .build_projection(&history, &[], Some(&observed), "")
        .expect("provider-reported projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    assert_eq!(plan.estimated_before_tokens, 60);
    // The after estimate (50) decreased from the deterministic before (60)
    // but is above the provider-reported 50-boundary of this test's before
    // measurement: progress must be accepted. A 200-byte summary weighs
    // exactly 50 tokens under the corrected ceiling division.
    let mut history = history;
    let (commit, rebuilt) = engine
        .prepare_compaction(&history, &conversation(), &plan, &"x".repeat(200), &[])
        .expect("progress accepted");
    let record = history.commit_compaction(commit).expect("commit");
    assert_eq!(record.generation, 1);
    assert_eq!(
        plan.estimated_before,
        TokenMeasurement {
            input_tokens: 50,
            source: TokenMeasurementSource::ProviderReported,
        },
        "the provider-reported measurement is preserved as plan metadata"
    );
    assert_eq!(rebuilt.estimated_input.input_tokens, 50);
}

/// Empty and whitespace-only summaries are rejected at the application
/// boundary: no summarizer can erase history through an empty summary.
#[test]
fn empty_and_whitespace_summaries_are_rejected() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = state(vec![user("u1", ""), user("u2", "")]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("plan");
    for bad in ["", "   ", "\n\t "] {
        let error = engine
            .prepare_compaction(&history, &conversation(), &plan, bad, &[])
            .expect_err("empty summary must be rejected");
        assert_eq!(error.kind, ContextErrorKind::SummaryFailed);
    }
    assert_eq!(history.revision(), SurfaceRevision::new(2));
}

// ---------------------------------------------------------------------------
// Continuation constraint
// ---------------------------------------------------------------------------

/// The continuation constraint retires the continuation-owning turn
/// completely; the owning Assistant message is never split.
#[test]
fn continuation_constraint_covers_the_owning_turn_completely() {
    let engine = engine(200, 0, 5, weighted(100, 10, 100));
    let history = state(vec![
        user("u1", ""),
        assistant("a1", vec![call_block("c1")]),
        tool_message("t1", "c1"),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints {
                must_cover_through: Some(&MessageId::new("a1")),
                fresh_inbound: None,
            },
        )
        .expect("plan");
    assert_eq!(plan.span.end, MessageId::new("t1"));
    // The continuation-owning Assistant message and its complete tool-result
    // portion are both retired into the summary input.
    let retired_ids: Vec<String> = plan.retired.iter().map(newly_retired_id).collect();
    assert!(retired_ids.contains(&"a1".to_owned()));
    assert!(retired_ids.contains(&"t1".to_owned()));
}

/// A continuation-owning oversized turn is retired whole, never split.
#[test]
fn continuation_owner_is_never_split() {
    let engine = engine(60, 0, 5, weighted(10, 10, 10));
    let history = state(vec![
        user("u1", ""),
        assistant(
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
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    // Without the constraint this turn would split (see the split test);
    // with the constraint it must be retired whole.
    let plan = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints {
                must_cover_through: Some(&MessageId::new("a1")),
                fresh_inbound: None,
            },
        )
        .expect("plan");
    assert_eq!(plan.span.end, MessageId::new("t2"));
}

/// When the continuation-owning turn lies outside the current compactable
/// run, the constraint is unsatisfiable: compaction cannot retire the owner,
/// so the plan fails explicitly instead of clearing the continuation while
/// leaving its message active.
#[test]
fn continuation_owner_outside_the_compactable_run_is_unsatisfiable() {
    let engine = engine(1_000, 0, 0, weighted(10, 10, 0));
    let history = state(vec![
        user("u1", ""),
        system("sys-2", "trusted"),
        assistant("a1", vec![text_block("x")]),
        user("u2", ""),
    ]);
    let projection = engine
        .build_projection(&history, &[], None, "")
        .expect("projection");
    let error = engine
        .plan_compaction(
            &history,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints {
                must_cover_through: Some(&MessageId::new("a1")),
                fresh_inbound: None,
            },
        )
        .expect_err("a continuation owner outside the compactable run cannot be retired");
    assert_eq!(error.kind, ContextErrorKind::NoProgress);
    assert!(
        error.message.contains("compactable region"),
        "the error explains the constraint: {}",
        error.message
    );
    // The same conversation without the intervening system message
    // satisfies the constraint: the check is specific to the compactable
    // run, not to age.
    let unpinned = state(vec![
        user("u1", ""),
        assistant("a1", vec![text_block("x")]),
        user("u2", ""),
    ]);
    let projection = engine
        .build_projection(&unpinned, &[], None, "")
        .expect("projection");
    let plan = engine
        .plan_compaction(
            &unpinned,
            &projection,
            &[],
            CompactionBudgets::new(0, 0, 1_000_000),
            &rustx::context::CompactionConstraints {
                must_cover_through: Some(&MessageId::new("a1")),
                fresh_inbound: None,
            },
        )
        .expect("the continuation owner is retired");
    assert!(
        plan.retired.iter().any(
            |message| matches!(message, MessageBlock::Assistant(assistant) if assistant.id.as_str() == "a1")
        ),
        "the continuation-owning turn is covered by the span"
    );
    assert_eq!(plan.span.end, MessageId::new("u2"));
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
    let model = fake_model(vec![
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
    let summarizer =
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
    let runtime = runtime_with(200, 0, 5, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;

    let summary_id_committed = committed_summary(&result).id.clone();
    let (surface_revision, compaction_tokens_before, compaction_estimated_after) = result
        .events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::CompactionCompleted {
                surface_revision,
                tokens_before,
                estimated_tokens_after,
                ..
            } => Some((*surface_revision, *tokens_before, *estimated_tokens_after)),
            _ => None,
        })
        .expect("the attempt emitted one compaction completion");
    let expected = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-1"),
        },
        RuntimeEvent::TurnStarted,
        RuntimeEvent::ModelRequestStarted {
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::AssistantMessageStarted {
            message_id: assistant_message_id(1),
        },
        RuntimeEvent::ToolCallStarted {
            message_id: assistant_message_id(1),
            block_index: ContentBlockIndex::new(0),
            call: call_start(),
        },
        RuntimeEvent::ToolCallArgumentsDelta {
            message_id: assistant_message_id(1),
            block_index: ContentBlockIndex::new(0),
            call_id: ToolCallId::new("call-1"),
            arguments_delta: "{}".to_owned(),
        },
        RuntimeEvent::ToolCallCompleted {
            message_id: assistant_message_id(1),
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
        RuntimeEvent::AssistantMessageCommitted {
            message_id: assistant_message_id(1),
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
        RuntimeEvent::ToolMessageCommitted {
            message_id: MessageId::new("attempt-1-tool-1-call-1"),
            tool_call_id: ToolCallId::new("call-1"),
        },
        RuntimeEvent::TurnCompleted,
        RuntimeEvent::TurnStarted,
        RuntimeEvent::CompactionStarted,
        RuntimeEvent::CompactionCompleted {
            generation: 1,
            summary_message_id: summary_id_committed.clone(),
            surface_revision,
            tokens_before: compaction_tokens_before,
            estimated_tokens_after: compaction_estimated_after,
        },
        RuntimeEvent::ModelRequestStarted {
            request_id: RequestId::new("request:9:attempt-1:1:2:0"),
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::AssistantMessageStarted {
            message_id: assistant_message_id(2),
        },
        RuntimeEvent::AssistantTextDelta {
            message_id: assistant_message_id(2),
            block_index: ContentBlockIndex::new(0),
            delta: "answer".to_owned(),
        },
        RuntimeEvent::ModelRequestCompleted {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
        RuntimeEvent::AssistantMessageCommitted {
            message_id: assistant_message_id(2),
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
        MessageBlock::Assistant(assistant) if assistant.id.as_str() == "attempt-1-agent-1"
    ));
    assert!(matches!(
        &requests[1].messages[2],
        MessageBlock::Tool(tool) if tool.id.as_str() == "attempt-1-tool-1-call-1"
    ));

    // The Message Ledger keeps every original fact and gains exactly one
    // canonical runtime compaction summary; nothing is rewritten.
    assert_eq!(
        result
            .messages()
            .iter()
            .map(message_id_of)
            .collect::<Vec<_>>(),
        vec![
            "msg-user-1".to_owned(),
            "attempt-1-agent-1".to_owned(),
            "attempt-1-tool-1-call-1".to_owned(),
            summary_id(1).as_str().to_owned(),
            "attempt-1-agent-2".to_owned(),
        ]
    );
    assert_eq!(result.messages()[0].clone(), user("msg-user-1", "hi"));
    assert_eq!(committed_summary(&result).id, summary_id(1));
    // The Conversation Surface replaced exactly the compacted span.
    assert_eq!(
        result
            .active_ids()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec![
            summary_id(1).as_str().to_owned(),
            "attempt-1-agent-1".to_owned(),
            "attempt-1-tool-1-call-1".to_owned(),
            "attempt-1-agent-2".to_owned(),
        ],
        "the summary replaced exactly the selected span at its position"
    );
    // Exactly one Surface replacement was applied.
    assert_eq!(result.conversation.surface().compaction_generation(), 1);
}

/// Below the threshold, the loop never compacts and preserves M3 behavior.
#[tokio::test]
async fn below_threshold_runs_without_compaction() {
    let scripted = scripted_call();
    let model = fake_model(vec![
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
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Return("s".to_owned())]);
    let runtime = runtime_with(10_000, 0, 5, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
    assert_no_compaction_committed(&result);
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
    let model = fake_model(vec![
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
    let summarizer =
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;

    let summary_id_committed = committed_summary(&result).id.clone();
    let (surface_revision, compaction_tokens_before, compaction_estimated_after) = result
        .events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::CompactionCompleted {
                surface_revision,
                tokens_before,
                estimated_tokens_after,
                ..
            } => Some((*surface_revision, *tokens_before, *estimated_tokens_after)),
            _ => None,
        })
        .expect("the attempt emitted one compaction completion");
    let expected = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-1"),
        },
        RuntimeEvent::TurnStarted,
        RuntimeEvent::ModelRequestStarted {
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::AssistantMessageStarted {
            message_id: assistant_message_id(1),
        },
        RuntimeEvent::AssistantTextDelta {
            message_id: assistant_message_id(1),
            block_index: ContentBlockIndex::new(0),
            delta: "provisional".to_owned(),
        },
        RuntimeEvent::ModelRequestFailed {
            error: overflow_error(),
        },
        RuntimeEvent::CompactionStarted,
        RuntimeEvent::CompactionCompleted {
            generation: 1,
            summary_message_id: summary_id_committed.clone(),
            surface_revision,
            tokens_before: compaction_tokens_before,
            estimated_tokens_after: compaction_estimated_after,
        },
        RuntimeEvent::ModelRetryScheduled {
            attempt_number: 1,
            retry_delay_ms: None,
        },
        RuntimeEvent::ModelRequestStarted {
            request_id: RequestId::new("request:9:attempt-1:1:1:1"),
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::AssistantMessageStarted {
            message_id: retry_message_id(1),
        },
        RuntimeEvent::AssistantTextDelta {
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
        RuntimeEvent::AssistantMessageCommitted {
            message_id: retry_message_id(1),
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

    // The retry request uses the smaller projection with the canonical
    // runtime summary and no continuation.
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        &requests[1].messages[0],
        MessageBlock::User(user) if user.kind == InboundKind::CompactionSummary
    ));
    assert_eq!(requests[1].messages.len(), 1);
    assert_eq!(requests[1].continuation, None);
    // Only the successful invocation is committed; the ledger additionally
    // carries the canonical compaction summary.
    assert_eq!(
        result
            .messages()
            .iter()
            .map(message_id_of)
            .collect::<Vec<_>>(),
        vec![
            "msg-user-1".to_owned(),
            summary_id(1).as_str().to_owned(),
            retry_message_id(1).as_str().to_owned(),
        ]
    );
}

/// An overflow retry reuses one admitted Context Assembly generation. The
/// contributor, native context sampling, and ordering are never rerun; only
/// the historical Surface revision changes because compaction committed.
#[tokio::test]
async fn overflow_retry_reuses_the_admitted_context_generation() {
    let model = fake_model(vec![
        vec![FakeStep::Emit(started()), FakeStep::Emit(overflow_event())],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let invocations = Arc::new(AtomicUsize::new(0));
    let invocations_for_contributor = Arc::clone(&invocations);
    let mut assembly = ContextAssembly::new();
    assembly
        .register_extension(
            "certified.test",
            Some("package-generation-1".to_owned()),
            Arc::new(move |_: &rustx::context::ContributorInputSnapshot| {
                invocations_for_contributor.fetch_add(1, Ordering::SeqCst);
                Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "frozen extension context".to_owned(),
                    })],
                })])
            }),
        )
        .expect("register extension contributor");
    let runtime = runtime_with_assembly(
        500,
        0,
        5,
        weighted(100, 10, 0),
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]),
        assembly,
    );
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;

    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert_eq!(result.request_snapshots().len(), 2);
    assert_eq!(
        result.request_snapshots()[0].context_generation,
        result.request_snapshots()[1].context_generation
    );
    assert_ne!(
        result.request_snapshots()[0].surface_revision,
        result.request_snapshots()[1].surface_revision,
        "compaction changes the retry Surface revision"
    );

    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(
            request
                .messages
                .iter()
                .filter(|message| {
                    matches!(
                        message,
                        MessageBlock::User(user)
                            if user.kind == InboundKind::Context(
                                rustx::message::types::ContextKind::ExtensionEnvironment
                            )
                    )
                })
                .count(),
            1,
            "the retry reuses one canonical extension fact"
        );
    }
    assert_eq!(
        result
            .messages()
            .iter()
            .filter(|message| {
                matches!(
                    message,
                    MessageBlock::User(user)
                        if user.kind == InboundKind::Context(
                            rustx::message::types::ContextKind::ExtensionEnvironment
                        )
                )
            })
            .count(),
        1
    );
    for (request, snapshot) in requests.iter().zip(result.request_snapshots()) {
        assert_eq!(
            snapshot
                .reconstruct(&result.conversation)
                .expect("reconstruct retry request"),
            *request
        );
    }
}

/// `ContextWindowExceeded` is a rejected provider request, so it does not
/// prove that the fresh inbound turn was observed. Overflow compaction must
/// therefore retain the pending inbound while reusing the already-admitted
/// dynamic context generation.
#[tokio::test]
async fn overflow_retry_preserves_pending_fresh_inbound_and_context_generation() {
    let planner = engine(500, 0, 5, weighted(100, 10, 0));
    let candidate_history = state(vec![
        user("old", "old history"),
        fresh_user("msg-inbound-1", "fresh inbound"),
        user("accepted-context-1", "accepted dynamic context"),
        user("accepted-context-2", "another accepted fact"),
    ]);
    let candidate_projection = planner
        .build_projection(&candidate_history, &[], None, "")
        .expect("candidate projection");
    let unconstrained = planner
        .plan_compaction(
            &candidate_history,
            &candidate_projection,
            &[],
            CompactionBudgets::new(1, 1, 1_000_000),
            &rustx::context::CompactionConstraints::default(),
        )
        .expect("an unrestricted candidate crosses fresh inbound");
    assert_eq!(unconstrained.span.end, MessageId::new("accepted-context-1"));
    assert_ne!(
        unconstrained.span.end,
        MessageId::new("old"),
        "the unrestricted candidate crosses the pending fresh inbound"
    );
    let fresh =
        rustx::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("msg-inbound-1")])
            .expect("fresh trigger");
    let protected = planner
        .plan_compaction(
            &candidate_history,
            &candidate_projection,
            &[],
            CompactionBudgets::new(1, 1, 1_000_000),
            &rustx::context::CompactionConstraints {
                must_cover_through: None,
                fresh_inbound: Some(&fresh),
            },
        )
        .expect("fresh inbound leaves an earlier candidate");
    assert_eq!(protected.span.end, MessageId::new("old"));
    assert!(
        !protected
            .retired
            .iter()
            .any(|message| message_id_of(message) == "msg-inbound-1"),
        "the pending fresh identity is not in the overflow summary span"
    );

    let model = fake_model(vec![
        vec![FakeStep::Emit(started()), FakeStep::Emit(overflow_event())],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let invocations = Arc::new(AtomicUsize::new(0));
    let invocations_for_contributor = Arc::clone(&invocations);
    let mut assembly = ContextAssembly::new();
    assembly
        .register_extension(
            "fresh-overflow.test",
            Some("package-generation-1".to_owned()),
            Arc::new(move |_: &rustx::context::ContributorInputSnapshot| {
                invocations_for_contributor.fetch_add(1, Ordering::SeqCst);
                Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "accepted once".to_owned(),
                    })],
                })])
            }),
        )
        .expect("register overflow contributor");
    let runtime = runtime_with_assembly(
        500,
        0,
        5,
        weighted(100, 10, 0),
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]),
        assembly,
    );
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let evaluations = Arc::new(AtomicUsize::new(0));
    let result = AgentExecution::new(
        fresh_request(
            "attempt-1",
            vec![
                user("old", "old history"),
                fresh_user("msg-inbound-1", "fresh inbound"),
            ],
            &model,
        ),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert().with_pre_step_policy(Arc::new(
            CountingPreStepPolicy {
                evaluations: Arc::clone(&evaluations),
            },
        )),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;

    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert_eq!(model.requests().len(), 2);
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert_eq!(
        evaluations.load(Ordering::SeqCst),
        1,
        "an overflow retry is not a new model-step admission: the pre-step \
         policy is not re-evaluated"
    );
    assert_eq!(result.request_snapshots().len(), 2);
    assert_eq!(
        result.request_snapshots()[0].context_generation,
        result.request_snapshots()[1].context_generation,
        "overflow retry reuses the admitted ContextGeneration"
    );
    assert_ne!(
        result.request_snapshots()[0].surface_revision,
        result.request_snapshots()[1].surface_revision,
        "successful overflow compaction creates a new historical Surface revision"
    );
    let retry = &model.requests()[1];
    assert!(
        retry.messages.iter().any(|message| {
            matches!(
                message,
                MessageBlock::User(user) if user.id == MessageId::new("msg-inbound-1")
            )
        }),
        "the retry still presents pending fresh inbound"
    );
    assert!(
        result
            .active_ids()
            .contains(&MessageId::new("msg-inbound-1")),
        "fresh inbound remains active until the successful retry observes it"
    );
    assert_eq!(
        result
            .messages()
            .iter()
            .filter(|message| {
                matches!(
                    message,
                    MessageBlock::User(user)
                        if user.kind == InboundKind::Context(
                            rustx::message::types::ContextKind::AgentStatus
                        )
                )
            })
            .count(),
        1,
        "Agent Status is sampled and committed once"
    );
    assert_eq!(
        result
            .messages()
            .iter()
            .filter(|message| {
                matches!(
                    message,
                    MessageBlock::User(user)
                        if user.kind == InboundKind::Context(
                            rustx::message::types::ContextKind::ExtensionEnvironment
                        )
                )
            })
            .count(),
        1,
        "dynamic extension context is committed once"
    );
    for (request, snapshot) in model.requests().iter().zip(result.request_snapshots()) {
        assert_eq!(
            snapshot
                .reconstruct(&result.conversation)
                .expect("reconstruct overflow request"),
            *request
        );
    }
}

/// A second overflow after the retry settles the attempt with the second
/// overflow; no second compaction and no third request occur.
#[tokio::test]
async fn overflow_retry_exhausted_after_one_retry() {
    let model = fake_model(vec![
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
    let summarizer =
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
    let model = fake_model(vec![
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
    let summarizer =
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
        result.messages().len(),
        3,
        "input + the canonical compaction summary + one committed Assistant message"
    );
    let MessageBlock::Assistant(assistant) = &result.messages()[2] else {
        panic!("the committed message must be the retry Assistant message");
    };
    assert_eq!(
        assistant.id,
        retry_message_id(1),
        "the committed message carries the retry identity"
    );
    let texts: Vec<String> = assistant
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["RETRY".to_owned()], "exactly the retry output");
    let serialized = serde_json::to_string(result.messages()).expect("serialize messages");
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
    let model = fake_model(vec![
        first,
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "plain answer")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = tool_registry_with_alpha();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let summarizer =
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
            .messages()
            .iter()
            .all(|message| !matches!(message, MessageBlock::Tool(_))),
        "no tool message is committed for the failed request's call"
    );
    let MessageBlock::Assistant(assistant) = &result.messages()[2] else {
        panic!("the committed message must be the retry Assistant message");
    };
    assert_eq!(assistant.id, retry_message_id(1));
    assert_eq!(assistant.content.len(), 1, "only the retry text block");
}

/// The overflow retry budget is genuinely per model turn: both turns are
/// entitled to their own single retry, and the budget never persists across
/// turns.
#[tokio::test]
async fn overflow_retry_budget_is_per_model_turn() {
    let scripted = scripted_call();
    let model = fake_model(vec![
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
    let summarizer = FakeContextSummarizer::new(vec![
        FakeSummaryStep::Return("summary-1".to_owned()),
        FakeSummaryStep::Return("summary-2".to_owned()),
    ]);
    let runtime = runtime_with(500, 0, 0, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
    // The committed Message Ledger holds both turns, each with the retry
    // identity of its own turn, plus the two canonical compaction summaries.
    assert_eq!(
        result
            .messages()
            .iter()
            .map(message_id_of)
            .collect::<Vec<_>>(),
        vec![
            "msg-user-1".to_owned(),
            summary_id(1).as_str().to_owned(),
            retry_message_id(1).as_str().to_owned(),
            "attempt-1-tool-1-call-1".to_owned(),
            summary_id(2).as_str().to_owned(),
            retry_message_id(2).as_str().to_owned(),
        ],
        "one canonical summary per compaction, nothing rewritten"
    );
    assert_eq!(result.conversation.surface().compaction_generation(), 2);
}

/// An invalid (empty or whitespace-only) summary from a custom/fake
/// summarizer fails the compaction: no canonical summary is committed, no
/// Surface rewrite happens, and no overflow retry follows.
#[tokio::test]
async fn invalid_summary_fails_without_commit_or_retry() {
    for bad_summary in ["", "   "] {
        let model = fake_model(vec![vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(overflow_event()),
        ]]);
        let tools = ToolRegistry::new();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let summarizer =
            FakeContextSummarizer::new(vec![FakeSummaryStep::Return(bad_summary.to_owned())]);
        let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer);
        let tool_runtime = common::tool_runtime("conv-1");
        let capability = common::capability_lease(tools, &tool_runtime).await;
        let result = AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
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
                .all(|event| !matches!(event, RuntimeEvent::CompactionCompleted { .. })),
            "no compaction may be committed"
        );
        assert!(
            result
                .events
                .iter()
                .all(|event| !matches!(event, RuntimeEvent::ModelRetryScheduled { .. })),
            "no overflow retry may follow an invalid summary"
        );
        assert_no_compaction_committed(&result);
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
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "provisional")),
        FakeStep::Emit(overflow_event()),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Fail(ContextError::new(
        ContextErrorKind::SummaryFailed,
        "summary generation refused",
    ))]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
    assert_no_compaction_committed(&result);
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
fn fresh_request(
    attempt: &str,
    initial_messages: Vec<MessageBlock>,
    model: &Arc<FakeModel>,
) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id: conversation(),
        attempt_id: AttemptId::new(attempt),
        conversation: state(initial_messages),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::FreshInbound(
            rustx::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("msg-inbound-1")])
                .expect("valid fresh turn"),
        ),
        timezone: None,
        model: support::attempt_model_with_window(model.clone(), "fake-model", 10_000_000, 1),
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
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "ok")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut composer = rustx::context::AgentStatusComposer::new(Arc::new(FixedClock(fixed_time())));
    composer
        .register(Arc::new(FailingStatusProvider))
        .expect("register");
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        fresh_request(
            "attempt-1",
            vec![fresh_user("msg-inbound-1", "deploy it")],
            &model,
        ),
        capability.into_lease(),
        &cancellation,
        rustx::context::ContextRuntime::with_scripted_summarizer(
            engine(10_000_000, 0, 0, weighted(10, 10, 10)),
            Arc::new(FakeContextSummarizer::new(Vec::new())),
            composer,
            CompactionBudgets::new(1, 1, 1_000_000),
        ),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "ok")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
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
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        fresh_request("attempt-1", initial, &model),
        capability.into_lease(),
        &cancellation,
        runtime_with(250, 0, 0, weighted(100, 10, 0), summarizer),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
    assert_no_compaction_committed(&result);
}

/// A no-progress compaction (summary not smaller than the replaced
/// context) fails explicitly: no canonical summary, no Surface rewrite, no
/// retry, no loop, one terminal event.
#[tokio::test]
async fn no_progress_compaction_fails_without_retry() {
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "provisional")),
        FakeStep::Emit(overflow_event()),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    // 400 bytes estimate 101 tokens >= the 100-token replaced context.
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Return("x".repeat(400))]);
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
            .any(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. })),
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
    assert_no_compaction_committed(&result);
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

/// Cancellation before proactive compaction begins: no `CompactionStarted`,
/// no summary, no canonical commit, no Surface rewrite, no retry.
#[tokio::test]
async fn cancel_before_proactive_compaction() {
    let scripted = scripted_call();
    let model = fake_model(vec![vec![
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
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Return("s".to_owned())]);
    // Window 200: turn 1 fits (100 tokens), but after turn 1 the history
    // (210 tokens) would require proactive compaction at turn 2 — which
    // never starts because cancellation settles the attempt first.
    let runtime = runtime_with(200, 0, 5, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let execution = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
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
    assert_no_compaction_committed(&result);
}

/// Cancellation while the summary is parked (after summary generation
/// began, before the semantic commit): the pending summary future is
/// dropped, no completion, no failure, no retry, and neither a
/// half-committed summary nor a half-applied Surface rewrite exists.
#[tokio::test]
async fn cancel_while_summary_generation_is_pending() {
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "provisional")),
        FakeStep::Emit(overflow_event()),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::ParkUntilCancelled]);
    let parked = summarizer.parked();
    let runtime = runtime_with(500, 0, 5, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let execution = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
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
            RuntimeEvent::CompactionCompleted { .. } | RuntimeEvent::CompactionFailed { .. }
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
    assert_no_compaction_committed(&result);
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
    let model = fake_model(vec![
        turn1,
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "final")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let tools = tool_registry_with_alpha();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let runtime = runtime_with(window, 0, 5, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
                MessageBlock::Assistant(assistant) if assistant.id == assistant_message_id(1)
            )),
            "the continuation-owning Assistant message may not remain literal"
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
/// tools, no continuation, the resolved summary invocation's
/// model/protocol/output budget, and deterministic input.
#[tokio::test]
async fn model_backed_summarizer_issues_a_canonical_request() {
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "summary ")),
        FakeStep::Emit(text_delta(0, "text")),
        FakeStep::Emit(done_with_usage(ModelFinishReason::Stop, 9)),
    ]]);
    let summarizer = ModelBackedSummarizer::new(summary_invocation(&model, 128));
    let request = SummaryRequest {
        retired: vec![user("u1", "hi")],
    };
    let text = summarizer
        .summarize(request.clone(), rustx::runtime::CancellationSignal::new())
        .await
        .expect("summary");
    assert_eq!(text, "summary text");

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model(), "fake-model");
    assert_eq!(
        requests[0].protocol(),
        rustx::model::ModelProtocol::OpenAiChatCompletions
    );
    assert_eq!(requests[0].max_output_tokens(), 128);
    assert!(requests[0].tools.is_empty());
    assert_eq!(requests[0].effective_system_prompt, "");
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
    assert_eq!(requests[0].messages, request.model_input().messages);
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
        let model = fake_model(vec![events]);
        let summarizer = ModelBackedSummarizer::new(summary_invocation(&model, 64));
        let request = SummaryRequest {
            retired: vec![user("u1", "hi")],
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
        let model = fake_model(vec![events]);
        let summarizer = ModelBackedSummarizer::new(summary_invocation(&model, 64));
        let request = SummaryRequest {
            retired: vec![user("u1", "hi")],
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
        let model = fake_model(vec![events]);
        let summarizer = ModelBackedSummarizer::new(summary_invocation(&model, 64));
        let request = SummaryRequest {
            retired: vec![user("u1", "hi")],
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
    let model = fake_model(vec![vec![FakeStep::ParkUntilCancelled]]);
    let summarizer = ModelBackedSummarizer::new(summary_invocation(&model, 64));
    let cancellation = rustx::runtime::CancellationSignal::new();
    let request = SummaryRequest {
        retired: vec![user("u1", "hi")],
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
    let model = fake_model(vec![
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
    // One attempt snapshot drives both the loop and the summary: the
    // production `for_attempt` path derives the engine window and the summary
    // invocation from exactly the model the attempt was admitted with.
    let snapshot = support::attempt_model_with_window(model.clone(), "fake-model", 500, 1);
    let runtime = ContextRuntime::for_attempt(
        rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 5,
            summary_output_cap: None,
        },
        weighted(100, 10, 0),
        rustx::context::AgentStatusComposer::default(),
        &snapshot,
    )
    .expect("runtime");
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let mut attempt_request = request("attempt-1", vec![user("msg-user-1", "hi")], 1, &model);
    attempt_request.model = snapshot;
    let result = AgentExecution::new(
        attempt_request,
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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

/// Invalidating the incompatible opaque provider continuation has exactly
/// one ownership path.
///
/// A successful incompatible Surface rewrite must discard the continuation
/// exactly once, immediately after the semantic commit. The M4 loop cleared
/// it from two caller sites as well; this regression keeps that duplicate
/// from returning.
#[test]
fn continuation_invalidation_has_exactly_one_ownership_path() {
    let source = std::fs::read_to_string("src/agent/execution.rs").expect("read the agent loop");
    let body = source
        .split_once("#[cfg(test)]\nmod tests {")
        .map_or(source.as_str(), |(body, _)| body);
    assert_eq!(
        body.matches("self.pending_continuation = None;").count(),
        1,
        "the opaque provider continuation must be invalidated from exactly one place"
    );
    assert_eq!(
        body.matches("self.continuation_owner = None;").count(),
        2,
        "the continuation owner is set from the turn assembly and cleared once \
         by the post-surface-rewrite ownership path"
    );
}

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

fn newly_retired_id(item: &MessageBlock) -> String {
    message_id_of(item)
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
        MessageBlock::Assistant(assistant) => assistant.id.to_string(),
        MessageBlock::Tool(tool) => tool.id.to_string(),
    }
}

/// Scripts a two-turn model whose first turn parks (with a released text
/// delta) and completes with Stop; the second turn completes immediately.
fn parked_two_turn_model(release: tokio::sync::watch::Receiver<bool>) -> Arc<FakeModel> {
    fake_model(vec![
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
    );
    let tool_runtime = common::tool_runtime("conv-1");
    let mailbox = tool_runtime.mailbox().clone();
    let controller = controller_enqueue_a_and_b(&model, &mailbox, release);
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-u0", "start")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
    let ids: Vec<String> = result.messages().iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec![
            "msg-u0".to_owned(),
            assistant_message_id(1).to_string(),
            "msg-inbound-a".to_owned(),
            "msg-inbound-b".to_owned(),
            "rustx-context-attempt-1-turn-2-4".to_owned(),
            assistant_message_id(2).to_string(),
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
            assistant_message_id(1).to_string(),
            "msg-inbound-a".to_owned(),
            "msg-inbound-b".to_owned(),
            "rustx-context-attempt-1-turn-2-4".to_owned(),
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
    // per-message = 100, per-block = 10, window = 350:
    // before the drain the projection is 100 tokens (below the threshold);
    // after the drain [u0, agent, A, B, Agent Status] is 410 tokens
    // (at/above it), so the drained batch deterministically triggers
    // proactive compaction while retaining the complete fresh inbound batch.
    let summarizer = FakeContextSummarizer::new(vec![FakeSummaryStep::Return("S".to_owned())]);
    let runtime = runtime_with(350, 0, 0, weighted(100, 10, 0), summarizer);
    let tool_runtime = common::tool_runtime("conv-1");
    let mailbox = tool_runtime.mailbox().clone();
    let controller = controller_enqueue_a_and_b(&model, &mailbox, release);
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-u0", "start")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
    // The Message Ledger still contains the original inbound
    // `UserMessageBlock`s even though the active Surface was rewritten; the
    // canonical runtime summary joins them as one more committed fact.
    let ids: Vec<String> = result.messages().iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec![
            "msg-u0".to_owned(),
            assistant_message_id(1).to_string(),
            "msg-inbound-a".to_owned(),
            "msg-inbound-b".to_owned(),
            "rustx-context-attempt-1-turn-2-4".to_owned(),
            summary_id(1).to_string(),
            assistant_message_id(2).to_string(),
        ],
        "the ledger preserves the drained inbound messages and gains the summary"
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
            "rustx-context-attempt-1-turn-2-4".to_owned(),
        ],
        "the continuation request uses the compacted projection with the \
         unobserved fresh inbound preserved literally"
    );
    // Exactly one Agent Status snapshot accompanies the fresh inbound turn,
    // and it is now a canonical Runtime context fact with a core-owned id.
    let status_messages = requests[1]
        .messages
        .iter()
        .filter_map(|message| match message {
            MessageBlock::User(user)
                if user.kind
                    == rustx::message::types::InboundKind::Context(
                        rustx::message::types::ContextKind::AgentStatus,
                    ) =>
            {
                Some(user)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(status_messages.len(), 1, "one canonical status fact");
    let status_text = match &status_messages[0].content[0] {
        UserContentBlock::Text(text) => &text.text,
        _ => panic!("status context is text"),
    };
    assert!(
        status_text.contains("<system-reminder>") && status_text.contains("Inbound message time"),
        "the rendered status is committed through Context Assembly"
    );
    let serialized = serde_json::to_string(&requests[1].messages).expect("serialize");
    assert!(
        serialized.contains('S'),
        "the summary reaches the projection"
    );
    // The committed canonical summary is a derived compaction summary: no
    // fabricated wall-clock timestamp.
    assert!(
        matches!(
            committed_summary(&result),
            UserMessageBlock {
                source: UserSource::Runtime,
                kind: InboundKind::CompactionSummary,
                timestamp: None,
                ..
            }
        ),
        "a compaction summary never carries a fabricated timestamp"
    );
    // Agent Status is now an admitted canonical Runtime fact. It therefore
    // appears in the Ledger exactly once; compaction may project it away only
    // by replacing a complete active span with the canonical summary.
    let ledger_serialized = serde_json::to_string(result.messages()).expect("serialize");
    assert!(
        ledger_serialized.contains("<system-reminder>"),
        "the admitted Agent Status fact must remain in the Message Ledger"
    );
}

/// Without compaction, an ordinary inbound drain retains the pending
/// provider continuation through the M4 projection path.
#[tokio::test]
async fn m4_drain_retains_continuation_without_compaction() {
    let state = anthropic_state();
    let (release, parked) = model_release();
    let model = fake_model(vec![
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
    );
    let tool_runtime = common::tool_runtime("conv-1");
    let mailbox = tool_runtime.mailbox().clone();
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
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = AgentExecution::new(
        request("attempt-1", vec![user("msg-u0", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
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
