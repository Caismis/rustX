//! Issue #136 deterministic tool-cancellation phase regressions.
//!
//! These tests drive the real Agent Loop with explicit watch-channel gates and
//! synchronous runtime-event observations. They prove the per-call executor
//! start frontier, cancellation settlement arbitration, model-order
//! preservation, and the absence of a second model turn after cancellation.
//! Wall-clock time is used only as an outer anti-hang guard.

use super::super::{common, support};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::future::BoxFuture;
use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionObserver, AgentExecutionRequest,
    AgentStatusObservation, AttemptLifecycle, LifecycleError, PreToolDecision, PreToolPolicy,
    PreToolView,
};
use rustx::context::{ContextRuntime, DefaultTokenEstimator, SessionContextPolicy};
use rustx::durable::TranscriptCursor;
use rustx::events::types::{AttemptOutcome, RuntimeEvent};
use rustx::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::publication::{PublicationAudit, PublicationFrame, PublicationStreamStart};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolCallId};
use rustx::runtime::types::CancellationReason;
use rustx::tools::ToolProgressCapability;
use rustx::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
use rustx::tools::types::{
    ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus, ToolInvocation,
};
use support::fake::{
    FakeModel, FakeStep, FakeTool, ScriptedCall, await_started, fake_model, success_result,
    tool_call_events,
};
use tokio::sync::watch;

fn request(model: &Arc<FakeModel>) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-136"),
        conversation_id: ConversationId::new("conv-136"),
        attempt_id: AttemptId::new("attempt-136"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new("msg-user-136"),
                content: vec![UserContentBlock::Text(rustx::message::content::TextBlock {
                    text: "Run the tool".to_owned(),
                })],
                source: UserSource::Human,
                kind: rustx::message::types::InboundKind::Message,
                timestamp: None,
            }),
        ])
        .expect("valid fixture conversation"),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        model: support::attempt_model(model.clone(), "fake-model"),
    }
}

fn context_runtime(model: &Arc<FakeModel>) -> ContextRuntime {
    let snapshot = support::attempt_model(model.clone(), "fake-model");
    ContextRuntime::for_attempt(
        SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(DefaultTokenEstimator),
        rustx::context::AgentStatusEngine::default(),
        &snapshot,
        rustx::model::ModelTimeoutPolicy::default(),
        support::default_monotonic_clock(),
    )
    .expect("valid context runtime")
}

fn call(id: &'static str, tool_id: &'static str, name: &'static str) -> ScriptedCall {
    ScriptedCall {
        id,
        tool_id,
        name,
        arguments: serde_json::json!({}),
    }
}

fn tool_turn(calls: &[ScriptedCall]) -> Vec<Vec<FakeStep>> {
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for (index, call) in calls.iter().enumerate() {
        for event in tool_call_events(
            u32::try_from(index).expect("the scripted batch is small"),
            call,
        ) {
            first.push(FakeStep::Emit(event));
        }
    }
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    vec![first]
}

fn tool_messages(
    audit: &common::DurableExecutionAudit,
) -> Vec<&rustx::message::types::ToolMessageBlock> {
    audit
        .result
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Tool(tool) => Some(tool),
            _ => None,
        })
        .collect()
}

async fn run(
    model: &Arc<FakeModel>,
    tools: ToolRegistry,
    lifecycle: AttemptLifecycle,
    cancellation: &AgentCancellation,
    observer: &dyn AgentExecutionObserver,
) -> common::DurableExecutionAudit {
    run_inner(model, tools, lifecycle, cancellation, observer, None, None).await
}

async fn run_with_tool_cancellation_settlement_pause(
    model: &Arc<FakeModel>,
    tools: ToolRegistry,
    lifecycle: AttemptLifecycle,
    cancellation: &AgentCancellation,
    observer: &dyn AgentExecutionObserver,
    pause: crate::agent::execution::test_sync::ToolCancellationSettlementPause,
) -> common::DurableExecutionAudit {
    run_inner(
        model,
        tools,
        lifecycle,
        cancellation,
        observer,
        Some(pause),
        None,
    )
    .await
}

