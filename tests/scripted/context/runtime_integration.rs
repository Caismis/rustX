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
use rustx::durable::RequestStore;
use support::model::{FixtureModel, ScriptedAdapterFactory, fixture_session_model};

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::DurableExecutionAudit as AgentExecutionResult;
use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::context::{
    CompactionBudgets, ContextAssembly, ContextConfig, ContextEngine, ContextError,
    ContextErrorKind, ContextProposal, ContextRuntime, TokenEstimator, UserMessageProposal,
};
use rustx::conversation::ConversationState;
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
            input_tokens: Some(input_tokens),
            output_tokens: Some(4),
            total_tokens: Some(input_tokens + 4),
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

fn coded_overflow_error() -> rustx::model::ModelError {
    crate::model::adapter::openai::mapping::normalize_error(
        async_openai::error::OpenAIError::ApiError(async_openai::error::ApiErrorResponse {
            status_code: reqwest::StatusCode::BAD_REQUEST,
            api_error: async_openai::error::ApiError {
                message: "请求被拒绝".into(),
                r#type: Some("invalid_request_error".into()),
                code: Some("context_length_exceeded".into()),
                param: None,
            },
        }),
    )
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
        Some(rustx::context::AgentStatusEngine::new(
            rustx::context::AgentStatusConfig::default(),
            Arc::new(FixedClock(fixed_time())),
        )),
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
        Some(rustx::context::AgentStatusEngine::new(
            rustx::context::AgentStatusConfig::default(),
            Arc::new(FixedClock(fixed_time())),
        )),
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

/// Absence of a durable projection activation proves no compaction committed.
fn assert_no_compaction_committed(result: &AgentExecutionResult) {
    assert!(result.projections.is_empty());
    assert!(
        !result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. }))
    );
}

/// Materialized summary body from the durable historical checkpoint domain.
fn committed_summary(result: &AgentExecutionResult) -> &rustx::model::projected::ProjectedMessage {
    let checkpoint = result
        .projections
        .last()
        .expect("successful compaction has a durable checkpoint");
    checkpoint
        .candidate
        .items
        .iter()
        .find_map(|item| match &item.message {
            rustx::context::checkpoint::CheckpointMessage::Derived(message)
                if message
                    .transformations
                    .contains(&rustx::model::projected::SemanticTransformation::Summary) =>
            {
                Some(&message.message)
            }
            _ => None,
        })
        .expect("summary checkpoint contains a Derived Summary")
}
fn summary_content(result: &AgentExecutionResult) -> &[UserContentBlock] {
    let rustx::model::projected::ProjectedMessage::User { content, .. } = committed_summary(result)
    else {
        panic!("summary has User semantics without canonical identity")
    };
    content
}
fn compaction_event(result: &AgentExecutionResult, started: bool) -> RuntimeEvent {
    result
        .event_history
        .iter()
        .find(|event| {
            if started {
                matches!(event, RuntimeEvent::CompactionStarted { .. })
            } else {
                matches!(event, RuntimeEvent::CompactionCompleted { .. })
            }
        })
        .unwrap()
        .clone()
}
fn operation(result: &AgentExecutionResult) -> rustx::runtime::identity::CompactionOperationId {
    let RuntimeEvent::CompactionStarted { operation_id } = compaction_event(result, true) else {
        unreachable!()
    };
    operation_id
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

fn native_request(
    attempt: &str,
    history: Vec<MessageBlock>,
    model: &Arc<FakeModel>,
    state: &ProviderContinuationState,
) -> AgentExecutionRequest {
    let (protocol, stateless) = match state {
        ProviderContinuationState::Anthropic(_) => {
            (rustx::model::ModelProtocol::AnthropicMessages, false)
        }
        ProviderContinuationState::OpenAiResponses(state) => (
            rustx::model::ModelProtocol::OpenAiResponses,
            matches!(state, OpenAiResponsesContinuation::Stateless { .. }),
        ),
    };
    let mut fixture = FixtureModel::text("fixture/native", protocol)
        .with_context_window(10_000_000)
        .with_max_output_tokens(1);
    if stateless {
        fixture.compat = serde_json::json!({"responsesStorage":"stateless"});
    }
    let mut input = request(attempt, history, 1, model);
    input.model = fixture_session_model(
        &[fixture],
        "fixture/native",
        &ScriptedAdapterFactory::new(model.clone()),
    )
    .snapshot()
    .unwrap();
    input
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
                state: state.clone(),
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let result = common::durable_agent_result(
        AgentExecution::new(
            native_request("attempt-1", vec![user("msg-user-1", "hi")], &model, &state),
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
        opaque: serde_json::json!({"type":"thinking", "thinking":"native reasoning", "signature": "sig-1"}),
    })
}

fn stored_state() -> ProviderContinuationState {
    ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stored {
        previous_response_id: "resp_abc".to_owned(),
    })
}

