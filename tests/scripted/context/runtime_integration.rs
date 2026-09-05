//! Runtime integration: `AgentExecution` / `ConversationRuntime` composition
//! with the context plane.
//!
//! Every test here drives a real `AgentExecution` with the `ContextRuntime`
//! bundle over scripted fixture models, tools, and summarizers, and asserts
//! the **observable boundary contract** of the composition:
//!
//! - proactive compaction is requested before the next model request;
//! - provider overflow triggers the one allowed compact-and-retry path, with
//!   its budget, replay, and settlement rules at the attempt boundary;
//! - context preparation/compaction errors are classified correctly at the
//!   Agent Loop boundary;
//! - cancellation at the boundary prevents a later request and settles the
//!   attempt structurally;
//! - provider continuation is preserved, invalidated exactly once by a
//!   committed rewrite, and never fabricated;
//! - the adapter-backed summarizer never contaminates the attempt's request
//!   state;
//! - drained inbound batches join the projection/compaction correctly.
//!
//! The internal compaction pipeline state machine (plan -> summarize ->
//! validate -> durable commit -> install, failure atomicity) is owned by
//! `compaction_pipeline.rs`; the provider-independent planning/projection
//! semantics are owned by `engine.rs`. Nothing here re-proves those
//! lower-layer contracts beyond their observable boundary effect.
//! Multi-attempt `ConversationRuntime` composition (request reconstruction,
//! client detach/reattach, session-model freeze) lives in
//! `runtime_multi_compaction.rs`.

use super::super::{common, support};

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionRequest, AgentExecutionResult,
};
use rustx::context::{
    CompactionBudgets, ContextAssembly, ContextConfig, ContextEngine, ContextError,
    ContextErrorKind, ContextProposal, ContextRuntime, TokenEstimator, UserMessageProposal,
};
use rustx::conversation::{ConversationState, summary_message_id};
use rustx::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, ContentBlockIndex, ContextKind, InboundKind, MessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::ModelInputMessage;
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
use rustx::runtime::types::CancellationReason;
use rustx::tools::executor::ToolRegistry;
use support::context::{FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator};
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, fake_model, model_release, success_result,
    tool_call_events,
};

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
            retry_disposition: rustx::model::error::ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
            timeout_phase: None,
            generation: None,
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
        retry_disposition: rustx::model::error::ModelRetryDisposition::Never,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
        malformed_tool_proposal: None,
        timeout_phase: None,
        generation: None,
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

use support::audit::{assert_outcome, assert_single_terminal, assert_trace};

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
) -> ContextRuntime {
    ContextRuntime::with_scripted_summarizer(
        engine(window, reserve, keep_recent, estimator),
        Arc::new(summarizer),
        rustx::context::AgentStatusEngine::new(
            rustx::context::AgentStatusConfig::default(),
            Arc::new(FixedClock(fixed_time())),
        ),
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
        rustx::context::AgentStatusEngine::new(
            rustx::context::AgentStatusConfig::default(),
            Arc::new(FixedClock(fixed_time())),
        ),
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
    matches!(message, MessageBlock::User(user) if user.kind.is_compaction_summary())
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
            MessageBlock::User(user) if user.kind.is_compaction_summary() => Some(user),
            _ => None,
        })
        .expect("a successful compaction commits one canonical summary")
}

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

/// A fresh-inbound request: the first turn carries a pending fresh inbound
/// turn, so the optional `FreshInbound` Agent Status opportunity is eligible.
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

async fn run_continuation_case(
    emit_continuation: bool,
    state: ProviderContinuationState,
    window: u64,
    summarizer: FakeContextSummarizer,
) -> (common::DurableExecutionAudit, Vec<ModelRequest>) {
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );
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

/// One conversation state bootstrapped from ordered canonical messages.
fn state(messages: Vec<MessageBlock>) -> ConversationState {
    ConversationState::from_messages(messages).expect("bootstrap conversation")
}