async fn run_with_tool_physical_settlement_pause(
    model: &Arc<FakeModel>,
    tools: ToolRegistry,
    lifecycle: AttemptLifecycle,
    cancellation: &AgentCancellation,
    observer: &dyn AgentExecutionObserver,
    pause: crate::agent::execution::test_sync::ToolPhysicalSettlementPause,
) -> common::DurableExecutionAudit {
    run_inner(
        model,
        tools,
        lifecycle,
        cancellation,
        observer,
        None,
        Some(pause),
    )
    .await
}

async fn run_inner(
    model: &Arc<FakeModel>,
    tools: ToolRegistry,
    lifecycle: AttemptLifecycle,
    cancellation: &AgentCancellation,
    observer: &dyn AgentExecutionObserver,
    cancellation_settlement_pause: Option<
        crate::agent::execution::test_sync::ToolCancellationSettlementPause,
    >,
    physical_settlement_pause: Option<
        crate::agent::execution::test_sync::ToolPhysicalSettlementPause,
    >,
) -> common::DurableExecutionAudit {
    let tool_runtime = common::tool_runtime("conv-136");
    let store = tool_runtime.durable_store();
    let capability = common::capability_lease(tools, &tool_runtime).await;
    let publication = common::RecordingPublicationObserver::default();
    let mut execution = AgentExecution::new(
        request(model),
        capability.into_lease(),
        cancellation,
        support::default_execution_policy(),
        context_runtime(model),
        &tool_runtime,
        lifecycle,
    )
    .expect("conversation identity matches the tool runtime");
    execution.observe(observer);
    if let Some(pause) = cancellation_settlement_pause {
        execution.install_tool_cancellation_settlement_pause(pause);
    }
    if let Some(pause) = physical_settlement_pause {
        execution.install_tool_physical_settlement_pause(pause);
    }
    let result = tokio::time::timeout(Duration::from_secs(5), execution.run())
        .await
        .expect("Issue #136 execution must settle");
    common::durable_agent_result_with_publication(result, store.as_ref(), &publication)
}

#[derive(Default)]
struct NoopObserver;

impl AgentExecutionObserver for NoopObserver {
    fn observe_event(&self, _attempt_id: &AttemptId, _event: &RuntimeEvent) {}

    fn observe_committed(
        &self,
        _attempt_id: &AttemptId,
        _block: &MessageBlock,
        _transcript_cursor: Option<TranscriptCursor>,
    ) {
    }

    fn observe_status(&self, _observation: &AgentStatusObservation) {}

    fn observe_publication_opened(&self, _attempt_id: &AttemptId, _start: &PublicationStreamStart) {
    }

    fn observe_publication(&self, _attempt_id: &AttemptId, _frame: &PublicationFrame) {}

    fn observe_publication_settled(
        &self,
        _attempt_id: &AttemptId,
        _audit: &PublicationAudit,
        _transcript_cursor: TranscriptCursor,
    ) {
    }
}

struct GatedPreToolPolicy {
    entered: watch::Sender<bool>,
    release: watch::Receiver<bool>,
}

impl PreToolPolicy for GatedPreToolPolicy {
    fn evaluate<'a>(
        &'a self,
        _view: &'a PreToolView<'a>,
    ) -> BoxFuture<'a, Result<PreToolDecision, LifecycleError>> {
        let entered = self.entered.clone();
        let mut release = self.release.clone();
        Box::pin(async move {
            entered.send_replace(true);
            release
                .wait_for(|released| *released)
                .await
                .expect("pre-tool release channel stays open");
            Ok(PreToolDecision::Allow)
        })
    }
}

#[derive(Clone, Copy)]
enum CancelEvent {
    Started,
    Completed,
}

struct CancelOnToolEvent {
    cancellation: AgentCancellation,
    call_id: ToolCallId,
    reason: CancellationReason,
    event: CancelEvent,
}

impl CancelOnToolEvent {
    fn matches(&self, event: &RuntimeEvent) -> bool {
        match (self.event, event) {
            (CancelEvent::Started, RuntimeEvent::ToolExecutionStarted { tool_call_id, .. })
            | (CancelEvent::Completed, RuntimeEvent::ToolExecutionCompleted { tool_call_id, .. }) => {
                *tool_call_id == self.call_id
            }
            _ => false,
        }
    }
}