fn stateless_state() -> ProviderContinuationState {
    ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {
        items: vec![
            serde_json::json!({
                "type": "reasoning",
                "id": "rs_1",
                "encrypted_content": "opaque"
            }),
            serde_json::json!({"type":"function_call", "id":"fc_1", "call_id":"call-1", "name":"alpha", "arguments":"{}"}),
        ],
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
    ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18")
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
        Some(rustx::context::AgentStatusEngine::default()),
        assembly,
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
            .filter_map(ModelInputMessage::as_canonical)
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
        vec!["recent", "attempt-1-agent-1"]
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .effective_system_prompt
            .contains(&extension_guidance)
    );
    // Inspect the actual provider input, not just the hot conversation's ids:
    // the retired prefix is replaced, while the recent message keeps its exact
    // original body and canonical identity after the summary.
    assert_eq!(requests[0].messages.len(), 2);
    let ModelInputMessage::Derived(summary) = &requests[0].messages[0] else {
        panic!("the compacted prefix must be a derived summary");
    };
    assert!(
        summary
            .transformations
            .contains(&rustx::model::projected::SemanticTransformation::Summary)
    );
    assert_eq!(
        requests[0].messages[1].as_canonical(),
        Some(&user("recent", "recent history")),
        "nonzero recent-token retention preserves the original message verbatim"
    );
    let history = rustx::runtime::RequestHistory::new(tool_runtime.durable_store());
    let frozen = common::request_snapshots(&history);
    assert_eq!(frozen.len(), 1);
    let reconstructed = history.reconstruct(&frozen[0].identity).unwrap();
    assert_eq!(
        reconstructed.messages, requests[0].messages,
        "the frozen request inspected by Trace preserves the same summary and original recent suffix"
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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

    let expected = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-1"),
        },
        RuntimeEvent::TurnStarted,
        RuntimeEvent::ModelRequestStarted {
            owner: rustx::model::ModelRequestOwner::Agent {
                identity: rustx::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: rustx::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::ModelRequestCompleted {
            owner: rustx::model::ModelRequestOwner::Agent {
                identity: rustx::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: rustx::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            finish_reason: ModelFinishReason::ToolCalls,
            usage: Some(ModelUsage {
                input_tokens: Some(100),
                output_tokens: Some(4),
                total_tokens: Some(104),
                details: None,
            }),
            generation: None,
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
            message_id: MessageId::new("attempt-1-tool-1-0"),
            tool_call_id: ToolCallId::new("call-1"),
        },
        RuntimeEvent::TurnCompleted,
        RuntimeEvent::TurnStarted,
        compaction_event(&result, true),
        compaction_event(&result, false),
        RuntimeEvent::ModelRequestStarted {
            owner: rustx::model::ModelRequestOwner::Agent {
                identity: rustx::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: rustx::runtime::identity::TurnId::new("2"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:2:0"),
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::ModelRequestCompleted {
            owner: rustx::model::ModelRequestOwner::Agent {
                identity: rustx::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: rustx::runtime::identity::TurnId::new("2"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:2:0"),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
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
    let ModelInputMessage::Derived(summary) = &requests[1].messages[0] else {
        panic!("first projected message must be the summary");
    };
    assert!(matches!(
        &summary.message,
        rustx::model::projected::ProjectedMessage::User {
            source: UserSource::Runtime,
            kind: rustx::model::projected::ProjectedUserKind::CompactionContext,
            ..
        }
    ));
    assert!(
        summary
            .transformations
            .contains(&rustx::model::projected::SemanticTransformation::Summary)
    );
    assert!(matches!(
        requests[1].messages[1].as_canonical(),
        Some(MessageBlock::Assistant(assistant)) if assistant.id.as_str() == "attempt-1-agent-1"
    ));
    assert!(matches!(
        requests[1].messages[2].as_canonical(),
        Some(MessageBlock::Tool(tool)) if tool.id.as_str() == "attempt-1-tool-1-0"
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
            "attempt-1-tool-1-0".to_owned(),
            "attempt-1-agent-2".to_owned(),
        ]
    );
    assert_eq!(result.messages()[0].clone(), user("msg-user-1", "hi"));

    // The Conversation Surface replaced exactly the compacted span.
    assert_eq!(
        result
            .active_ids()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>(),
        vec![
            "attempt-1-agent-1".to_owned(),
            "attempt-1-tool-1-0".to_owned(),
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted { .. })),
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
    let overflow = coded_overflow_error();
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "provisional")),
            FakeStep::Emit(ModelEvent::Failed {
                error: overflow.clone(),
            }),
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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

    let expected = vec![
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("attempt-1"),
        },
        RuntimeEvent::TurnStarted,
        RuntimeEvent::ModelRequestStarted {
            owner: rustx::model::ModelRequestOwner::Agent {
                identity: rustx::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: rustx::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::ModelRequestFailed {
            owner: rustx::model::ModelRequestOwner::Agent {
                identity: rustx::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: rustx::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            error: overflow,
            usage: None,
            generation: None,
        },
        compaction_event(&result, true),
        compaction_event(&result, false),
        RuntimeEvent::ModelRetryScheduled {
            failed_request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            retry_number: 1,
            retry_delay_ms: None,
        },
        RuntimeEvent::ModelRequestStarted {
            owner: rustx::model::ModelRequestOwner::Agent {
                identity: rustx::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: rustx::runtime::identity::TurnId::new("1"),
                    retry_number: 1,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:1"),
            model: "fake-model".to_owned(),
        },
        RuntimeEvent::ModelRequestCompleted {
            owner: rustx::model::ModelRequestOwner::Agent {
                identity: rustx::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: rustx::runtime::identity::TurnId::new("1"),
                    retry_number: 1,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:1"),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(ModelUsage {
                input_tokens: Some(4),
                output_tokens: Some(4),
                total_tokens: Some(8),
                details: None,
            }),
            generation: None,
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
        requests[1].messages[0],
        ModelInputMessage::Derived(ref message) if message.transformations.contains(&rustx::model::projected::SemanticTransformation::Summary)
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
    let summary = summary_content(&result);
    assert!(
        matches!(
            summary.first(),
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
            summary_content(&result).first(),
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
        vec!["msg-user-1".to_owned()],
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
        result.snapshot_history()[0].history_cut.activation,
        result.snapshot_history()[1].history_cut.activation,
        "compaction advances projection activation while canonical frontier stays fixed"
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
                .reconstruct_from_canonical(result.messages())
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
    let candidate_messages = candidate_history
        .ledger()
        .audit_records()
        .iter()
        .cloned()
        .map(Into::into)
        .collect::<rustx::model::SelectedModelInput>();
    let unconstrained = rustx::context::planning::summary_prefix(
        &planner,
        &candidate_messages,
        "",
        &[],
        CompactionBudgets::new(1, 1, 1_000_000),
        &rustx::context::CompactionConstraints::default(),
        &rustx::runtime::CancellationSignal::new(),
    )
    .await
    .expect("unrestricted historical prefix");
    assert_eq!(
        unconstrained, 3,
        "unrestricted retirement crosses fresh inbound"
    );
    let fresh =
        rustx::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("msg-inbound-1")])
            .unwrap();
    let protected = rustx::context::planning::summary_prefix(
        &planner,
        &candidate_messages,
        "",
        &[],
        CompactionBudgets::new(1, 1, 1_000_000),
        &rustx::context::CompactionConstraints {
            fresh_inbound: Some(&fresh),
            ..Default::default()
        },
        &rustx::runtime::CancellationSignal::new(),
    )
    .await
    .expect("fresh input leaves earlier retirement");
    assert_eq!(protected, 1);

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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
        result.snapshot_history()[0].history_cut.activation,
        result.snapshot_history()[1].history_cut.activation,
        "successful overflow compaction activates a new historical projection"
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
                .reconstruct_from_canonical(result.messages())
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
        .filter(|event| matches!(event, RuntimeEvent::CompactionStarted { .. }))
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
        2,
        "input and exactly one real committed Assistant message"
    );
    let MessageBlock::Assistant(assistant) = &result.messages()[1] else {
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
    let MessageBlock::Assistant(assistant) = &result.messages()[1] else {
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
        .filter(|event| matches!(event, RuntimeEvent::CompactionStarted { .. }))
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
            retry_message_id(1).as_str().to_owned(),
            "attempt-1-tool-1-0".to_owned(),
            retry_message_id(2).as_str().to_owned(),
        ],
        "compaction preserves canonical history without summary commits"
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
        let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
            owner: rustx::model::ModelRequestOwner::Agent {
                identity: rustx::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: rustx::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            error: overflow_error(),
            usage: None,
            generation: None,
        },
        compaction_event(&result, true),
        RuntimeEvent::CompactionFailed {
            operation_id: operation(&result),
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
        Some(rustx::context::AgentStatusEngine::default()),
        CompactionBudgets::new(1, 1, 9),
    );
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
        RuntimeEvent::CompactionFailed { error, .. } if error.contains("no complete historical prefix")
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
        FakeSummaryStep::Fail(rustx::context::summarizer::summary_execution_failed(
            coded_overflow_error(),
        )),
        FakeSummaryStep::Return("summary-1".to_owned()),
    ]));
    let runtime = ContextRuntime::with_scripted_summarizer(
        engine(500, 0, 5, weighted(100, 10, 0)),
        summarizer.clone(),
        Some(rustx::context::AgentStatusEngine::default()),
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
                Some(status_engine),
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
            .all(|event| !matches!(event, RuntimeEvent::CompactionStarted { .. })),
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
            .all(|event| !matches!(event, RuntimeEvent::CompactionStarted { .. })),
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted { .. })),
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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

#[tokio::test]
async fn cancellation_observed_by_compaction_budget_preserves_the_attempt_reason() {
    use crate::runtime::monotonic::{ManualMonotonicClock, MonotonicClock};
    #[derive(Debug)]
    struct CancelOnBudgetClock {
        cancellation: AgentCancellation,
        clock: ManualMonotonicClock,
        reads: AtomicUsize,
    }
    impl MonotonicClock for CancelOnBudgetClock {
        fn now_millis(&self) -> u64 {
            if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
                self.cancellation.cancel();
            }
            self.clock.now_millis()
        }
        fn wait_until_millis(&self, deadline: u64) -> futures_util::future::BoxFuture<'static, ()> {
            self.clock.wait_until_millis(deadline)
        }
    }
    for reason in [
        CancellationReason::UserRequested,
        CancellationReason::RuntimeShutdown,
    ] {
        let cancellation = AgentCancellation::new(reason);
        let clock = Arc::new(CancelOnBudgetClock {
            cancellation: cancellation.clone(),
            clock: ManualMonotonicClock::new(),
            reads: AtomicUsize::new(0),
        });
        let model = fake_model(Vec::new());
        let summarizer = FakeContextSummarizer::new(Vec::new());
        let context = runtime_with(50, 0, 0, weighted(100, 10, 0), summarizer.clone());
        // The first monotonic read on this proactive path constructs the
        // budget after the initial cancellation check and before check_start.
        let mut policy = crate::scripted_suites::support::default_execution_policy();
        policy.monotonic_clock = clock.clone();
        let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
        let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let result = common::durable_agent_result(
            AgentExecution::new(
                request(
                    "cancel-budget-precheck",
                    vec![user("history", "history")],
                    1,
                    &model,
                ),
                capability.into_lease(),
                &cancellation,
                policy,
                context,
                &tool_runtime,
                rustx::agent::AttemptLifecycle::inert(),
            )
            .unwrap()
            .run()
            .await,
            tool_runtime.durable_store().as_ref(),
        );
        assert_eq!(clock.reads.load(Ordering::SeqCst), 1);
        assert_eq!(result.outcome, AttemptOutcome::Cancelled { reason });
        assert_single_terminal(&result.event_history);
        assert!(
            matches!(result.event_history.last(), Some(RuntimeEvent::AttemptCancelled { reason: observed, .. }) if *observed == reason)
        );
        assert!(!result.event_history.iter().any(|event| matches!(
            event,
            RuntimeEvent::AttemptFailed { .. }
                | RuntimeEvent::CompactionStarted { .. }
                | RuntimeEvent::CompactionFailed { .. }
                | RuntimeEvent::ModelRequestStarted { .. }
                | RuntimeEvent::AuxiliaryCallStarted { .. }
        )));
        assert!(model.requests().is_empty());
        assert!(summarizer.requests().is_empty());
        assert_no_compaction_committed(&result);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One native admission gate covers trigger, cancellation reason and each Busy source.
async fn cancellation_winning_summary_admission_is_not_a_compaction_failure() {
    #[derive(Clone, Copy)]
    enum Pressure {
        None,
        Capacity,
        Lifecycle,
    }
    for (pressure, cancel_attempt) in [
        (Pressure::None, true),
        (Pressure::Capacity, true),
        (Pressure::Lifecycle, true),
        (Pressure::Lifecycle, false),
    ] {
        for (reason, overflow) in [
            (CancellationReason::UserRequested, false),
            (CancellationReason::UserRequested, true),
            (CancellationReason::RuntimeShutdown, false),
            (CancellationReason::RuntimeShutdown, true),
        ] {
            let model = fake_model(if overflow {
                vec![vec![
                    FakeStep::Emit(started()),
                    FakeStep::Emit(overflow_event()),
                ]]
            } else {
                Vec::new()
            });
            let cancellation = AgentCancellation::new(reason);
            let policy = crate::scripted_suites::support::default_execution_policy();
            let request = request(
                "cancel-summary-admission",
                vec![user("history", "history")],
                1,
                &model,
            );
            let mut context = runtime_with(
                if overflow { 5_000 } else { 50 },
                0,
                0,
                weighted(100, 10, 0),
                FakeContextSummarizer::new(Vec::new()),
            );
            context.summarizer = Arc::new(rustx::context::ModelBackedSummarizer::new(
                request.model.summary_invocation().clone(),
                policy.model_timeout_policy,
                policy.monotonic_clock.clone(),
            ));
            let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
            let capacity = crate::runtime::auxiliary::capacity::ProcessCapacity::new();
            let lifecycle = crate::runtime::types::ConversationLifecycle::new();
            assert!(lifecycle.activate());
            let backend = crate::runtime::auxiliary::executor::AuxiliaryExecutor::new(
                tool_runtime.durable_store().clone(),
                Arc::new(crate::runtime::types::SystemClock),
                policy.monotonic_clock.clone(),
                lifecycle.clone(),
                Arc::new(crate::runtime::types::DurabilityGate::new()),
                capacity.conversation(),
            )
            .unwrap();
            context
                .summarizer
                .bind_backend(Arc::new(backend), context.resource_revision.get());
            let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
            let mut execution = AgentExecution::new(
                request,
                capability.into_lease(),
                &cancellation,
                policy,
                context,
                &tool_runtime,
                rustx::agent::AttemptLifecycle::inert(),
            )
            .unwrap();
            let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
            let _release = gate.arm_scoped();
            execution.install_compaction_admission_gate(gate.clone());
            let cancel = cancellation.clone();
            let (finished, settled) = std::sync::mpsc::channel();
            let controller = tokio::task::spawn_blocking(move || {
                gate.wait_entered();
                let mut held = Vec::new();
                if matches!(pressure, Pressure::Capacity) {
                    for _ in 0..8 {
                        let domain = capacity.conversation();
                        for _ in 0..4 {
                            held.push(domain.reserve_compaction().unwrap());
                        }
                    }
                }
                let resume = || {
                    if cancel_attempt {
                        cancel.cancel();
                    }
                    gate.release();
                };
                if matches!(pressure, Pressure::Lifecycle) {
                    // This is the very same lifecycle used by the executor,
                    // held independently of its cancellation gate. No Store
                    // connection/SQL lease is held across terminal writes.
                    lifecycle
                        .with_running_commit(|| {
                            resume();
                            settled
                                .recv_timeout(std::time::Duration::from_secs(10))
                                .unwrap();
                        })
                        .unwrap();
                } else {
                    resume();
                }
                held
            });
            let execution_result = execution.run().await;
            let _ = finished.send(());
            let _held = controller.await.unwrap();
            let result = common::durable_agent_result(
                execution_result,
                tool_runtime.durable_store().as_ref(),
            );
            if cancel_attempt {
                assert_eq!(result.outcome, AttemptOutcome::Cancelled { reason });
            } else {
                assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
                assert!(
                    result.event_history.iter().any(|event| matches!(
                        event, RuntimeEvent::CompactionFailed { error, .. }
                            if error.contains("native lifecycle ownership boundary is busy")
                    )),
                    "active lifecycle contention still reports Busy without waiting"
                );
            }
            assert!(
                result
                    .event_history
                    .iter()
                    .any(|event| matches!(event, RuntimeEvent::CompactionStarted { .. }))
            );
            if cancel_attempt {
                assert!(!result.event_history.iter().any(|event| matches!(
                    event,
                    RuntimeEvent::CompactionFailed { .. } | RuntimeEvent::AttemptFailed { .. }
                )));
                assert!(matches!(
                    result.event_history.last(),
                    Some(RuntimeEvent::AttemptCancelled { .. })
                ));
            }
            assert_eq!(
                model.requests().len(),
                usize::from(overflow),
                "cancellation prevents Summary dispatch and any subsequent primary dispatch"
            );
            assert_no_compaction_committed(&result);
            let journal = tool_runtime.durable_store().read_events(None, 32).unwrap();
            assert!(
                !journal.events.iter().any(|event| matches!(
                    event.event,
                    RuntimeEvent::AuxiliaryCallStarted { .. }
                        | RuntimeEvent::ModelRequestStarted {
                            owner: rustx::model::ModelRequestOwner::Auxiliary { .. },
                            ..
                        }
                )),
                "cancelled admission creates no Summary Call or Request start"
            );
        }
    }
}

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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted { .. })),
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
                .any(|event| matches!(event, RuntimeEvent::CompactionStarted { .. })),
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
            message,
            ModelInputMessage::Derived(derived) if derived.transformations.contains(&rustx::model::projected::SemanticTransformation::Summary)
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
        Some(rustx::context::AgentStatusEngine::default()),
        &snapshot,
        rustx::model::ModelTimeoutPolicy::default(),
        support::default_monotonic_clock(),
    )
    .expect("runtime");
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
        requests[2].messages[0],
        ModelInputMessage::Derived(ref message) if message.transformations.contains(&rustx::model::projected::SemanticTransformation::Summary)
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted { .. })),
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
            .filter(|event| matches!(event, RuntimeEvent::CompactionStarted { .. }))
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
            rustx::model::projected::ProjectedMessage::User {
                source: UserSource::Runtime,
                kind: rustx::model::projected::ProjectedUserKind::CompactionContext,
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
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
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
            native_request("attempt-1", vec![user("msg-u0", "hi")], &model, &state),
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
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted { .. })),
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

/// The SQLite hook arms only after COMMIT; the first hot read then fails.
/// Reconciliation must either install the current projection or stop before start.
#[tokio::test]
async fn committed_checkpoint_hot_reconciliation_never_starts_from_stale_input() {
    use rustx::durable::sqlite::CompactionFaultOperation;
    for (stage, recovered) in [
        (CompactionFaultOperation::AfterCommitOneHotReadFailure, true),
        (
            CompactionFaultOperation::AfterCommitTwoHotReadFailures,
            false,
        ),
    ] {
        let model = fake_model(vec![vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "answer")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ]]);
        let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
            "new projection".into(),
        )]));
        let context = ContextRuntime::with_scripted_summarizer(
            engine(200, 0, 5, weighted(100, 10, 0)),
            summarizer.clone(),
            None,
            CompactionBudgets::new(1, 1, 1_000_000),
        );
        let sqlite =
            Arc::new(rustx::durable::SqliteConversationStore::in_memory(conversation()).unwrap());
        let store = sqlite.clone() as Arc<dyn rustx::durable::ConversationStore>;
        let tool_runtime = common::tool_runtime_with_store(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
            Some(store.clone()),
        );
        sqlite.arm_compaction_fault_script([stage]);
        let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let result = common::durable_agent_result(
            AgentExecution::new(
                request(
                    "attempt-1",
                    vec![user("old-a", "A"), user("old-b", "B")],
                    1,
                    &model,
                ),
                capability.into_lease(),
                &cancellation,
                crate::scripted_suites::support::default_execution_policy(),
                context,
                &tool_runtime,
                rustx::agent::AttemptLifecycle::inert(),
            )
            .unwrap()
            .run()
            .await,
            store.as_ref(),
        );
        assert_eq!(
            summarizer.requests().len(),
            1,
            "reconciliation never regenerates a summary"
        );
        assert_eq!(result.projections.len(), 1);
        assert_eq!(
            result
                .event_history
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. }))
                .count(),
            1
        );
        assert!(
            !result
                .event_history
                .iter()
                .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. })),
            "confirmed commit cannot produce contradictory failure"
        );
        assert_eq!(model.requests().len(), usize::from(recovered));
        if recovered {
            assert!(matches!(
                model.requests()[0].messages[0],
                ModelInputMessage::Derived(_)
            ));
            assert_eq!(
                result.snapshot_history()[0].history_cut.checkpoint_id,
                result
                    .projections
                    .last()
                    .map(|p| p.candidate.checkpoint_id.clone())
            );
        } else {
            assert!(result.durable_failure.is_some());
            assert!(result.snapshot_history().is_empty());
            assert!(
                !result
                    .event_history
                    .iter()
                    .any(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
            );
        }
    }
}

