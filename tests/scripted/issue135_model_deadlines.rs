//! Issue #135: deterministic model request response-start and stream-idle
//! deadline regressions.
//!
//! The primary tests drive the real Agent Loop with the in-crate fake model,
//! the durable request-start/outcome path, and the publication coalescer.
//! Every semantic cut is controlled by the runtime manual monotonic clock and
//! explicit synchronization channels; wall-clock time is used only as an
//! outer anti-hang guard.

use super::{common, support};

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustx::agent::{
    AgentCancellation, AgentExecution, AgentExecutionObserver, AgentExecutionRequest,
};
use rustx::context::{
    AgentStatusEngine, CompactionBudgets, ContextConfig, ContextEngine, ContextRuntime,
    ContextSummarizer, ModelBackedSummarizer, SummaryRequest,
};
use rustx::conversation::ConversationState;
use rustx::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    ContentBlockIndex, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::deadline::{
    ModelDeadlinePhase, ModelEventProgress, ModelRequestDeadline, ModelTimeoutPolicy,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::ModelUsage;
use rustx::runtime::continuation::{AnthropicContinuation, ProviderContinuationState};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId};
use rustx::runtime::{ManualMonotonicClock, MonotonicClock};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::ToolCallStart;
use support::context::{FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator};
use support::fake::{FakeModel, FakeStep, fake_model, model_release};

const CONVERSATION: &str = "conv-135";

fn conversation() -> ConversationId {
    ConversationId::new(CONVERSATION)
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

fn request(attempt: &str, model: &Arc<FakeModel>) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-135"),
        conversation_id: conversation(),
        attempt_id: AttemptId::new(attempt),
        conversation: ConversationState::from_messages(vec![user("seed", "hello")])
            .expect("valid fixture conversation"),
        initial_turn_trigger: rustx::runtime::inbound::InitialTurnTrigger::Continuation,
        model: support::attempt_model_with_window(model.clone(), "fake-model", 10_000_000, 512),
    }
}

fn runtime() -> ContextRuntime {
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 10_000_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(ScriptedEstimator::new(10, 10, 10)),
    )
    .expect("valid context configuration");
    ContextRuntime::with_scripted_summarizer(
        engine,
        Arc::new(FakeContextSummarizer::new(Vec::<FakeSummaryStep>::new())),
        AgentStatusEngine::default(),
        CompactionBudgets::new(1, 1, 1_000_000),
    )
}

async fn make_execution<'a>(
    attempt: &str,
    model: &Arc<FakeModel>,
    cancellation: &'a AgentCancellation,
    tool_runtime: &'a rustx::tools::runtime::ConversationToolRuntime,
    policy: ModelTimeoutPolicy,
    clock: Arc<ManualMonotonicClock>,
) -> AgentExecution<'a> {
    let capability = common::capability_lease(ToolRegistry::new(), tool_runtime).await;
    let mut execution = AgentExecution::new(
        request(attempt, model),
        capability.into_lease(),
        cancellation,
        runtime(),
        tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_publication_policy(
        rustx::publication::CoalescePolicy::default(),
        clock as Arc<dyn MonotonicClock>,
    );
    execution.install_model_timeout_policy(policy);
    execution
}

fn started() -> ModelEvent {
    ModelEvent::Started
}

fn text(text: &str) -> ModelEvent {
    ModelEvent::TextDelta {
        block_index: ContentBlockIndex::new(0),
        text: text.to_owned(),
    }
}

fn reasoning(text: &str) -> ModelEvent {
    ModelEvent::ReasoningDelta {
        block_index: ContentBlockIndex::new(0),
        text: text.to_owned(),
    }
}

fn usage(input_tokens: u64, output_tokens: u64) -> ModelEvent {
    ModelEvent::UsageUpdate {
        usage: ModelUsage {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
            details: None,
        },
    }
}