impl AgentExecutionObserver for CancelOnToolEvent {
    fn observe_event(&self, _attempt_id: &AttemptId, event: &RuntimeEvent) {
        if self.matches(event) {
            let _ = self.cancellation.request_cancel(self.reason);
        }
    }

    fn observe_committed(
        &self,
        _attempt_id: &AttemptId,
        _block: &MessageBlock,
        _transcript_cursor: Option<TranscriptCursor>,
    ) {
    }

    fn observe_status(&self, _observation: &AgentStatusObservation) {}

    fn observe_publication_opened(&self, _attempt_id: &AttemptId, _start: &PublicationStreamStart) {
    }

    fn observe_publication(&self, _attempt_id: &AttemptId, _frame: &PublicationFrame) {}

    fn observe_publication_settled(
        &self,
        _attempt_id: &AttemptId,
        _audit: &PublicationAudit,
        _transcript_cursor: TranscriptCursor,
    ) {
    }
}

/// An executor that deliberately ignores the cancellation view and settles
/// with a fixed late result after a release. A cancellation *request* is not
/// a confirmed cancellation result: when the cancellation branch wins the
/// foreground arbitration, the outcome this executor itself settled remains
/// authoritative in the tool result slot, while the attempt still settles
/// cancelled.
struct LateCompletionTool {
    definition: rustx::tools::types::ToolDefinition,
    started: watch::Sender<bool>,
    release: watch::Sender<bool>,
    side_effect: Arc<AtomicBool>,
    result: ToolExecutionResult,
}

/// An executor that returns a physical cancellation with its own reason. The
/// Agent Loop must preserve that reason when the physical result wins, while
/// still canonicalizing the phase from the executor frontier.
struct PhysicalCancelledTool {
    definition: rustx::tools::types::ToolDefinition,
    started: watch::Sender<bool>,
    result: ToolExecutionResult,
}

impl PhysicalCancelledTool {
    fn register(self, registry: &mut ToolRegistry) {
        registry
            .register(self.definition.clone(), Arc::new(self))
            .expect("physical cancellation tool registration");
    }
}

impl ToolExecutor for PhysicalCancelledTool {
    fn execute<'a>(
        &'a self,
        _invocation: ToolInvocation,
        _context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        self.started.send_replace(true);
        let result = self.result.clone();
        Box::pin(async move { result })
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

impl LateCompletionTool {
    fn register(self, registry: &mut ToolRegistry) {
        registry
            .register(self.definition.clone(), Arc::new(self))
            .expect("late completion tool registration");
    }
}

impl ToolExecutor for LateCompletionTool {
    fn execute<'a>(
        &'a self,
        _invocation: ToolInvocation,
        _context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        let started = self.started.clone();
        let mut release = self.release.subscribe();
        let side_effect = Arc::clone(&self.side_effect);
        let result = self.result.clone();
        Box::pin(async move {
            started.send_replace(true);
            release
                .wait_for(|released| *released)
                .await
                .expect("late completion release channel stays open");
            side_effect.store(true, Ordering::SeqCst);
            result
        })
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue136_canonical_call_cancelled_before_executor_start() {
    let call = call("call-before", "tool-before", "before");
    let model = fake_model(tool_turn(&[call]));
    let tool = FakeTool::new(
        common::tool("before", "tool-before"),
        success_result("must not run"),
    );
    let calls = tool.calls();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);