#[tokio::test]
async fn n1_incomplete_search_settles_once_as_runtime_failure_even_after_real_overflow() {
    for overflow in [false, true] {
        let model = fake_model(vec![vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(ModelEvent::Failed {
                error: overflow_error(),
            }),
        ]]);
        let history: Vec<_> = (1..=129)
            .map(|n| user(&format!("M{n}"), "history"))
            .collect();
        let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
            "must not run".into(),
        )]));
        let context = ContextRuntime::with_scripted_summarizer(
            engine(
                if overflow { 100_000 } else { 100 },
                0,
                0,
                Arc::new(rustx::context::ClosureTokenEstimator::new(|_, _, _| 1000)),
            ),
            summarizer.clone(),
            None,
            CompactionBudgets::new(0, 1, 10),
        );
        let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
        let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let result = common::durable_agent_result(
            AgentExecution::new(
                request("attempt-1", history.clone(), 1, &model),
                capability.into_lease(),
                &cancellation,
                support::default_execution_policy(),
                context,
                &tool_runtime,
                rustx::agent::AttemptLifecycle::inert(),
            )
            .unwrap()
            .run()
            .await,
            tool_runtime.durable_store().as_ref(),
        );
        assert_eq!(model.requests().len(), usize::from(overflow));
        assert!(summarizer.requests().is_empty());
        assert_eq!(result.messages(), history);
        assert_single_terminal(&result.event_history);
        let RuntimeEvent::AttemptFailed {
            error:
                AttemptFailure::Runtime {
                    error: rustx::runtime::types::RuntimeError::ContextCompactionFailed { message },
                },
            ..
        } = result.event_history.last().unwrap()
        else {
            panic!("incomplete search is a runtime planning failure");
        };
        assert!(message.contains("compaction candidate search incomplete"));
        assert_eq!(
            result
                .event_history
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::CompactionFailed { .. }))
                .count(),
            1
        );
        assert_eq!(
            result
                .event_history
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
                .count(),
            usize::from(overflow)
        );
        assert!(
            tool_runtime
                .durable_store()
                .projection_head()
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn n1_default_middle_candidate_commits_and_freezes_the_subsequent_primary_input() {
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "done")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let history: Vec<_> = (1..=129)
        .map(|n| {
            let text = if n <= 2 {
                "x".repeat(100_000)
            } else {
                "history".into()
            };
            user(&format!("M{n}"), &text)
        })
        .collect();
    let historical = rustx::model::input::canonical_input(&history);
    let summary_input = rustx::context::SummaryRequest {
        retired: historical[..2].to_vec(),
    }
    .model_input();
    let summary_limit =
        rustx::context::DefaultTokenEstimator.estimate_input(&summary_input.messages, "", &[]);
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "Earlier work summarized.".into(),
    )]));
    let context = ContextRuntime::with_scripted_summarizer(
        engine(
            10_000,
            0,
            0,
            Arc::new(rustx::context::DefaultTokenEstimator),
        ),
        summarizer.clone(),
        None,
        CompactionBudgets::new(1, 128, summary_limit),
    );
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", history.clone(), 1, &model),
            capability.into_lease(),
            &cancellation,
            support::default_execution_policy(),
            context,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .unwrap()
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );
    assert_eq!(summarizer.requests().len(), 1);
    assert_eq!(summarizer.requests()[0].retired, historical[..2]);
    assert_eq!(model.requests().len(), 1);
    let sent = &model.requests()[0];
    assert!(!sent.messages.iter().any(|item| {
        item.canonical_id()
            .is_some_and(|id| ["M1", "M2"].contains(&id.as_str()))
    }));
    for n in 3..=129 {
        assert!(
            sent.messages
                .iter()
                .any(|item| item.canonical_id() == Some(history[n - 1].id()))
        );
    }
    let store = tool_runtime.durable_store();
    assert!(store.projection_head().unwrap().is_some());
    let snapshot = &result.snapshot_history()[0];
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        *sent
    );
    assert_eq!(&store.load_canonical().unwrap()[..history.len()], history);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert_single_terminal(&result.event_history);
}