fn conversation() -> ConversationId {
    ConversationId::new("conv-1")
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

fn message_id_of(message: &MessageBlock) -> String {
    match message {
        MessageBlock::User(user) => user.id.as_str().to_owned(),
        MessageBlock::Assistant(assistant) => assistant.id.as_str().to_owned(),
        MessageBlock::Tool(tool) => tool.id.as_str().to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Context assembly
// ---------------------------------------------------------------------------

fn summary_id(generation: u64) -> MessageId {
    summary_message_id(&conversation(), generation)
}

// ---------------------------------------------------------------------------
// Proactive compaction at the attempt boundary
// ---------------------------------------------------------------------------

/// Proactive compaction uses the complete frozen `ContextAssembly`, including
/// extension System sections, before it selects a Surface span. Otherwise a
/// candidate can pass baseline accounting and fail again after staging the
/// real request-time System authority.
#[tokio::test]
async fn proactive_compaction_accounts_for_frozen_extension_system_sections() {
    let extension_guidance = "[baseline-extension-system-guidance]\n".repeat(6);
    let mut assembly = ContextAssembly::new();
    let identity = assembly
        .register_extension(
            "baseline.extension",
            Some("generation-1".to_owned()),
            Arc::new(|_: &rustx::context::ContributorInputSnapshot| Ok(Vec::new())),
        )
        .expect("register extension");
    assembly
        .register_extension_system_section(&identity, extension_guidance.clone())
        .expect("register extension System section");

    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "answer")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "baseline summary".to_owned(),
    )]));
    let runtime = ContextRuntime::with_scripted_summarizer_and_assembly(
        engine(250, 0, 100, weighted(100, 10, 0)),
        summarizer.clone(),
        rustx::context::AgentStatusEngine::default(),
        assembly,
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let result = common::durable_agent_result(
        AgentExecution::new(
            request(
                "attempt-1",
                vec![
                    user("old", "old history"),
                    user("middle", "middle history"),
                    user("recent", "recent history"),
                ],
                1,
                &model,
            ),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );

    let summary_requests = summarizer.requests();
    assert_eq!(summary_requests.len(), 1);
    assert_eq!(
        summary_requests[0]
            .retired
            .iter()
            .map(message_id_of)
            .collect::<Vec<_>>(),
        vec!["old", "middle"],
        "the full frozen System authority makes the one-message candidate fail"
    );
    assert_eq!(
        result
            .conversation
            .active_ids()
            .iter()
            .map(MessageId::as_str)
            .collect::<Vec<_>>(),
        vec!["conv-1-summary-1", "recent", "attempt-1-agent-1"]
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .effective_system_prompt
            .contains(&extension_guidance)
    );
}

/// A long history compacts proactively before the next model request: the
/// trace order is `TurnStarted`, `CompactionStarted`, `CompactionCompleted`,
/// `ModelRequestStarted`; the request receives the projection, and the
/// committed result remains canonical history.
#[tokio::test]
#[allow(clippy::too_many_lines)] // the full expected trace is asserted verbatim
async fn proactive_compaction_before_the_next_turn() {
    // The first request's reported usage is load-bearing: it becomes the
    // anchor of the next turn's measurement, so it is scripted to agree with
    // this estimator's view of the same context (one User message at weight
    // 100). The pressure that triggers compaction therefore comes from the
    // assistant turn and tool result appended after it.
    let scripted = scripted_call();
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(tool_call_events(0, &scripted)[0].clone()),
            FakeStep::Emit(tool_call_events(0, &scripted)[1].clone()),
            FakeStep::Emit(tool_call_events(0, &scripted)[2].clone()),
            FakeStep::Emit(done_with_usage(ModelFinishReason::ToolCalls, 100)),
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    let summary_id_committed = committed_summary(&result).id.clone();
    let (surface_revision, compaction_tokens_before, compaction_estimated_after) = result
        .event_history
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
        RuntimeEvent::ModelRequestCompleted {
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            finish_reason: ModelFinishReason::ToolCalls,
            usage: Some(ModelUsage {
                input_tokens: 100,
                output_tokens: 4,
                total_tokens: 104,
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
        RuntimeEvent::ModelRequestCompleted {
            request_id: RequestId::new("request:9:attempt-1:1:2:0"),
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
    assert_trace(&result.event_history, &expected);
    assert_single_terminal(&result.event_history);
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
    let Some(MessageBlock::User(summary)) = requests[1].messages[0].as_canonical() else {
        panic!("first projected message must be the summary");
    };
    assert_eq!(summary.id, summary_id(1));
    assert_eq!(summary.source, UserSource::Runtime);
    assert!(summary.kind.is_compaction_summary());
    assert!(matches!(
        requests[1].messages[1].as_canonical(),
        Some(MessageBlock::Assistant(assistant)) if assistant.id.as_str() == "attempt-1-agent-1"
    ));
    assert!(matches!(
        requests[1].messages[2].as_canonical(),
        Some(MessageBlock::Tool(tool)) if tool.id.as_str() == "attempt-1-tool-1-call-1"
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    assert!(
        !result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted)),
        "no compaction below the threshold"
    );
    assert_single_terminal(&result.event_history);
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    let summary_id_committed = committed_summary(&result).id.clone();
    let (surface_revision, compaction_tokens_before, compaction_estimated_after) = result
        .event_history
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
        RuntimeEvent::ModelRequestFailed {
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            error: overflow_error(),
            usage: None,
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
            failed_request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            retry_number: 1,
            retry_delay_ms: None,
        },
        RuntimeEvent::ModelRequestStarted {
            request_id: RequestId::new("request:9:attempt-1:1:1:1"),
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::ModelRequestCompleted {
            request_id: RequestId::new("request:9:attempt-1:1:1:1"),
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
    assert_trace(&result.event_history, &expected);
    assert_single_terminal(&result.event_history);
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
        requests[1].messages[0].as_canonical(),
        Some(MessageBlock::User(user)) if user.kind.is_compaction_summary()
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
/// Issue #12 (M9b): the overflow retry reaches the same start gate. The
/// compaction commits as its own independent durable fact; cancelling
/// immediately before the retry's start arbitration is again
/// cancellation-before-start — the retry never starts, no second
/// `ModelRequestStarted`, no second provider request — while the committed
/// compaction remains.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_before_overflow_retry_start_stops_the_retry() {
    use crate::agent::execution::test_sync::StartBoundaryPause;
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
    let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
    let mut pre_start = pre_start.expect("pre-start phase installed");
    let mut execution = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_start_boundary_pause(pause);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        // Request #1 reaches its start arbitration: let it start; it ends
        // with the overflow. Compaction then commits independently.
        pre_start.await_park(1).await;
        pre_start.release();
        // The retry reaches the same gate: cancel before its start.
        pre_start.await_park(2).await;
        controller_cancellation.cancel();
        pre_start.release();
    });
    let result =
        common::durable_agent_result(execution.run().await, tool_runtime.durable_store().as_ref());
    controller.await.expect("controller task");

    assert_eq!(model.requests().len(), 1, "the retry request never started");
    assert_eq!(
        result
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        1,
        "exactly one ModelRequestStarted: the retry start never committed"
    );
    assert!(
        result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. })),
        "the compaction committed as its own independent durable fact"
    );
    let summary = committed_summary(&result);
    assert!(
        matches!(
            summary.content.first(),
            Some(rustx::message::types::UserContentBlock::Text(text)) if text.text == "summary-1"
        ),
        "the summary remains canonical even though the retry never started"
    );
    assert!(
        matches!(
            result.outcome,
            AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested
            }
        ),
        "the attempt settles cancelled before the retry start: {:?}",
        result.outcome
    );
    assert!(
        matches!(
            result.event_history.last(),
            Some(RuntimeEvent::AttemptCancelled { .. })
        ) && result
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
            .count()
            == 1,
        "the cancellation terminal is unique and last"
    );
}

/// Issue #12 (M9b), Finding 1: when the staged request-scoped context
/// overflows the soft limit during model-turn preparation, the compaction
/// that makes room for it commits as its own independent durable fact. A
/// cancellation that then wins the start arbitration keeps the committed
/// compaction and discards the staged context without a trace: no provider
/// request, no `ModelRequestStarted`, no canonical request-scoped context.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_after_preparation_compaction_keeps_it_and_discards_staged_context() {
    use crate::agent::execution::test_sync::StartBoundaryPause;
    let model = fake_model(vec![]);
    let mut assembly = ContextAssembly::new();
    assembly
        .register_extension(
            "certified.test",
            Some("package-v1".to_owned()),
            Arc::new(|_: &rustx::context::ContributorInputSnapshot| {
                Ok(vec![
                    ContextProposal::UserMessage(UserMessageProposal {
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: "staged context a".to_owned(),
                        })],
                    }),
                    ContextProposal::UserMessage(UserMessageProposal {
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: "staged context b".to_owned(),
                        })],
                    }),
                ])
            }),
        )
        .expect("register extension contributor");
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let summarizer =
        FakeContextSummarizer::new(vec![FakeSummaryStep::Return("summary-1".to_owned())]);
    let runtime = runtime_with_assembly(250, 0, 5, weighted(100, 10, 0), summarizer, assembly);
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
    let mut pre_start = pre_start.expect("pre-start phase installed");
    let mut execution = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_start_boundary_pause(pause);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        // The staged context overflows the soft limit, so the preparation
        // compaction commits before the start arbitration; the execution
        // then parks at the gate. Cancel there: the compaction stays, the
        // staged context is discarded.
        pre_start.await_park(1).await;
        controller_cancellation.cancel();
        pre_start.release();
    });
    let result =
        common::durable_agent_result(execution.run().await, tool_runtime.durable_store().as_ref());
    controller.await.expect("controller task");

    assert_eq!(
        model.requests().len(),
        0,
        "the provider request never started"
    );
    assert_eq!(
        result
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        0,
        "no request-start fact exists"
    );
    assert!(
        result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. })),
        "the preparation compaction committed as its own independent durable fact"
    );
    assert!(
        matches!(
            committed_summary(&result).content.first(),
            Some(rustx::message::types::UserContentBlock::Text(text)) if text.text == "summary-1"
        ),
        "the committed compaction summary remains canonical"
    );
    // Only the original inbound and the compaction summary are canonical:
    // the staged request-scoped context never became canonical.
    assert_eq!(
        result
            .messages()
            .iter()
            .map(message_id_of)
            .collect::<Vec<_>>(),
        vec!["msg-user-1".to_owned(), summary_id(1).as_str().to_owned()],
        "only the original inbound and the compaction summary are canonical; the staged context was discarded"
    );
    assert!(
        result.messages().iter().all(|message| !matches!(
            message,
            MessageBlock::User(user) if matches!(user.kind, InboundKind::Context(_))
        )),
        "no request-scoped context became canonical"
    );
    assert!(
        matches!(
            result.outcome,
            AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested
            }
        ),
        "the attempt settles cancelled before start: {:?}",
        result.outcome
    );
    assert!(
        matches!(
            result.event_history.last(),
            Some(RuntimeEvent::AttemptCancelled { .. })
        ) && result
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
            .count()
            == 1,
        "the cancellation terminal is unique and last"
    );
}

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
    let publication = common::RecordingPublicationObserver::default();
    let mut execution = AgentExecution::new(
        request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
        capability.into_lease(),
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.observe(&publication);
    let result = common::durable_agent_result_with_publication(
        execution.run().await,
        tool_runtime.durable_store().as_ref(),
        &publication,
    );

    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert_eq!(result.snapshot_history().len(), 2);
    assert_eq!(
        result.snapshot_history()[0].context_generation,
        result.snapshot_history()[1].context_generation
    );
    assert_ne!(
        result.snapshot_history()[0].surface_revision,
        result.snapshot_history()[1].surface_revision,
        "compaction changes the retry Surface revision"
    );
    let opened = publication.opened();
    assert_eq!(opened.len(), 2, "each provider request owns one stream");
    assert_ne!(
        opened[0].message_id, opened[1].message_id,
        "the retry freezes its own provisional Assistant identity"
    );
    assert_eq!(publication.audits().len(), 1);
    assert_eq!(
        publication.audits()[0].kind,
        rustx::publication::PublicationAuditKind::Incomplete
    );
    assert_eq!(
        publication.trace(),
        vec![
            common::PublicationObservation::Opened(opened[0].stream_id.clone()),
            common::PublicationObservation::Settled(
                opened[0].stream_id.clone(),
                rustx::publication::PublicationAuditKind::Incomplete,
            ),
            common::PublicationObservation::Opened(opened[1].stream_id.clone()),
        ],
        "the abandoned stream settles before the retry stream opens"
    );
    assert!(
        tool_runtime
            .durable_store()
            .load_unsettled_publication_streams()
            .expect("unsettled publication streams")
            .is_empty()
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
                        message.as_canonical(),
                        Some(MessageBlock::User(user))
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
    for (request, snapshot) in requests.iter().zip(result.snapshot_history()) {
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
                ..Default::default()
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
    let result = common::durable_agent_result(
        AgentExecution::new(
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
            crate::scripted_suites::support::default_execution_policy(),
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
        .await,
        tool_runtime.durable_store().as_ref(),
    );

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
    assert_eq!(result.snapshot_history().len(), 2);
    assert_eq!(
        result.snapshot_history()[0].context_generation,
        result.snapshot_history()[1].context_generation,
        "overflow retry reuses the admitted ContextGeneration"
    );
    assert_ne!(
        result.snapshot_history()[0].surface_revision,
        result.snapshot_history()[1].surface_revision,
        "successful overflow compaction creates a new historical Surface revision"
    );
    let retry = &model.requests()[1];
    assert!(
        retry.messages.iter().any(|message| {
            matches!(
                message.as_canonical(),
                Some(MessageBlock::User(user)) if user.id == MessageId::new("msg-inbound-1")
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
                        if matches!(
                            &user.kind,
                            InboundKind::Context(
                                rustx::message::types::ContextKind::AgentStatus(_)
                            )
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
    for (request, snapshot) in model.requests().iter().zip(result.snapshot_history()) {
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    let started = result
        .event_history
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
        .count();
    assert_eq!(started, 2, "exactly two provider requests");
    let retries = result
        .event_history
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
        .count();
    assert_eq!(retries, 1, "exactly one retry");
    let compactions = result
        .event_history
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::CompactionStarted))
        .count();
    assert_eq!(compactions, 1, "no second overflow compaction");

    assert_single_terminal(&result.event_history);
    assert_outcome(
        &result,
        &AttemptOutcome::Failed {
            error: AttemptFailure::Model {
                error: overflow_error(),
            },
        },
    );
    assert!(matches!(
        result.event_history.last(),
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    assert_single_terminal(&result.event_history);
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    assert_single_terminal(&result.event_history);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert!(
        result.event_history.iter().all(|event| {
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    let requests = model.requests();
    assert_eq!(
        requests.len(),
        4,
        "two invocations per turn: request + retry"
    );
    let retries = result
        .event_history
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
        .count();
    assert_eq!(retries, 2, "each turn gets exactly one retry");
    let compactions = result
        .event_history
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::CompactionStarted))
        .count();
    assert_eq!(compactions, 2, "one compaction per overflow");
    assert_single_terminal(&result.event_history);
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
        let result = common::durable_agent_result(
            AgentExecution::new(
                request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
                capability.into_lease(),
                &cancellation,
                crate::scripted_suites::support::default_execution_policy(),
                runtime,
                &tool_runtime,
                rustx::agent::AttemptLifecycle::inert(),
            )
            .expect("conversation identity matches the tool runtime")
            .run()
            .await,
            tool_runtime.durable_store().as_ref(),
        );

        assert_single_terminal(&result.event_history);
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
                .event_history
                .iter()
                .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. })),
            "the invalid summary is a compaction failure"
        );
        assert!(
            result
                .event_history
                .iter()
                .all(|event| !matches!(event, RuntimeEvent::CompactionCompleted { .. })),
            "no compaction may be committed"
        );
        assert!(
            result
                .event_history
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    let expected_tail = vec![
        RuntimeEvent::ModelRequestFailed {
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            error: overflow_error(),
            usage: None,
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
        &result.event_history[result.event_history.len() - expected_tail.len()..],
        &expected_tail
    );
    assert_single_terminal(&result.event_history);
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

// ---------------------------------------------------------------------------
// Overflow replanning at the attempt boundary
// ---------------------------------------------------------------------------

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
        rustx::context::AgentStatusEngine::default(),
        CompactionBudgets::new(1, 1, 9),
    );
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    assert!(
        summarizer.requests().is_empty(),
        "the impossible summary is never invoked"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "no overflow retry follows CannotFit"
    );
    assert!(result.event_history.iter().any(|event| matches!(
        event,
        RuntimeEvent::CompactionFailed { error } if error.contains("no complete-message surface span")
    )));
    assert!(
        !result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
    );
    assert_no_compaction_committed(&result);
}

/// A summary request the summary model itself rejects as too large is not a
/// dead end.
///
/// The selected span was *estimated* to fit the summary model's own request
/// budget, and the provider proved that estimate wrong. Abandoning the
/// compaction here is what makes a context overflow unrecoverable: the
/// attempt would report the original overflow again with nothing compacted.
/// Instead the pipeline replans the same transition against a halved summary
/// input budget and summarizes again.
#[tokio::test]
async fn a_rejected_summary_request_replans_against_a_smaller_budget() {
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
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![
        FakeSummaryStep::Fail(ContextError::new(
            ContextErrorKind::SummaryInputTooLarge,
            "the summary request exceeded the summary model context window",
        )),
        FakeSummaryStep::Return("summary-1".to_owned()),
    ]));
    let runtime = ContextRuntime::with_scripted_summarizer(
        engine(500, 0, 5, weighted(100, 10, 0)),
        summarizer.clone(),
        rustx::context::AgentStatusEngine::default(),
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    assert_eq!(
        summarizer.requests().len(),
        2,
        "the rejected summary request is replanned, not abandoned"
    );
    assert!(
        result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. })),
        "the replanned compaction commits: {:?}",
        result.event_history
    );
    assert!(
        !result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. })),
        "a recovered rejection is not a compaction failure"
    );
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

// ---------------------------------------------------------------------------
// Context failure classification at the Agent Loop boundary
// ---------------------------------------------------------------------------

/// A failing Agent Status module is optional context enrichment: it is
/// quarantined for this attempt, the normal context path continues, and the
/// provider still receives exactly one request. The failed module emits no
/// model-visible status and no structured status observation.
#[tokio::test]
async fn failing_status_module_is_quarantined_not_preparation_failure() {
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "ok")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let tools = ToolRegistry::new();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let seam = rustx::context::AgentStatusTestSeam::new();
    seam.fail_evaluate_once(rustx::context::AgentStatusModuleId::Time);
    let status_engine = rustx::context::AgentStatusEngine::new(
        rustx::context::AgentStatusConfig::default(),
        Arc::new(FixedClock(fixed_time())),
    )
    .with_test_seam(seam.clone());
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = common::durable_agent_result(
        AgentExecution::new(
            fresh_request(
                "attempt-1",
                vec![fresh_user("msg-inbound-1", "deploy it")],
                &model,
            ),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            rustx::context::ContextRuntime::with_scripted_summarizer(
                engine(10_000_000, 0, 0, weighted(10, 10, 10)),
                Arc::new(FakeContextSummarizer::new(Vec::new())),
                status_engine,
                CompactionBudgets::new(1, 1, 1_000_000),
            ),
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    assert_eq!(
        model.requests().len(),
        1,
        "status failure does not block the model request"
    );
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(
        seam.capture_count(rustx::context::AgentStatusModuleId::Time),
        1
    );
    assert_eq!(
        seam.evaluate_count(rustx::context::AgentStatusModuleId::Time),
        1
    );
    assert!(
        result
            .conversation
            .active_messages()
            .expect("active messages")
            .iter()
            .all(|message| !matches!(
                message,
                MessageBlock::User(user)
                    if matches!(
                        &user.kind,
                        InboundKind::Context(ContextKind::AgentStatus(_))
                    )
            ))
    );
    let terminals: Vec<&RuntimeEvent> = result
        .event_history
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::AttemptCompleted { .. }))
        .collect();
    assert_eq!(terminals.len(), 1, "exactly one terminal event");
    assert_eq!(
        result.event_history.last(),
        Some(terminals[0]),
        "the terminal event is last"
    );
    assert!(
        result
            .event_history
            .iter()
            .all(|event| !matches!(event, RuntimeEvent::CompactionStarted)),
        "module failure never starts a compaction pipeline"
    );
    assert!(matches!(
        terminals[0],
        RuntimeEvent::AttemptCompleted { .. }
    ));
    assert!(
        result
            .event_history
            .iter()
            .all(|event| !matches!(event, RuntimeEvent::CompactionStarted)),
        "module failure never creates an alternate compaction"
    );
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            fresh_request("attempt-1", initial, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime_with(250, 0, 0, weighted(100, 10, 0), summarizer),
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    assert_eq!(
        model.requests().len(),
        0,
        "no provider request follows a failed proactive compaction"
    );
    assert!(
        result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted)),
        "a proactive compaction pipeline must actually start"
    );
    assert!(
        result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. })),
        "the compaction failure event carries the diagnostic"
    );
    let RuntimeEvent::AttemptFailed { error, .. } = result.event_history.last().expect("terminal")
    else {
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-user-1", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    assert!(
        result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. }))
    );
    assert!(
        !result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. })),
        "no overflow retry after a failed compaction"
    );
    assert!(
        !result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. })),
        "no compaction completion without progress"
    );
    assert_single_terminal(&result.event_history);
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