    let (entered, mut entered_rx) = watch::channel(false);
    let (release, release_rx) = watch::channel(false);
    let lifecycle = AttemptLifecycle::inert().with_pre_tool_policy(Arc::new(GatedPreToolPolicy {
        entered,
        release: release_rx,
    }));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        entered_rx
            .wait_for(|is_entered| *is_entered)
            .await
            .expect("pre-tool policy entered");
        assert!(controller_cancellation.request_cancel(CancellationReason::RuntimeShutdown));
        release.send_replace(true);
    });

    let audit = run(&model, tools, lifecycle, &cancellation, &NoopObserver).await;
    controller
        .await
        .expect("before-start cancellation controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1, "one canonical result slot exists");
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::RuntimeShutdown,
            phase: ToolCancellationPhase::BeforeStart,
        }
    ));
    assert!(
        calls.borrow().is_empty(),
        "executor invocation count is zero"
    );
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
            .count(),
        0,
        "no start fact exists before the executor frontier"
    );
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionCompleted { .. }))
            .count(),
        0,
        "a never-started call has no physical completion fact"
    );
    assert!(matches!(
        audit.result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::RuntimeShutdown
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue136_running_call_is_cancelled_during_execution() {
    let call = call("call-running", "tool-running", "running");
    let model = fake_model(tool_turn(&[call]));
    let (tool, _release) = FakeTool::parking(
        common::tool("running", "tool-running"),
        success_result("not reached"),
    );
    let calls = tool.calls();
    let mut started = tool.started();
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);

    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        await_started(&mut started, "running tool").await;
        assert!(controller_cancellation.request_cancel(CancellationReason::UserRequested));
    });
    let audit = run(
        &model,
        tools,
        AttemptLifecycle::inert(),
        &cancellation,
        &NoopObserver,
    )
    .await;
    controller.await.expect("running cancellation controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested,
            phase: ToolCancellationPhase::DuringExecution,
        }
    ));
    assert_eq!(calls.borrow().len(), 1, "executor invocation count is one");
    assert!(
        audit
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue136_completion_wins_before_cancellation() {
    let call = call("call-complete", "tool-complete", "complete");
    let model = fake_model(tool_turn(&[call]));
    let mut tools = ToolRegistry::new();
    FakeTool::new(
        common::tool("complete", "tool-complete"),
        success_result("real result"),
    )
    .register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let observer = CancelOnToolEvent {
        cancellation: cancellation.clone(),
        call_id: ToolCallId::new("call-complete"),
        reason: CancellationReason::UserRequested,
        event: CancelEvent::Completed,
    };

    let audit = run(
        &model,
        tools,
        AttemptLifecycle::inert(),
        &cancellation,
        &observer,
    )
    .await;

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Success
    ));
    assert_eq!(messages[0].result, success_result("real result"));
    assert_eq!(
        model.requests().len(),
        1,
        "cancellation prevents continuation"
    );
    assert!(matches!(
        audit.result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
}

/// Drives the exact parked cut: the late-completion executor started, the
/// attempt cancellation wins the foreground arbitration (proven by the
/// cancellation-settlement pause), and only then does the executor settle
/// with its pre-programmed result. The cancellation request is not a
/// confirmed cancellation result, so the executor-settled status is
/// authoritative for the tool result slot.
async fn run_cancellation_winner_with_late_executor_outcome(
    result: ToolExecutionResult,
) -> (
    common::DurableExecutionAudit,
    Arc<AtomicBool>,
    Arc<FakeModel>,
) {
    let call = call("call-late", "tool-late", "late");
    let model = fake_model(tool_turn(&[call]));
    let (started, mut started_rx) = watch::channel(false);
    let (release, _release_rx) = watch::channel(false);
    let (cancellation_settlement_pause, mut cancellation_won_rx, release_pause) =
        crate::agent::execution::test_sync::ToolCancellationSettlementPause::install();
    let side_effect = Arc::new(AtomicBool::new(false));
    let mut tools = ToolRegistry::new();
    LateCompletionTool {
        definition: common::tool("late", "tool-late"),
        started,
        release: release.clone(),
        side_effect: Arc::clone(&side_effect),
        result,
    }
    .register(&mut tools);

    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        started_rx
            .wait_for(|is_started| *is_started)
            .await
            .expect("late tool started");
        assert!(controller_cancellation.request_cancel(CancellationReason::UserRequested));
        cancellation_won_rx
            .wait_for(|won| *won)
            .await
            .expect("cancellation branch won the foreground arbitration");
        release_pause
            .send(())
            .expect("cancellation settlement pause remains installed");
        release.send_replace(true);
    });
    let audit = run_with_tool_cancellation_settlement_pause(
        &model,
        tools,
        AttemptLifecycle::inert(),
        &cancellation,
        &NoopObserver,
        cancellation_settlement_pause,
    )
    .await;
    controller.await.expect("late completion controller");
    (audit, side_effect, model)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue136_cancellation_winner_keeps_the_executor_settled_success() {
    let (audit, side_effect, model) =
        run_cancellation_winner_with_late_executor_outcome(success_result("late completion")).await;

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1);
    // The executor proved the call ran to a known completion; the earlier
    // cancellation request must not overwrite that settled outcome.
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Success
    ));
    assert!(
        side_effect.load(Ordering::SeqCst),
        "the settled completion's side effect is real, never rolled back"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "the cancelled attempt has no next turn even though the tool completed"
    );
    assert!(matches!(
        audit.result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue136_cancellation_winner_keeps_the_executor_settled_outcome_unknown() {
    let (audit, side_effect, model) =
        run_cancellation_winner_with_late_executor_outcome(ToolExecutionResult {
            status: ToolExecutionStatus::OutcomeUnknown {
                detail: "remote termination could not be confirmed".to_owned(),
            },
            content: Vec::new(),
            duration_ms: 4,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        })
        .await;

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1);
    // The executor's honest unknown outcome is kept: a cancellation request
    // must not overwrite it with a false `Cancelled` claim.
    let ToolExecutionStatus::OutcomeUnknown { detail } = &messages[0].result.status else {
        panic!(
            "the executor-settled unknown outcome is authoritative: {:?}",
            messages[0].result.status
        );
    };
    assert_eq!(detail, "remote termination could not be confirmed");
    assert!(
        side_effect.load(Ordering::SeqCst),
        "an unknown outcome may carry real side effects"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "the cancelled attempt has no next turn"
    );
    assert!(matches!(
        audit.result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue136_physical_result_winner_freezes_executor_cancellation_reason() {
    let call = call(
        "call-physical-cancel",
        "tool-physical-cancel",
        "physical-cancel",
    );
    let model = fake_model(tool_turn(&[call]));
    let (started, mut started_rx) = watch::channel(false);
    let (physical_settlement_pause, mut physical_won_rx, release_pause) =
        crate::agent::execution::test_sync::ToolPhysicalSettlementPause::install();
    let mut tools = ToolRegistry::new();
    PhysicalCancelledTool {
        definition: common::tool("physical-cancel", "tool-physical-cancel"),
        started,
        result: ToolExecutionResult {
            status: ToolExecutionStatus::Cancelled {
                reason: CancellationReason::ParentCancelled,
                // This is intentionally only a provisional executor value;
                // the Agent Loop owns the authoritative phase.
                phase: ToolCancellationPhase::BeforeStart,
            },
            content: Vec::new(),
            duration_ms: 4,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        },
    }
    .register(&mut tools);

    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        started_rx
            .wait_for(|is_started| *is_started)
            .await
            .expect("physical cancellation executor started");
        physical_won_rx
            .wait_for(|won| *won)
            .await
            .expect("physical result won the foreground arbitration");
        assert!(controller_cancellation.request_cancel(CancellationReason::UserRequested));
        release_pause
            .send(())
            .expect("physical settlement pause remains installed");
    });

    let audit = run_with_tool_physical_settlement_pause(
        &model,
        tools,
        AttemptLifecycle::inert(),
        &cancellation,
        &NoopObserver,
        physical_settlement_pause,
    )
    .await;
    controller
        .await
        .expect("physical-result cancellation controller");

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::ParentCancelled,
            phase: ToolCancellationPhase::DuringExecution,
        }
    ));
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter_map(|event| match event {
                RuntimeEvent::ToolExecutionCompleted { result, .. } => Some(&result.status),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![&messages[0].result.status],
        "the physical winner's result is the completed execution fact"
    );
    assert!(matches!(
        audit.result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue136_mixed_batch_preserves_order_and_phases() {
    let calls = [
        call("call-a", "tool-a", "a"),
        call("call-b", "tool-b", "b"),
        call("call-c", "tool-c", "c"),
    ];
    let model = fake_model(tool_turn(&calls));
    let mut tools = ToolRegistry::new();
    FakeTool::new(common::tool("a", "tool-a"), success_result("A complete")).register(&mut tools);
    let (tool_b, _release_b) = FakeTool::parking(
        common::tool_policies(
            "b",
            "tool-b",
            rustx::tools::types::ToolExecutionPolicy::ForegroundOnly,
            rustx::tools::types::ToolConcurrencyPolicy::Parallel,
        ),
        success_result("B not complete"),
    );
    let b_calls = tool_b.calls();
    tool_b.register(&mut tools);
    let tool_c = FakeTool::new(
        common::tool_policies(
            "c",
            "tool-c",
            rustx::tools::types::ToolExecutionPolicy::ForegroundOnly,
            rustx::tools::types::ToolConcurrencyPolicy::Parallel,
        ),
        success_result("C must not start"),
    );
    let c_calls = tool_c.calls();
    tool_c.register(&mut tools);

    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let observer = CancelOnToolEvent {
        cancellation: cancellation.clone(),
        call_id: ToolCallId::new("call-b"),
        reason: CancellationReason::UserRequested,
        event: CancelEvent::Started,
    };
    let audit = run(
        &model,
        tools,
        AttemptLifecycle::inert(),
        &cancellation,
        &observer,
    )
    .await;

    let messages = tool_messages(&audit);
    assert_eq!(messages.len(), 3, "one result slot per canonical ToolCall");
    assert_eq!(
        messages
            .iter()
            .map(|message| message.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-a", "call-b", "call-c"],
        "canonical model order is independent of completion order"
    );
    assert!(matches!(
        messages[0].result.status,
        ToolExecutionStatus::Success
    ));
    assert!(matches!(
        messages[1].result.status,
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested,
            phase: ToolCancellationPhase::DuringExecution,
        }
    ));
    assert!(matches!(
        messages[2].result.status,
        ToolExecutionStatus::Cancelled {
            reason: CancellationReason::UserRequested,
            phase: ToolCancellationPhase::BeforeStart,
        }
    ));
    assert_eq!(b_calls.borrow().len(), 1, "B crossed the executor frontier");
    assert!(c_calls.borrow().is_empty(), "C never crossed the frontier");
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionCompleted { .. }))
            .count(),
        2,
        "only A and B have physical completion facts"
    );
    assert_eq!(
        model.requests().len(),
        1,
        "the cancelled attempt has no next turn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue136_incomplete_proposal_has_no_tool_execution_authority() {
    let call = call("call-incomplete", "tool-incomplete", "incomplete");
    let events = tool_call_events(0, &call);
    let model = fake_model(vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(events[0].clone()),
        FakeStep::Emit(events[1].clone()),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        }),
    ]]);
    let mut tools = ToolRegistry::new();
    FakeTool::new(
        common::tool("incomplete", "tool-incomplete"),
        success_result("must not run"),
    )
    .register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    let audit = run(
        &model,
        tools,
        AttemptLifecycle::inert(),
        &cancellation,
        &NoopObserver,
    )
    .await;

    assert!(tool_messages(&audit).is_empty());
    assert!(!audit.event_history.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolExecutionStarted { .. } | RuntimeEvent::ToolExecutionCompleted { .. }
    )));
    assert!(
        !audit
            .result
            .messages()
            .iter()
            .any(|message| matches!(message, MessageBlock::Tool(_)))
    );
}

#[test]
fn issue136_reason_and_phase_are_independent_closed_wire_axes() {
    for (reason, reason_wire) in [
        (CancellationReason::UserRequested, "user_requested"),
        (CancellationReason::RuntimeShutdown, "runtime_shutdown"),
        (CancellationReason::ParentCancelled, "parent_cancelled"),
    ] {
        for (phase, phase_wire) in [
            (ToolCancellationPhase::BeforeStart, "before_start"),
            (ToolCancellationPhase::DuringExecution, "during_execution"),
        ] {
            let status = ToolExecutionStatus::Cancelled { reason, phase };
            let value = serde_json::to_value(&status).expect("serialize cancellation status");
            assert_eq!(value["reason"], reason_wire);
            assert_eq!(value["phase"], phase_wire);
            assert_eq!(
                serde_json::from_value::<ToolExecutionStatus>(value).expect("decode status"),
                status
            );
        }
    }
}