#[tokio::test]
async fn n1_cancellation_during_exact_query_settles_without_summary_commit_or_request() {
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let queries = Arc::new(AtomicUsize::new(0));
    let counted = queries.clone();
    let estimator = rustx::context::ClosureTokenEstimator::new(move |messages, _, _| {
        if matches!(messages, [ModelInputMessage::Derived(_)]) {
            counted.fetch_add(1, Ordering::SeqCst);
            // Existing scripted estimator seam: cancellation linearizes inside
            // the first exact query, before the next planning observation.
            controller_cancellation.cancel();
        }
        1000
    });
    let model = fake_model(vec![]);
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![]));
    let context = ContextRuntime::with_scripted_summarizer(
        engine(100, 0, 0, Arc::new(estimator)),
        summarizer.clone(),
        None,
        CompactionBudgets::new(0, 1, 10),
    );
    let history: Vec<_> = (1..=129)
        .map(|n| user(&format!("M{n}"), "history"))
        .collect();
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", history.clone(), 1, &model),
            capability.into_lease(),
            &cancellation,
            support::default_execution_policy(),
            context,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .unwrap()
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );
    assert_eq!(queries.load(Ordering::SeqCst), 1);
    assert_single_terminal(&result.event_history);
    assert!(matches!(
        result.event_history.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
    assert!(summarizer.requests().is_empty());
    assert!(model.requests().is_empty());
    assert_eq!(result.messages(), history);
    assert!(!result.event_history.iter().any(|event| matches!(
        event,
        RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::CompactionCompleted { .. }
            | RuntimeEvent::CompactionFailed { .. }
    )));
    let store = tool_runtime.durable_store();
    assert!(store.projection_head().unwrap().is_none());
    let operation = result
        .event_history
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::CompactionStarted { operation_id } => Some(operation_id),
            _ => None,
        })
        .expect("planning operation allocated before its first exact query");
    assert!(
        store
            .projection_receipt(operation, "unused")
            .unwrap()
            .is_none()
    );
}