// ---------------------------------------------------------------------------
// Cancellation at the Agent Loop/context boundary
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
        crate::scripted_suites::support::default_execution_policy(),
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
    let result =
        common::durable_agent_result(execution.run().await, tool_runtime.durable_store().as_ref());
    controller.await.expect("controller task");
    assert_single_terminal(&result.event_history);
    assert!(matches!(
        result.event_history.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
    assert!(
        !result
            .event_history
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
        crate::scripted_suites::support::default_execution_policy(),
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
    let result =
        common::durable_agent_result(execution.run().await, tool_runtime.durable_store().as_ref());
    controller.await.expect("controller task");
    assert_single_terminal(&result.event_history);
    assert!(matches!(
        result.event_history.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
    assert!(
        !result.event_history.iter().any(|event| matches!(
            event,
            RuntimeEvent::CompactionCompleted { .. } | RuntimeEvent::CompactionFailed { .. }
        )),
        "no post-cancel compaction facts"
    );
    assert!(
        !result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. })),
        "no retry after cancellation"
    );
    let started_requests = result
        .event_history
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

// ---------------------------------------------------------------------------
// Continuation policy across a committed rewrite
// ---------------------------------------------------------------------------

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
        assert_single_terminal(&result.event_history);
        assert!(
            !result
                .event_history
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
        assert_single_terminal(&result.event_history);
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].continuation, None,
            "a changed projection must never pair with the old opaque continuation"
        );
        // The continuation-owning turn was fully retired into the summary:
        // the projected request contains no literal part of it.
        assert!(requests[1].messages.iter().any(|message| matches!(
            message.as_canonical(),
            Some(MessageBlock::User(user)) if user.kind.is_compaction_summary()
        )));
        assert!(
            !requests[1].messages.iter().any(|message| matches!(
                message.as_canonical(),
                Some(MessageBlock::Assistant(assistant)) if assistant.id == assistant_message_id(1)
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
    assert_single_terminal(&result.event_history);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].continuation, None);
    assert_eq!(requests[1].continuation, None);
}

// ---------------------------------------------------------------------------
// Adapter-backed summarizer
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Summarizer isolation inside an execution
// ---------------------------------------------------------------------------

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
        rustx::context::AgentStatusEngine::default(),
        &snapshot,
        rustx::model::ModelTimeoutPolicy::default(),
        support::default_monotonic_clock(),
    )
    .expect("runtime");
    let tool_runtime = common::tool_runtime("conv-1");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let mut attempt_request = request("attempt-1", vec![user("msg-user-1", "hi")], 1, &model);
    attempt_request.model = snapshot;
    let result = common::durable_agent_result(
        AgentExecution::new(
            attempt_request,
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );

    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    // The summary request is the canonical one-off: no tools, no
    // continuation.
    assert!(requests[1].tools.is_empty());
    assert_eq!(requests[1].continuation, None);
    // The retry request carries the summary projection and no continuation.
    assert_eq!(requests[2].continuation, None);
    assert!(matches!(
        requests[2].messages[0].as_canonical(),
        Some(MessageBlock::User(user)) if user.kind.is_compaction_summary()
    ));
    assert_single_terminal(&result.event_history);
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

// ---------------------------------------------------------------------------
// Drained inbound batches before projection/compaction (Issue #22)
// ---------------------------------------------------------------------------

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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-u0", "start")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );
    controller.await.expect("controller task");

    assert_single_terminal(&result.event_history);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert!(
        !result
            .event_history
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
    let request_ids: Vec<String> = requests[1]
        .messages
        .iter()
        .filter_map(ModelInputMessage::as_canonical)
        .map(block_id)
        .collect();
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-u0", "start")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );
    controller.await.expect("controller task");

    assert_single_terminal(&result.event_history);
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
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::CompactionStarted))
            .count(),
        1,
        "the drained batch must cross the compaction threshold exactly once"
    );
    // The Message Ledger still contains the original inbound
    // `UserMessageBlock`s even though the active Surface was rewritten; the
    // canonical runtime summary joins them as one more committed fact.
    // Issue #12 (M9b): the request-scoped Agent Status context commits
    // inside the durable model-turn start transaction, which is strictly
    // after the independent compaction commit, so the summary precedes the
    // status fact in commit order.
    let ids: Vec<String> = result.messages().iter().map(block_id).collect();
    assert_eq!(
        ids,
        vec![
            "msg-u0".to_owned(),
            assistant_message_id(1).to_string(),
            "msg-inbound-a".to_owned(),
            "msg-inbound-b".to_owned(),
            summary_id(1).to_string(),
            "rustx-context-attempt-1-turn-2-4".to_owned(),
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
    let request_ids: Vec<String> = requests[1]
        .messages
        .iter()
        .filter_map(ModelInputMessage::as_canonical)
        .map(block_id)
        .collect();
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
        .filter_map(|message| match message.as_canonical() {
            Some(MessageBlock::User(user))
                if matches!(
                    &user.kind,
                    rustx::message::types::InboundKind::Context(
                        rustx::message::types::ContextKind::AgentStatus(_)
                    )
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
        status_text.contains(
            "<system-reminder>\nTimezone: UTC\nCurrent time: 2026-08-07 12:00:00\n</system-reminder>"
        ) && !status_text.contains("Inbound message time"),
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
                kind: InboundKind::CompactionSummary(_),
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
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("msg-u0", "hi")], 0, &model),
            capability.into_lease(),
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .expect("conversation identity matches the tool runtime")
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );
    controller.await.expect("controller task");

    assert_single_terminal(&result.event_history);
    assert!(
        !result
            .event_history
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
            .any(|block| matches!(block.as_canonical(), Some(MessageBlock::User(user)) if user.id == MessageId::new("msg-inbound-a"))),
        "the drained message is part of the projection"
    );
}