fn continuation() -> ModelEvent {
    ModelEvent::ContinuationState {
        block_index: ContentBlockIndex::new(0),
        state: ProviderContinuationState::Anthropic(AnthropicContinuation {
            opaque: serde_json::json!({"signature": "sig-135"}),
        }),
    }
}

fn completed() -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }
}

fn timeout_policy(response_start_ms: u64, stream_idle_ms: u64) -> ModelTimeoutPolicy {
    ModelTimeoutPolicy::new(
        Duration::from_millis(response_start_ms),
        Duration::from_millis(stream_idle_ms),
    )
}

fn summary_invocation(model: &Arc<FakeModel>) -> rustx::model::ResolvedModelInvocation {
    support::attempt_model_with_window(model.clone(), "fake-model", 10_000_000, 128)
        .summary_invocation()
        .clone()
}

/// Runs one request until its response-start or stream-idle timeout wins,
/// then cancels during the ordinary retry backoff so the test observes the
/// timeout without waiting for a second actual request.
async fn run_first_timeout(
    attempt: &str,
    prefix: Vec<ModelEvent>,
    policy: ModelTimeoutPolicy,
) -> (common::DurableExecutionAudit, Arc<FakeModel>) {
    let (release, release_rx) = model_release();
    let expected_events =
        u32::try_from(prefix.len()).expect("test event prefix length fits in u32");
    let mut script = prefix.into_iter().map(FakeStep::Emit).collect::<Vec<_>>();
    script.push(FakeStep::ParkUntilReleased(release_rx));
    let model = fake_model(vec![script]);
    let cancellation =
        AgentCancellation::new(rustx::runtime::types::CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let clock = Arc::new(ManualMonotonicClock::new());
    let (event_pause, mut reached, event_release) =
        crate::agent::execution::test_sync::ModelEventPause::install();
    let execution = make_execution(
        attempt,
        &model,
        &cancellation,
        &tool_runtime,
        policy,
        clock.clone(),
    )
    .await;
    let mut execution = execution;
    execution.install_model_event_pause(event_pause);
    let controller_clock = clock.clone();
    let controller_cancellation = cancellation.clone();
    let mut exited = model.streams_exited();
    let controller = tokio::spawn(async move {
        for count in 1..=expected_events {
            reached
                .wait_for(|observed| *observed >= count)
                .await
                .expect("model event pause remains open");
            event_release.send(()).expect("release model event pause");
        }
        controller_clock.advance(
            u64::try_from(policy.response_start_timeout.as_millis())
                .expect("test response timeout fits in milliseconds"),
        );
        exited
            .wait_for(|count| *count >= 1)
            .await
            .expect("timed-out stream exits");
        controller_cancellation.cancel();
        drop(release);
    });
    let audit = tokio::time::timeout(Duration::from_secs(2), async {
        let mut execution = execution;
        execution.observe(&publication);
        common::durable_agent_result_with_publication(
            execution.run().await,
            tool_runtime.durable_store().as_ref(),
            &publication,
        )
    })
    .await
    .expect("timeout test must settle without wall-clock waiting");
    controller.await.expect("timeout controller completes");
    (audit, model)
}

fn timeout_failures(
    audit: &common::DurableExecutionAudit,
) -> Vec<(
    rustx::runtime::identity::RequestId,
    ModelUsage,
    rustx::model::ModelErrorKind,
)> {
    audit
        .event_history
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ModelRequestFailed {
                request_id,
                error,
                usage: Some(usage),
            } => Some((request_id.clone(), usage.clone(), error.kind.clone())),
            RuntimeEvent::ModelRequestFailed {
                request_id, error, ..
            } => Some((
                request_id.clone(),
                ModelUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                    total_tokens: 0,
                    details: None,
                },
                error.kind.clone(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn deadline_classification_is_shared_and_phase_transitions_are_explicit() {
    let policy = timeout_policy(10, 20);
    let generation_events = [
        text("text"),
        reasoning("reasoning"),
        ModelEvent::ToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: ToolCallStart {
                id: ToolCallId::new("call-135"),
                tool_id: ToolId::new("tool-135"),
                name: "tool".to_owned(),
            },
        },
    ];
    for event in generation_events {
        assert_eq!(
            ModelEventProgress::classify(&event),
            ModelEventProgress::Generation
        );
        let clock = ManualMonotonicClock::new();
        let mut deadline = ModelRequestDeadline::new(policy, clock.now_millis());
        clock.advance(10);
        deadline.observe(&event, clock.now_millis());
        assert_eq!(deadline.phase(), ModelDeadlinePhase::Streaming);
        assert_eq!(deadline.deadline_millis(), Some(30));
    }
}

#[test]
fn early_usage_and_continuation_do_not_end_response_start_but_later_liveness_resets_idle() {
    let policy = timeout_policy(10, 20);
    let clock = ManualMonotonicClock::new();
    let mut deadline = ModelRequestDeadline::new(policy, clock.now_millis());
    deadline.observe(&ModelEvent::Started, clock.now_millis());
    clock.advance(5);
    deadline.observe(&usage(1, 1), clock.now_millis());
    deadline.observe(&continuation(), clock.now_millis());
    assert_eq!(deadline.phase(), ModelDeadlinePhase::AwaitingGeneration);
    assert_eq!(deadline.deadline_millis(), Some(10));
    deadline.observe(&text("first"), clock.now_millis());
    assert_eq!(deadline.deadline_millis(), Some(25));
    clock.advance(5);
    deadline.observe(&usage(2, 2), clock.now_millis());
    assert_eq!(deadline.deadline_millis(), Some(30));
    clock.advance(5);
    deadline.observe(&continuation(), clock.now_millis());
    assert_eq!(deadline.deadline_millis(), Some(35));
    clock.advance(5);
    deadline.observe(&text("second"), clock.now_millis());
    assert_eq!(deadline.deadline_millis(), Some(40));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn started_and_silence_reaches_response_start_timeout() {
    let (audit, model) = Box::pin(run_first_timeout(
        "attempt-135-start",
        vec![started()],
        timeout_policy(10, 20),
    ))
    .await;
    assert_eq!(model.requests().len(), 1);
    let failures = timeout_failures(&audit);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].2, rustx::model::ModelErrorKind::Timeout);
    assert_eq!(failures[0].0, audit.snapshot_history()[0].request_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn early_usage_and_continuation_still_reach_response_start_timeout() {
    for (attempt, event) in [
        ("attempt-135-usage", usage(3, 1)),
        ("attempt-135-cont", continuation()),
    ] {
        let (audit, model) = Box::pin(run_first_timeout(
            attempt,
            vec![started(), event],
            timeout_policy(10, 20),
        ))
        .await;
        assert_eq!(model.requests().len(), 1);
        assert_eq!(timeout_failures(&audit).len(), 1);
        assert!(matches!(
            audit
                .event_history
                .iter()
                .find(|event| matches!(event, RuntimeEvent::ModelRequestFailed { .. })),
            Some(RuntimeEvent::ModelRequestFailed { error, .. })
                if error.kind == rustx::model::ModelErrorKind::Timeout
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_and_liveness_progress_are_applied_by_the_primary_loop() {
    let (audit, model) = Box::pin(run_first_timeout(
        "attempt-135-usage-retained",
        vec![started(), text("partial"), usage(17, 5)],
        timeout_policy(100, 10),
    ))
    .await;
    assert_eq!(model.requests().len(), 1);
    let failures = timeout_failures(&audit);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].1.input_tokens, 17);
    assert_eq!(failures[0].1.output_tokens, 5);
    assert_eq!(failures[0].1.total_tokens, 22);

    let snapshot = serde_json::to_value(audit.snapshot_history()[0].clone())
        .expect("request snapshot serializes");
    assert!(snapshot.get("modelTimeoutPolicy").is_none());
    assert!(snapshot.get("responseStartTimeout").is_none());
    assert!(snapshot.get("streamIdleTimeout").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_adapter_terminal_after_timeout_cannot_create_a_second_outcome() {
    let (release, release_rx) = model_release();
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::ParkUntilReleased(release_rx),
        FakeStep::Emit(completed()),
    ]]);
    let cancellation =
        AgentCancellation::new(rustx::runtime::types::CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let clock = Arc::new(ManualMonotonicClock::new());
    let (event_pause, mut reached, event_release) =
        crate::agent::execution::test_sync::ModelEventPause::install();
    let mut execution = make_execution(
        "attempt-135-late-terminal",
        &model,
        &cancellation,
        &tool_runtime,
        timeout_policy(10, 20),
        clock.clone(),
    )
    .await;
    execution.install_model_event_pause(event_pause);
    let controller_clock = clock.clone();
    let controller_cancellation = cancellation.clone();
    let mut exited = model.streams_exited();
    let controller = tokio::spawn(async move {
        reached
            .wait_for(|observed| *observed >= 1)
            .await
            .expect("Started is processed");
        event_release.send(()).expect("release Started pause");
        controller_clock.advance(10);
        exited
            .wait_for(|count| *count >= 1)
            .await
            .expect("timed-out stream exits before late release");
        // This is the adapter's late completion opportunity. The primary
        // loop must have dropped the stream, so this cannot become another
        // provider outcome for the same RequestId.
        let _ = release.send_replace(true);
        controller_cancellation.cancel();
    });
    execution.observe(&publication);
    let audit = tokio::time::timeout(Duration::from_secs(2), async {
        common::durable_agent_result_with_publication(
            execution.run().await,
            tool_runtime.durable_store().as_ref(),
            &publication,
        )
    })
    .await
    .expect("late-terminal timeout test must settle");
    controller
        .await
        .expect("late-terminal controller completes");
    assert_eq!(model.requests().len(), 1);
    assert_eq!(model.emitted_count(), 1);
    assert_eq!(timeout_failures(&audit).len(), 1);
}

/// An observer sequence makes the simultaneous publication/timeout cut
/// visible: the committed-for-release frame must be observed before P.
#[derive(Default)]
struct OrderingObserver {
    order: Mutex<Vec<&'static str>>,
}

impl OrderingObserver {
    fn order(&self) -> Vec<&'static str> {
        self.order.lock().expect("ordering observer lock").clone()
    }
}

impl AgentExecutionObserver for OrderingObserver {
    fn observe_event(&self, _attempt_id: &AttemptId, event: &RuntimeEvent) {
        if matches!(event, RuntimeEvent::ModelRequestFailed { .. }) {
            self.order
                .lock()
                .expect("ordering observer lock")
                .push("failed");
        }
    }

    fn observe_committed(
        &self,
        _attempt_id: &AttemptId,
        _block: &MessageBlock,
        _transcript_cursor: Option<rustx::durable::TranscriptCursor>,
    ) {
    }

    fn observe_status(&self, _observation: &rustx::agent::AgentStatusObservation) {}

    fn observe_publication_opened(
        &self,
        _attempt_id: &AttemptId,
        _start: &rustx::publication::PublicationStreamStart,
    ) {
    }

    fn observe_publication(
        &self,
        _attempt_id: &AttemptId,
        _frame: &rustx::publication::PublicationFrame,
    ) {
        self.order
            .lock()
            .expect("ordering observer lock")
            .push("frame");
    }

    fn observe_publication_settled(
        &self,
        _attempt_id: &AttemptId,
        _audit: &rustx::publication::PublicationAudit,
        _transcript_cursor: rustx::durable::TranscriptCursor,
    ) {
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_flush_wins_before_timeout_at_the_same_cut_and_does_not_reset_idle() {
    let (_release, release_rx) = model_release();
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text("partial")),
        FakeStep::ParkUntilReleased(release_rx),
    ]]);
    let cancellation =
        AgentCancellation::new(rustx::runtime::types::CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let clock = Arc::new(ManualMonotonicClock::new());
    let (event_pause, mut reached, event_release) =
        crate::agent::execution::test_sync::ModelEventPause::install();
    let (arbitration_pause, mut arbitration_reached, arbitration_release) =
        crate::agent::execution::test_sync::ModelArbitrationPause::install(2);
    let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let mut execution = AgentExecution::new(
        request("attempt-135-publication", &model),
        capability.into_lease(),
        &cancellation,
        runtime(),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_publication_policy(
        rustx::publication::CoalescePolicy {
            max_bytes: 1_000,
            max_latency_millis: 5,
        },
        clock.clone() as Arc<dyn MonotonicClock>,
    );
    execution.install_model_timeout_policy(timeout_policy(100, 5));
    execution.install_model_event_pause(event_pause);
    execution.install_model_arbitration_pause(arbitration_pause);
    let observer = OrderingObserver::default();
    let controller_clock = clock.clone();
    let controller_cancellation = cancellation.clone();
    let mut exited = model.streams_exited();
    let controller = tokio::spawn(async move {
        for count in 1..=2 {
            reached
                .wait_for(|observed| *observed >= count)
                .await
                .expect("model event pause remains open");
            event_release.send(()).expect("release model event pause");
        }
        arbitration_reached
            .wait_for(|entered| *entered)
            .await
            .expect("publication arbitration pause remains open");
        // Text is fully published into the coalescer before this release.
        // Both its oldest-payload deadline and stream-idle deadline are now
        // ready at the same manual-clock cut.
        controller_clock.advance(5);
        arbitration_release
            .send(())
            .expect("release publication arbitration pause");
        exited
            .wait_for(|count| *count >= 1)
            .await
            .expect("timed-out stream exits");
        controller_cancellation.cancel();
    });
    let audit = tokio::time::timeout(Duration::from_secs(2), async {
        execution.observe(&observer);
        common::durable_agent_result(execution.run().await, tool_runtime.durable_store().as_ref())
    })
    .await
    .expect("publication timeout must settle");
    controller.await.expect("publication controller completes");
    assert_eq!(observer.order(), vec!["frame", "failed"]);
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestFailed { .. }))
            .count(),
        1
    );
    assert!(audit.event_history.iter().any(|event| matches!(
        event,
        RuntimeEvent::ModelRequestFailed { error, .. }
            if error.kind == rustx::model::ModelErrorKind::Timeout
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_event_wins_when_ready_with_response_timeout() {
    let (provider_release, provider_release_rx) = model_release();
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::ParkUntilReleased(provider_release_rx),
        FakeStep::Emit(text("first")),
        FakeStep::Emit(completed()),
    ]]);
    let cancellation =
        AgentCancellation::new(rustx::runtime::types::CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let clock = Arc::new(ManualMonotonicClock::new());
    let (event_pause, mut reached, event_release) =
        crate::agent::execution::test_sync::ModelEventPause::install();
    let (arbitration_pause, mut arbitration_reached, arbitration_release) =
        crate::agent::execution::test_sync::ModelArbitrationPause::install(1);
    let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let mut execution = AgentExecution::new(
        request("attempt-135-provider-cut", &model),
        capability.into_lease(),
        &cancellation,
        runtime(),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_publication_policy(
        rustx::publication::CoalescePolicy::default(),
        clock.clone() as Arc<dyn MonotonicClock>,
    );
    execution.install_model_timeout_policy(timeout_policy(10, 100));
    execution.install_model_event_pause(event_pause);
    execution.install_model_arbitration_pause(arbitration_pause);
    let controller = tokio::spawn(async move {
        reached
            .wait_for(|observed| *observed >= 1)
            .await
            .expect("Started is processed");
        event_release.send(()).expect("release Started pause");
        arbitration_reached
            .wait_for(|entered| *entered)
            .await
            .expect("provider arbitration pause remains open");
        // These operations happen without an await between them. The
        // provider release and response-start wake become ready before the
        // biased arbitration is released and polled.
        provider_release.send(true).expect("release provider event");
        clock.advance(10);
        arbitration_release
            .send(())
            .expect("release provider arbitration pause");
        reached
            .wait_for(|observed| *observed >= 2)
            .await
            .expect("generation event wins and is processed");
        event_release.send(()).expect("release generation pause");
        reached
            .wait_for(|observed| *observed >= 3)
            .await
            .expect("completed event is processed");
        event_release.send(()).expect("release completed pause");
    });
    let observer = common::RecordingPublicationObserver::default();
    execution.observe(&observer);
    let audit = tokio::time::timeout(Duration::from_secs(2), async {
        common::durable_agent_result(execution.run().await, tool_runtime.durable_store().as_ref())
    })
    .await
    .expect("provider-cut test must settle");
    controller.await.expect("provider-cut controller completes");
    assert!(matches!(audit.outcome, AttemptOutcome::Completed { .. }));
    assert_eq!(model.requests().len(), 1);
    assert!(
        !audit
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestFailed { .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_wins_same_cut_and_retains_cancellation_provenance() {
    let (_release, release_rx) = model_release();
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::ParkUntilReleased(release_rx),
    ]]);
    let cancellation =
        AgentCancellation::new(rustx::runtime::types::CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let clock = Arc::new(ManualMonotonicClock::new());
    let (event_pause, mut reached, event_release) =
        crate::agent::execution::test_sync::ModelEventPause::install();
    let (arbitration_pause, mut arbitration_reached, arbitration_release) =
        crate::agent::execution::test_sync::ModelArbitrationPause::install(1);
    let capability = common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let mut execution = AgentExecution::new(
        request("attempt-135-cancellation-cut", &model),
        capability.into_lease(),
        &cancellation,
        runtime(),
        &tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_publication_policy(
        rustx::publication::CoalescePolicy::default(),
        clock.clone() as Arc<dyn MonotonicClock>,
    );
    execution.install_model_timeout_policy(timeout_policy(10, 20));
    execution.install_model_event_pause(event_pause);
    execution.install_model_arbitration_pause(arbitration_pause);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        reached
            .wait_for(|observed| *observed >= 1)
            .await
            .expect("Started is processed");
        event_release.send(()).expect("release Started pause");
        arbitration_reached
            .wait_for(|entered| *entered)
            .await
            .expect("cancellation arbitration pause remains open");
        // Both cancellation and the response-start deadline are ready before
        // the biased arbitration is released and polled.
        clock.advance(10);
        controller_cancellation.cancel();
        arbitration_release
            .send(())
            .expect("release cancellation arbitration pause");
    });
    let observer = common::RecordingPublicationObserver::default();
    execution.observe(&observer);
    let audit = tokio::time::timeout(Duration::from_secs(2), async {
        common::durable_agent_result(execution.run().await, tool_runtime.durable_store().as_ref())
    })
    .await
    .expect("cancellation-cut test must settle");
    controller.await.expect("cancellation controller completes");
    assert!(cancellation.is_cancelled());
    assert!(matches!(audit.outcome, AttemptOutcome::Cancelled { .. }));
    assert!(
        !audit
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestFailed { .. }))
    );
    assert!(
        !audit
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_runtime_timeouts_use_the_bounded_generic_retry_budget_without_cancellation() {
    use crate::agent::execution::test_sync::RetryBackoffPause;

    let mut releases = Vec::new();
    let mut scripts = Vec::new();
    for _ in 0..4 {
        let (release, release_rx) = model_release();
        releases.push(release);
        scripts.push(vec![
            FakeStep::Emit(started()),
            FakeStep::ParkUntilReleased(release_rx),
        ]);
    }
    let model = fake_model(scripts);
    let cancellation =
        AgentCancellation::new(rustx::runtime::types::CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let clock = Arc::new(ManualMonotonicClock::new());
    let (pause, mut retry_reached, retry_release) = RetryBackoffPause::install();
    let mut execution = make_execution(
        "attempt-135-budget",
        &model,
        &cancellation,
        &tool_runtime,
        timeout_policy(10, 20),
        clock.clone(),
    )
    .await;
    execution.install_retry_backoff_pause(pause);
    let controller_clock = clock.clone();
    let mut emitted = model.emitted();
    let mut exited = model.streams_exited();
    let controller = tokio::spawn(async move {
        for request_number in 1..=4_u64 {
            emitted
                .wait_for(|count| *count >= request_number)
                .await
                .expect("request Started remains observable");
            controller_clock.advance(10);
            exited
                .wait_for(|count| *count >= request_number)
                .await
                .expect("timed-out request stream exits");
            if request_number < 4 {
                retry_reached
                    .wait_for(|count| {
                        *count
                            >= u32::try_from(request_number)
                                .expect("test request number fits in u32")
                    })
                    .await
                    .expect("retry backoff remains observable");
                retry_release.send(()).expect("release retry backoff");
                controller_clock.advance(2_000 * (1_u64 << (request_number - 1)));
            }
        }
        drop(releases);
    });
    execution.observe(&publication);
    let audit = tokio::time::timeout(Duration::from_secs(2), async {
        common::durable_agent_result_with_publication(
            execution.run().await,
            tool_runtime.durable_store().as_ref(),
            &publication,
        )
    })
    .await
    .expect("bounded timeout retries must settle");
    controller.await.expect("retry controller completes");
    assert!(
        !cancellation.is_cancelled(),
        "timeouts must not use AgentCancellation"
    );
    assert_eq!(model.requests().len(), 4);
    assert_eq!(timeout_failures(&audit).len(), 4);
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. }))
            .count(),
        3
    );
    assert!(matches!(
        audit.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model { ref error }
        } if error.kind == rustx::model::ModelErrorKind::Timeout
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_backed_summarizer_uses_the_same_deadlines_without_generic_retry() {
    for (attempt, script, expected) in [
        (
            "summary-start-timeout",
            vec![FakeStep::Emit(started())],
            "summary generation timed out",
        ),
        (
            "summary-idle-timeout",
            vec![FakeStep::Emit(started()), FakeStep::Emit(text("partial"))],
            "summary generation timed out",
        ),
    ] {
        let (release, release_rx) = model_release();
        let mut script = script;
        script.push(FakeStep::ParkUntilReleased(release_rx));
        let model = fake_model(vec![script]);
        let invocation = summary_invocation(&model);
        let clock = Arc::new(ManualMonotonicClock::new());
        let summarizer = ModelBackedSummarizer::new(
            invocation,
            timeout_policy(10, 10),
            clock.clone() as Arc<dyn MonotonicClock>,
        );
        let mut emitted = model.emitted();
        let mut exited = model.streams_exited();
        let advance_clock = clock.clone();
        let expected_events = if attempt == "summary-start-timeout" {
            1
        } else {
            2
        };
        let controller = tokio::spawn(async move {
            emitted
                .wait_for(|count| *count >= expected_events)
                .await
                .expect("summary events remain observable");
            advance_clock.advance(20);
            exited
                .wait_for(|count| *count >= 1)
                .await
                .expect("timed-out summary stream exits");
            drop(release);
        });
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            summarizer.summarize(
                SummaryRequest {
                    retired: vec![user("retired", "old")],
                },
                rustx::runtime::CancellationSignal::new(),
            ),
        )
        .await
        .expect("summary timeout must be deterministic")
        .expect_err("summary must fail on timeout");
        controller.await.expect("summary controller completes");
        assert_eq!(error.kind, rustx::context::ContextErrorKind::SummaryFailed);
        assert!(error.message.contains(expected));
        assert_eq!(model.requests().len(), 1, "summarizer has no generic retry");
    }
}