#[derive(Debug)]
struct ProductionUploadFixture {
    calls: Arc<AtomicUsize>,
    owner: Arc<dyn rustx::model::uploads::UploadProjectionResolver>,
}
impl rustx::model::uploads::UploadProjectionResolver for ProductionUploadFixture {
    fn resolve(
        &self,
        messages: &[ModelInputMessage],
    ) -> Result<rustx::model::uploads::UploadProjection, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.owner.resolve(messages)
    }
}
#[tokio::test]
async fn production_image_compaction_discovers_second_of_129_boundaries() {
    production_media_middle_candidate(true, false).await;
}
#[tokio::test]
async fn production_upload_compaction_discovers_second_of_129_boundaries() {
    production_media_middle_candidate(false, true).await;
}
#[tokio::test]
async fn production_mixed_media_compaction_discovers_second_of_129_boundaries() {
    production_media_middle_candidate(true, true).await;
}
async fn production_media_middle_candidate(image: bool, upload: bool) {
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, "done")),
        FakeStep::Emit(done(ModelFinishReason::Stop)),
    ]]);
    let mut history: Vec<_> = (1..=129)
        .map(|n| {
            let text = if n <= 2 {
                "x".repeat(100_000)
            } else {
                "history".into()
            };
            user(&format!("M{n}"), &text)
        })
        .collect();
    let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    if image {
        let mut bytes = Vec::new();
        png::Encoder::new(&mut bytes, 1, 1)
            .write_header()
            .unwrap()
            .write_image_data(&[0])
            .unwrap();
        let id = tool_runtime.artifacts().put_bounded(&bytes).unwrap();
        let MessageBlock::User(first) = &mut history[0] else {
            unreachable!()
        };
        first.content.push(UserContentBlock::Image(
            rustx::message::content::ImageReference {
                artifact_id: id,
                alt: None,
            },
        ));
    }
    let session_root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let upload_owner = if upload {
        use rustx::local_runtime::session::uploads::{SessionUploadResolver, UploadFile};
        use rustx::local_runtime::{
            session::SessionPersistentState, session_controller::SessionController,
        };
        let controller = SessionController::open(session_root.path()).unwrap();
        let session = controller
            .create_session(SessionPersistentState::from_input(
                &rustx::local_runtime::configuration::SessionConfigInput::new(
                    workspace.path().to_path_buf(),
                ),
            ))
            .await
            .unwrap()
            .session;
        let files = controller
            .upload(
                &session.id,
                None,
                vec![UploadFile {
                    name: "data.txt".into(),
                    data: b"local, Session-owned contents".to_vec().into(),
                }],
            )
            .await
            .unwrap();
        let MessageBlock::User(first) = &mut history[0] else {
            unreachable!()
        };
        first
            .content
            .push(UserContentBlock::UploadedFile(files[0].file.clone()));
        Some(Arc::new(SessionUploadResolver::for_session(
            session_root.path(),
            session.id,
        ))
            as Arc<dyn rustx::model::uploads::UploadProjectionResolver>)
    } else {
        None
    };
    let media: Vec<_> = match &history[0] {
        MessageBlock::User(u) => u
            .content
            .iter()
            .filter(|b| !matches!(b, UserContentBlock::Text(_)))
            .cloned()
            .collect(),
        _ => unreachable!(),
    };
    let MessageBlock::User(last) = &mut history[128] else {
        unreachable!()
    };
    last.content.extend(media);
    let historical = rustx::model::input::canonical_input(&history);
    let summary_input = rustx::context::SummaryRequest {
        retired: historical[..2].to_vec(),
    }
    .model_input();
    let summary_limit =
        rustx::context::DefaultTokenEstimator.estimate_input(&summary_input.messages, "", &[]);
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "Earlier work summarized.".into(),
    )]));
    let mut context = ContextRuntime::with_scripted_summarizer(
        engine(
            10_000,
            0,
            0,
            Arc::new(rustx::context::DefaultTokenEstimator),
        ),
        summarizer.clone(),
        None,
        CompactionBudgets::new(1, 128, summary_limit),
    );
    let resolutions = Arc::new(AtomicUsize::new(0));
    if let Some(owner) = upload_owner {
        context
            .engine
            .set_upload_resolver(Arc::new(ProductionUploadFixture {
                calls: resolutions.clone(),
                owner,
            }));
    }
    // Independent exhaustive exact-cost oracle for this controlled fixture.
    // Reset observations afterwards: production capture is measured separately.
    let mut oracle = context.engine.clone();
    oracle.set_image_projection(tool_runtime.artifacts().clone(), image);
    oracle
        .prepare_media(
            &historical.clone().into(),
            &historical.clone().into(),
            &rustx::runtime::CancellationSignal::new(),
        )
        .await
        .unwrap();
    let feasible: Vec<_> = (1..=129)
        .filter(|n| {
            let summary = rustx::context::SummaryRequest {
                retired: historical[..*n].to_vec(),
            }
            .model_input();
            oracle.estimate_model_input(&summary.messages.into(), "", &[]) <= summary_limit
                && oracle
                    .estimate_model_input(&historical[*n..].to_vec().into(), "", &[])
                    .saturating_add(128)
                    <= 9_999
        })
        .collect();
    assert_eq!(feasible, vec![2]);
    resolutions.store(0, Ordering::SeqCst);
    rustx::model::media::take_work();
    let mut execution_request = request("attempt-1", history.clone(), 1, &model);
    if image {
        let mut fixture = FixtureModel::text(
            "fixture/media",
            rustx::model::ModelProtocol::OpenAiChatCompletions,
        );
        fixture.extra_input_modalities.push("image");
        execution_request.model = fixture_session_model(
            &[fixture],
            "fixture/media",
            &ScriptedAdapterFactory::new(model.clone()),
        )
        .snapshot()
        .unwrap();
    }
    let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = common::durable_agent_result(
        AgentExecution::new(
            execution_request,
            capability.into_lease(),
            &cancellation,
            support::default_execution_policy(),
            context,
            &tool_runtime,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .unwrap()
        .run()
        .await,
        tool_runtime.durable_store().as_ref(),
    );
    assert_eq!(
        summarizer.requests().len(),
        1,
        "{:#?}",
        result.event_history
    );
    assert_eq!(summarizer.requests()[0].retired, historical[..2]);
    assert_eq!(model.requests().len(), 1);
    let sent = &model.requests()[0];
    assert!(!sent.messages.iter().any(|item| {
        item.canonical_id()
            .is_some_and(|id| ["M1", "M2"].contains(&id.as_str()))
    }));
    for n in 3..=128 {
        assert!(
            sent.messages
                .iter()
                .any(|item| item.canonical_id() == Some(history[n - 1].id()))
        );
    }
    if image {
        assert_eq!(sent.images.len(), 1);
    }
    if upload {
        assert!(sent.messages.iter().any(|m| matches!(m, ModelInputMessage::Derived(d) if d.sources.iter().any(|r| r.message_id.as_str() == "M129") && d.transformations.contains(&rustx::model::projected::SemanticTransformation::UploadRendering))));
        assert!(!sent.messages.iter().any(|m| matches!(m.view(), rustx::model::projected::ModelMessageView::User(blocks) if blocks.iter().any(|b| matches!(b, UserContentBlock::UploadedFile(_))))));
    }
    let store = tool_runtime.durable_store();
    assert!(store.projection_head().unwrap().is_some());
    let snapshot = &result.snapshot_history()[0];
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        *sent
    );
    let reopened = rustx::durable::SqliteConversationStore::open(
        conversation(),
        &tool_runtime.artifacts().root().join("conversation.sqlite"),
    )
    .unwrap();
    assert_eq!(
        reopened
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        *sent
    );
    assert_eq!(&store.load_canonical().unwrap()[..history.len()], history);
    assert_outcome(
        &result,
        &AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop,
        },
    );
    assert_single_terminal(&result.event_history);
    if upload {
        assert_eq!(resolutions.load(Ordering::SeqCst), 1);
    }
}

#[derive(Debug)]
struct FailedMediaResolver {
    cancellation: Option<AgentCancellation>,
    path: Option<String>,
    calls: Arc<AtomicUsize>,
}
impl rustx::model::uploads::UploadProjectionResolver for FailedMediaResolver {
    fn resolve(
        &self,
        messages: &[ModelInputMessage],
    ) -> Result<rustx::model::uploads::UploadProjection, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(cancel) = &self.cancellation {
            cancel.cancel();
        }
        let path = self.path.as_ref().ok_or("Session upload unavailable")?;
        let mut seen = std::collections::BTreeSet::new();
        Ok(rustx::model::uploads::UploadProjection {
            files: messages
                .iter()
                .filter_map(|m| match m.view() {
                    rustx::model::projected::ModelMessageView::User(c) => Some(c),
                    _ => None,
                })
                .flat_map(|c| c.iter())
                .filter_map(|b| match b {
                    UserContentBlock::UploadedFile(f) if seen.insert(f.clone()) => {
                        Some(rustx::model::uploads::ResolvedUpload {
                            file: f.clone(),
                            path: path.clone(),
                        })
                    }
                    _ => None,
                })
                .collect(),
        })
    }
}
#[tokio::test]
async fn production_media_failures_and_cancelled_resolution_start_no_request_or_checkpoint() {
    for case in [
        "missing-image",
        "corrupt-image",
        "unresolved-upload",
        "relative-upload",
        "cancel",
    ] {
        let model = fake_model(vec![]);
        let summarizer = Arc::new(FakeContextSummarizer::new(vec![]));
        let mut context = ContextRuntime::with_scripted_summarizer(
            engine(100, 0, 0, Arc::new(rustx::context::DefaultTokenEstimator)),
            summarizer.clone(),
            None,
            CompactionBudgets::new(1, 10, 100_000),
        );
        let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let calls = Arc::new(AtomicUsize::new(0));
        let mut history = vec![user("M1", &"history".repeat(1000)), user("M2", "history")];
        let MessageBlock::User(first) = &mut history[0] else {
            unreachable!()
        };
        if case.contains("image") || case == "cancel" {
            let id = if case == "corrupt-image" {
                tool_runtime.artifacts().put_bounded(b"not PNG").unwrap()
            } else {
                rustx::runtime::ArtifactId::new("artifact_999")
            };
            first.content.push(UserContentBlock::Image(
                rustx::message::content::ImageReference {
                    artifact_id: id,
                    alt: None,
                },
            ));
        }
        if case.contains("upload") || case == "cancel" {
            first.content.push(UserContentBlock::UploadedFile(
                rustx::message::content::UploadedFileRef {
                    batch_id: "batch".into(),
                    name: "file.txt".into(),
                },
            ));
            context
                .engine
                .set_upload_resolver(Arc::new(FailedMediaResolver {
                    cancellation: (case == "cancel").then(|| cancellation.clone()),
                    path: match case {
                        "unresolved-upload" => None,
                        "relative-upload" => Some("relative".into()),
                        _ => Some("/session/file.txt".into()),
                    },
                    calls: calls.clone(),
                }));
        }
        let mut execution_request = request("attempt-1", history.clone(), 1, &model);
        let mut fixture = FixtureModel::text(
            "fixture/media",
            rustx::model::ModelProtocol::OpenAiChatCompletions,
        );
        fixture.extra_input_modalities.push("image");
        execution_request.model = fixture_session_model(
            &[fixture],
            "fixture/media",
            &ScriptedAdapterFactory::new(model.clone()),
        )
        .snapshot()
        .unwrap();
        let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
        rustx::model::media::take_work();
        let result = common::durable_agent_result(
            AgentExecution::new(
                execution_request,
                capability.into_lease(),
                &cancellation,
                support::default_execution_policy(),
                context,
                &tool_runtime,
                rustx::agent::AttemptLifecycle::inert(),
            )
            .unwrap()
            .run()
            .await,
            tool_runtime.durable_store().as_ref(),
        );
        assert_single_terminal(&result.event_history);
        assert!(summarizer.requests().is_empty());
        assert!(model.requests().is_empty());
        assert_eq!(result.messages(), history);
        assert!(!result.event_history.iter().any(|e| matches!(
            e,
            RuntimeEvent::ModelRequestStarted { .. }
                | RuntimeEvent::CompactionStarted { .. }
                | RuntimeEvent::CompactionCompleted { .. }
        )));
        let store = tool_runtime.durable_store();
        assert!(store.projection_head().unwrap().is_none());
        assert!(
            store
                .projection_timeline(rustx::context::checkpoint::ProjectionActivationId(0), 64)
                .unwrap()
                .is_empty()
        );
        if case == "cancel" {
            assert!(matches!(
                result.event_history.last(),
                Some(RuntimeEvent::AttemptCancelled { .. })
            ));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(rustx::model::media::take_work().reads, 0);
        } else {
            assert!(
                matches!(
                    result.event_history.last(),
                    Some(RuntimeEvent::AttemptFailed { .. })
                ),
                "{case}: {:?}",
                result.event_history
            );
        }
    }
}

#[tokio::test]
async fn mandatory_adapter_preflight_rejects_native_history_without_continuation_before_start() {
    use rustx::message::types::{AssistantMessageBlock, ReasoningBlock};
    use rustx::model::ModelProtocol;
    let cases = [
        (
            ModelProtocol::AnthropicMessages,
            ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stored {
                previous_response_id: "foreign-response".into(),
            }),
        ),
        (
            ModelProtocol::AnthropicMessages,
            ProviderContinuationState::Anthropic(AnthropicContinuation {
                opaque: serde_json::json!({"type":"thinking","thinking":"native", "signature":""}),
            }),
        ),
        (
            ModelProtocol::OpenAiResponses,
            ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {
                items: vec![serde_json::json!({"type":"reasoning","id":"native", "summary":[]})],
            }),
        ),
    ];
    for (protocol, native) in cases {
        let history = vec![
            user("M1", "history"),
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("M2"),
                content: vec![AssistantContentBlock::Reasoning(ReasoningBlock {
                    text: Some("native".into()),
                    provider_state: Some(native),
                })],
            }),
            user("M3", "next"),
        ];
        let model = fake_model(vec![vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "should not dispatch")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ]]);
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let tool_runtime = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
        let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
        let mut input = request("attempt-1", history.clone(), 128, &model);
        input.model = fixture_session_model(
            &[FixtureModel::text("fixture/native", protocol)],
            "fixture/native",
            &ScriptedAdapterFactory::new(model.clone()),
        )
        .snapshot()
        .unwrap();
        let summarizer = FakeContextSummarizer::new(Vec::new());
        let result = common::durable_agent_result(
            AgentExecution::new(
                input,
                capability.into_lease(),
                &cancellation,
                support::default_execution_policy(),
                runtime_with(100_000, 0, 0, weighted(1, 1, 0), summarizer),
                &tool_runtime,
                rustx::agent::AttemptLifecycle::inert(),
            )
            .unwrap()
            .run()
            .await,
            tool_runtime.durable_store().as_ref(),
        );
        assert_single_terminal(&result.event_history);
        assert!(
            model.requests().is_empty(),
            "incompatible {protocol:?} dispatched"
        );
        assert!(
            !result
                .event_history
                .iter()
                .any(|e| matches!(e, RuntimeEvent::ModelRequestStarted { .. }))
        );
        assert!(result.snapshot_history().is_empty());
        assert_eq!(result.messages(), history);
        assert!(matches!(
            result.event_history.last(),
            Some(RuntimeEvent::AttemptFailed { .. })
        ));
    }
}

#[tokio::test]
async fn mandatory_preflight_validates_once_for_frozen_transient_retries() {
    let mut transient = fail(rustx::model::ModelErrorKind::Transport, "transient");
    let ModelEvent::Failed { error } = &mut transient else {
        unreachable!()
    };
    error.retry_disposition = rustx::model::error::ModelRetryDisposition::Transient;
    let model = fake_model(vec![
        vec![FakeStep::Emit(transient)],
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "answer")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tools = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let capability = common::capability_lease(ToolRegistry::new(), &tools).await;
    rustx::model::adapter::validation::take_preflights();
    let result = common::durable_agent_result(
        AgentExecution::new(
            request("attempt-1", vec![user("M1", "request")], 128, &model),
            capability.into_lease(),
            &cancellation,
            support::default_execution_policy(),
            runtime_with(
                100_000,
                0,
                0,
                weighted(1, 1, 0),
                FakeContextSummarizer::new(vec![]),
            ),
            &tools,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .unwrap()
        .run()
        .await,
        tools.durable_store().as_ref(),
    );
    assert_single_terminal(&result.event_history);
    assert_eq!(rustx::model::adapter::validation::take_preflights(), 1);
    assert_eq!(model.requests().len(), 2);
    assert_eq!(model.requests()[0], model.requests()[1]);
    assert_eq!(result.snapshot_history().len(), 2);
    assert_ne!(
        result.snapshot_history()[0].request_id,
        result.snapshot_history()[1].request_id
    );
    assert_eq!(
        result.snapshot_history()[0].semantic_input_fingerprint,
        result.snapshot_history()[1].semantic_input_fingerprint
    );
    assert_eq!(
        result
            .event_history
            .iter()
            .filter(|e| matches!(e, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn mandatory_preflight_rejects_retained_native_history_after_successful_compaction() {
    use rustx::message::types::{AssistantMessageBlock, ReasoningBlock};
    let history = vec![
        user("M1", "old"),
        user("M2", "middle"),
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("M3"),
            content: vec![AssistantContentBlock::Reasoning(ReasoningBlock {
                text: Some("native".into()),
                provider_state: Some(ProviderContinuationState::OpenAiResponses(
                    OpenAiResponsesContinuation::Stored {
                        previous_response_id: "foreign-response".into(),
                    },
                )),
            })],
        }),
    ];
    let model = fake_model(vec![]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tools = common::tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let capability = common::capability_lease(ToolRegistry::new(), &tools).await;
    let mut input = request("attempt-1", history.clone(), 1, &model);
    input.model = fixture_session_model(
        &[FixtureModel::text(
            "fixture/native",
            rustx::model::ModelProtocol::AnthropicMessages,
        )
        .with_max_output_tokens(1)],
        "fixture/native",
        &ScriptedAdapterFactory::new(model.clone()),
    )
    .snapshot()
    .unwrap();
    let summary = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "summary".into(),
    )]));
    let context = ContextRuntime::with_scripted_summarizer(
        engine(200, 0, 10, weighted(100, 10, 0)),
        summary.clone(),
        None,
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let result = common::durable_agent_result(
        AgentExecution::new(
            input,
            capability.into_lease(),
            &cancellation,
            support::default_execution_policy(),
            context,
            &tools,
            rustx::agent::AttemptLifecycle::inert(),
        )
        .unwrap()
        .run()
        .await,
        tools.durable_store().as_ref(),
    );
    assert_single_terminal(&result.event_history);
    assert_eq!(summary.requests().len(), 1);
    assert_eq!(result.projections.len(), 1);
    assert!(result.snapshot_history().is_empty());
    assert!(model.requests().is_empty());
    assert_eq!(result.messages(), history);
    assert_eq!(
        result
            .event_history
            .iter()
            .filter(|e| matches!(e, RuntimeEvent::CompactionCompleted { .. }))
            .count(),
        1
    );
    assert!(
        !result
            .event_history
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ModelRequestStarted { .. }))
    );
    assert!(matches!(
        result.event_history.last(),
        Some(RuntimeEvent::AttemptFailed { .. })
    ));
}
