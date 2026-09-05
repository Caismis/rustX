//! Issue #134: bounded generic model retries with deterministic frozen replay.
//!
//! These scenarios exercise the real Agent Loop, durable request-start
//! boundary, publication plane, and context runtime over the in-crate fake
//! provider. Retry timing is controlled by a monotonic manual clock and
//! synchronization hooks; no test relies on elapsed wall-clock time.

use super::super::{common, support};

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::context::{
    AgentStatusClock, AgentStatusConfig, AgentStatusEngine, AgentStatusModuleId,
    AgentStatusTestSeam, CompactionBudgets, ContextAssembly, ContextConfig, ContextEngine,
    ContextProposal, ContextRuntime, UserMessageProposal,
};
use rustx::conversation::ConversationState;
use rustx::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, ContentBlockIndex, InboundKind, MessageBlock, UserContentBlock,
    UserMessageBlock, UserSource,
};
use rustx::model::error::{
    ContextOverflowReport, ModelError, ModelErrorKind, ModelRetryDisposition,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::ModelUsage;
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId, RequestId};
use rustx::runtime::inbound::{FreshInboundTurn, InitialTurnTrigger};
use rustx::runtime::types::CancellationReason;
use rustx::runtime::{ManualMonotonicClock, MonotonicClock};
use rustx::tools::executor::ToolRegistry;
use support::context::{FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator};
use support::fake::{FakeModel, FakeStep, FakeTool, ScriptedCall, fake_model, tool_call_events};

const CONVERSATION: &str = "conv-134";

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

fn fresh_user(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: Some(
            chrono::DateTime::parse_from_rfc3339("2026-08-27T00:00:00Z")
                .expect("fixed inbound timestamp")
                .with_timezone(&chrono::Utc),
        ),
    })
}

fn request(
    attempt: &str,
    initial_messages: Vec<MessageBlock>,
    model: &Arc<FakeModel>,
    trigger: InitialTurnTrigger,
) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-134"),
        conversation_id: conversation(),
        attempt_id: AttemptId::new(attempt),
        conversation: ConversationState::from_messages(initial_messages)
            .expect("valid fixture conversation"),
        initial_turn_trigger: trigger,
        model: support::attempt_model_with_window(model.clone(), "fake-model", 10_000_000, 512),
    }
}

fn continuation_request(
    attempt: &str,
    initial_messages: Vec<MessageBlock>,
    model: &Arc<FakeModel>,
) -> AgentExecutionRequest {
    request(
        attempt,
        initial_messages,
        model,
        InitialTurnTrigger::Continuation,
    )
}

fn fresh_request(
    attempt: &str,
    initial_messages: Vec<MessageBlock>,
    model: &Arc<FakeModel>,
    inbound_id: &str,
) -> AgentExecutionRequest {
    request(
        attempt,
        initial_messages,
        model,
        InitialTurnTrigger::FreshInbound(
            FreshInboundTurn::new(vec![MessageId::new(inbound_id)])
                .expect("valid fresh inbound trigger"),
        ),
    )
}

fn runtime(summarizer_steps: Vec<FakeSummaryStep>) -> ContextRuntime {
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
        Arc::new(FakeContextSummarizer::new(summarizer_steps)),
        AgentStatusEngine::default(),
        CompactionBudgets::new(1, 1, 1_000_000),
    )
}

fn compacting_runtime(summarizer_steps: Vec<FakeSummaryStep>) -> ContextRuntime {
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 500,
            reserve_tokens: 0,
            keep_recent_tokens: 5,
        },
        Arc::new(ScriptedEstimator::new(100, 10, 0)),
    )
    .expect("valid context configuration");
    ContextRuntime::with_scripted_summarizer(
        engine,
        Arc::new(FakeContextSummarizer::new(summarizer_steps)),
        AgentStatusEngine::default(),
        CompactionBudgets::new(1, 1, 1_000_000),
    )
}

async fn make_execution<'a>(
    request: AgentExecutionRequest,
    tools: ToolRegistry,
    cancellation: &'a AgentCancellation,
    runtime: ContextRuntime,
    tool_runtime: &'a rustx::tools::runtime::ConversationToolRuntime,
) -> AgentExecution<'a> {
    let capability = common::capability_lease(tools, tool_runtime).await;
    AgentExecution::new(
        request,
        capability.into_lease(),
        cancellation,
        support::default_execution_policy(),
        runtime,
        tool_runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
}

async fn finish_execution<'a>(
    execution: AgentExecution<'a>,
    tool_runtime: &'a rustx::tools::runtime::ConversationToolRuntime,
    publication: &'a common::RecordingPublicationObserver,
) -> common::DurableExecutionAudit {
    let mut execution = execution;
    execution.observe(publication);
    common::durable_agent_result_with_publication(
        execution.run().await,
        tool_runtime.durable_store().as_ref(),
        publication,
    )
}

fn started_ids(events: &[RuntimeEvent]) -> Vec<RequestId> {
    events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ModelRequestStarted { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .collect()
}

fn retry_schedules(events: &[RuntimeEvent]) -> Vec<(RequestId, u32, Option<u64>)> {
    events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::ModelRetryScheduled {
                failed_request_id,
                retry_number,
                retry_delay_ms,
            } => Some((failed_request_id.clone(), *retry_number, *retry_delay_ms)),
            _ => None,
        })
        .collect()
}

fn assistant_texts(audit: &common::DurableExecutionAudit) -> Vec<String> {
    audit
        .messages()
        .iter()
        .filter_map(|message| match message {
            MessageBlock::Assistant(assistant) => Some(
                assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantContentBlock::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

fn transient_failure(message: &str, retry_after_ms: Option<u64>) -> ModelEvent {
    ModelEvent::Failed {
        error: ModelError {
            kind: ModelErrorKind::RateLimit,
            message: message.to_owned(),
            retry_disposition: ModelRetryDisposition::Transient,
            retry_after_ms,
            provider_code: Some("rate_limit_error".to_owned()),
            context_overflow: None,
            malformed_tool_proposal: None,
        },
    }
}

fn transient_transport_failure(message: &str, retry_after_ms: Option<u64>) -> ModelEvent {
    ModelEvent::Failed {
        error: ModelError {
            kind: ModelErrorKind::Transport,
            message: message.to_owned(),
            retry_disposition: ModelRetryDisposition::Transient,
            retry_after_ms,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
        },
    }
}

fn overflow_failure() -> ModelEvent {
    ModelEvent::Failed {
        error: ModelError {
            kind: ModelErrorKind::ContextWindowExceeded,
            message: "provider context window exceeded".to_owned(),
            retry_disposition: ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: Some("context_length_exceeded".to_owned()),
            context_overflow: Some(ContextOverflowReport {
                reported_input_tokens: None,
                context_limit: None,
            }),
            malformed_tool_proposal: None,
        },
    }
}

fn completed() -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }
}

fn completed_with_usage(input_tokens: u64, output_tokens: u64) -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: Some(ModelUsage {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
            details: None,
        }),
    }
}

#[derive(Debug, Clone, Copy)]
struct FixedStatusClock;

impl AgentStatusClock for FixedStatusClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-27T00:00:00Z")
            .expect("fixed status timestamp")
            .with_timezone(&chrono::Utc)
    }
}

/// A transient recovery is a new durable request generation with its own
/// snapshot, provisional Assistant identity, and publication stream, while
/// replaying the same provider-neutral request semantics.
#[tokio::test]
async fn transient_retry_uses_shared_identity_and_frozen_request() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "discarded".to_owned(),
            }),
            FakeStep::Emit(transient_failure("temporary throttle", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "accepted".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let execution = make_execution(
        continuation_request("attempt-134-identity", vec![user("seed", "hello")], &model),
        ToolRegistry::new(),
        &cancellation,
        runtime(Vec::new()),
        &tool_runtime,
    )
    .await;
    let audit = finish_execution(execution, &tool_runtime, &publication).await;

    assert!(matches!(
        audit.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert_eq!(model.requests().len(), 2);
    assert_eq!(started_ids(&audit.event_history).len(), 2);
    assert_eq!(
        audit
            .snapshot_history()
            .iter()
            .map(|snapshot| snapshot.identity.retry_number)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_ne!(
        audit.snapshot_history()[0].request_id,
        audit.snapshot_history()[1].request_id
    );
    assert_ne!(
        audit.snapshot_history()[0].provisional_message_id,
        audit.snapshot_history()[1].provisional_message_id
    );
    let opened = publication.opened();
    assert_eq!(opened.len(), 2);
    assert_ne!(opened[0].stream_id, opened[1].stream_id);
    assert_eq!(opened[0].request_id, audit.snapshot_history()[0].request_id);
    assert_eq!(opened[1].request_id, audit.snapshot_history()[1].request_id);
    assert_eq!(model.requests()[0], model.requests()[1]);
    assert_eq!(assistant_texts(&audit), vec!["accepted".to_owned()]);
    assert_eq!(audit.publication_audits.len(), 1);
    assert_eq!(
        audit.publication_audits[0].request_id,
        audit.snapshot_history()[0].request_id
    );
    assert!(
        audit
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestFailed { usage: None, .. }))
    );
}

/// The three transient retries use exactly 2/4/8 seconds, and the test
/// advances the captured absolute deadlines through one manual monotonic
/// clock instead of sleeping.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transient_backoff_is_deterministic_and_bounded_by_manual_clock() {
    use crate::agent::execution::test_sync::RetryBackoffPause;

    let failure = |number| {
        FakeStep::Emit(transient_failure(
            &format!("temporary failure {number}"),
            None,
        ))
    };
    let model = fake_model(vec![
        vec![FakeStep::Emit(ModelEvent::Started), failure(0)],
        vec![FakeStep::Emit(ModelEvent::Started), failure(1)],
        vec![FakeStep::Emit(ModelEvent::Started), failure(2)],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(completed()),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let clock = Arc::new(ManualMonotonicClock::new());
    let (pause, mut reached, release) = RetryBackoffPause::install();
    let execution = make_execution(
        continuation_request("attempt-134-backoff", vec![user("seed", "hello")], &model),
        ToolRegistry::new(),
        &cancellation,
        runtime(Vec::new()),
        &tool_runtime,
    )
    .await;
    let mut execution = execution;
    execution.install_publication_policy(
        rustx::publication::CoalescePolicy::default(),
        clock.clone() as Arc<dyn MonotonicClock>,
    );
    execution.install_retry_backoff_pause(pause);
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        for (ordinal, delay) in [(1_u32, 2_000_u64), (2, 4_000), (3, 8_000)] {
            reached
                .wait_for(|count| *count >= ordinal)
                .await
                .expect("retry backoff pause remains open");
            release.send(()).expect("release captured retry deadline");
            controller_clock.advance(delay);
        }
    });
    let audit = finish_execution(execution, &tool_runtime, &publication).await;
    controller.await.expect("backoff controller completes");

    assert_eq!(model.requests().len(), 4);
    assert_eq!(
        retry_schedules(&audit.event_history)
            .into_iter()
            .map(|(_, number, delay)| (number, delay))
            .collect::<Vec<_>>(),
        vec![(1, Some(2_000)), (2, Some(4_000)), (3, Some(8_000))]
    );
    assert_eq!(
        audit
            .snapshot_history()
            .iter()
            .map(|snapshot| snapshot.identity.retry_number)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_after_hint_overrides_default_and_is_capped() {
    use crate::agent::execution::test_sync::RetryBackoffPause;

    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure("provider retry hint", Some(120_000))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(completed()),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let clock = Arc::new(ManualMonotonicClock::new());
    let (pause, mut reached, release) = RetryBackoffPause::install();
    let mut execution = make_execution(
        continuation_request("attempt-134-hint", vec![user("seed", "hello")], &model),
        ToolRegistry::new(),
        &cancellation,
        runtime(Vec::new()),
        &tool_runtime,
    )
    .await;
    execution.install_publication_policy(
        rustx::publication::CoalescePolicy::default(),
        clock.clone() as Arc<dyn MonotonicClock>,
    );
    execution.install_retry_backoff_pause(pause);
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        reached
            .wait_for(|count| *count >= 1)
            .await
            .expect("retry backoff pause remains open");
        release.send(()).expect("release retry deadline");
        controller_clock.advance(60_000);
    });
    let audit = finish_execution(execution, &tool_runtime, &publication).await;
    controller.await.expect("hint controller completes");

    let schedules = retry_schedules(&audit.event_history);
    assert_eq!(schedules.len(), 1);
    assert_eq!(schedules[0].1, 1);
    assert_eq!(schedules[0].2, Some(60_000));
    assert!(schedules[0].0.as_str().ends_with(":0"));
    assert_eq!(model.requests().len(), 2);
}

/// Once three transient retries have been consumed, the fourth retryable
/// failure is terminal and no fifth request is started.
#[tokio::test]
async fn transient_retry_budget_exhaustion_is_terminal() {
    let model = fake_model(
        (0..4)
            .map(|number| {
                vec![
                    FakeStep::Emit(ModelEvent::Started),
                    FakeStep::Emit(transient_failure(&format!("failure {number}"), Some(0))),
                ]
            })
            .collect(),
    );
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let execution = make_execution(
        continuation_request("attempt-134-budget", vec![user("seed", "hello")], &model),
        ToolRegistry::new(),
        &cancellation,
        runtime(Vec::new()),
        &tool_runtime,
    )
    .await;
    let audit = finish_execution(execution, &tool_runtime, &publication).await;

    assert_eq!(model.requests().len(), 4);
    assert_eq!(started_ids(&audit.event_history).len(), 4);
    assert_eq!(retry_schedules(&audit.event_history).len(), 3);
    assert!(matches!(
        audit.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model { .. }
        }
    ));
    assert!(audit.event_history.iter().all(|event| {
        !matches!(event, RuntimeEvent::ModelRequestStarted { request_id, .. }
            if request_id.as_str().ends_with(":4"))
    }));
}

/// Transient and overflow recovery share one actual-request ordinal. A
/// transient retry before compaction does not reset the overflow budget, and
/// later transient failures do not reset the transient budget; the fifth
/// primary request is the terminal fourth transient failure.
#[tokio::test]
async fn transient_and_overflow_recovery_share_ordinal_and_budgets() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure("before overflow", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(overflow_failure()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure("after compaction 1", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure("after compaction 2", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure("exhausted", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(completed()),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let execution = make_execution(
        continuation_request(
            "attempt-134-composition",
            vec![user("seed", "hello")],
            &model,
        ),
        ToolRegistry::new(),
        &cancellation,
        compacting_runtime(vec![FakeSummaryStep::Return("summary".to_owned())]),
        &tool_runtime,
    )
    .await;
    let audit = finish_execution(execution, &tool_runtime, &publication).await;

    assert_eq!(model.requests().len(), 5);
    assert_eq!(
        audit
            .snapshot_history()
            .iter()
            .map(|snapshot| snapshot.identity.retry_number)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    let schedules = retry_schedules(&audit.event_history);
    assert_eq!(schedules.len(), 4);
    assert_eq!(
        schedules
            .iter()
            .map(|(_, number, _)| *number)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(
        schedules
            .iter()
            .map(|(_, _, delay)| *delay)
            .collect::<Vec<_>>(),
        vec![Some(0), None, Some(0), Some(0)]
    );
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. }))
            .count(),
        1
    );
    assert!(matches!(
        audit.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model { .. }
        }
    ));
}

/// An overflow recovery followed by a transient retry cannot reset the one
/// overflow-compaction budget: the second overflow is terminal at ordinal 2.
#[tokio::test]
async fn transient_retry_does_not_reset_overflow_budget() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(overflow_failure()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure("temporary", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(overflow_failure()),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let execution = make_execution(
        continuation_request(
            "attempt-134-overflow-budget",
            vec![user("seed", "hello")],
            &model,
        ),
        ToolRegistry::new(),
        &cancellation,
        compacting_runtime(vec![FakeSummaryStep::Return("summary".to_owned())]),
        &tool_runtime,
    )
    .await;
    let audit = finish_execution(execution, &tool_runtime, &publication).await;

    assert_eq!(model.requests().len(), 3);
    assert_eq!(
        audit
            .snapshot_history()
            .iter()
            .map(|snapshot| snapshot.identity.retry_number)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. }))
            .count(),
        1
    );
    assert_eq!(retry_schedules(&audit.event_history).len(), 2);
    assert!(matches!(
        audit.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model { .. }
        }
    ));
}

/// Inbound arriving after the failed request and before retry start remains
/// in the mailbox. It is admitted only at the following safe logical-step
/// boundary, never into the frozen retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbound_during_backoff_is_not_inserted_into_frozen_retry() {
    use crate::agent::execution::test_sync::RetryBackoffPause;

    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure("temporary", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(completed()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(completed()),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let clock = Arc::new(ManualMonotonicClock::new());
    let (pause, mut reached, release) = RetryBackoffPause::install();
    let execution = make_execution(
        continuation_request("attempt-134-inbound", vec![user("seed", "hello")], &model),
        ToolRegistry::new(),
        &cancellation,
        runtime(Vec::new()),
        &tool_runtime,
    )
    .await;
    let mut execution = execution;
    execution.install_publication_policy(
        rustx::publication::CoalescePolicy::default(),
        clock.clone() as Arc<dyn MonotonicClock>,
    );
    execution.install_retry_backoff_pause(pause);
    let mailbox = tool_runtime.mailbox();
    let controller_clock = clock.clone();
    let controller = tokio::spawn(async move {
        reached
            .wait_for(|count| *count >= 1)
            .await
            .expect("retry pause remains open");
        mailbox
            .enqueue(support::fake::inbound_message(
                "late-inbound",
                "arrived during backoff",
                rustx::message::types::UserSource::Human,
            ))
            .expect("late inbound is accepted into the mailbox");
        release.send(()).expect("release retry deadline");
        controller_clock.advance(0);
    });
    let audit = finish_execution(execution, &tool_runtime, &publication).await;
    controller.await.expect("inbound controller completes");

    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    assert!(!requests[1].messages.iter().any(|message| {
        matches!(message, rustx::model::ModelInputMessage::Canonical(MessageBlock::User(message)) if message.id == MessageId::new("late-inbound"))
    }));
    assert!(requests[2].messages.iter().any(|message| {
        matches!(message, rustx::model::ModelInputMessage::Canonical(MessageBlock::User(message)) if message.id == MessageId::new("late-inbound"))
    }));
    assert_eq!(
        audit
            .snapshot_history()
            .iter()
            .map(|snapshot| snapshot.identity.retry_number)
            .collect::<Vec<_>>(),
        vec![0, 1, 0]
    );
}

/// `FreshInbound`, Agent Status, and extension contributor admission happen
/// once for the logical step. The retry receives the exact admitted request
/// semantics and does not run another status/contributor generation.
#[tokio::test]
async fn transient_retry_does_not_regenerate_status_or_contributors() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure("temporary", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(completed()),
        ],
    ]);
    let contributor_calls = Arc::new(AtomicUsize::new(0));
    let calls = contributor_calls.clone();
    let mut assembly = ContextAssembly::new();
    assembly
        .register_extension(
            "issue134.frozen",
            Some("package-1".to_owned()),
            Arc::new(move |_: &rustx::context::ContributorInputSnapshot| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "accepted contributor".to_owned(),
                    })],
                })])
            }),
        )
        .expect("register extension contributor");
    let seam = AgentStatusTestSeam::new();
    let status_engine =
        AgentStatusEngine::new(AgentStatusConfig::default(), Arc::new(FixedStatusClock))
            .with_test_seam(seam.clone());
    let context_engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 10_000_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(ScriptedEstimator::new(10, 10, 10)),
    )
    .expect("valid context configuration");
    let runtime = ContextRuntime::with_scripted_summarizer_and_assembly(
        context_engine,
        Arc::new(FakeContextSummarizer::new(Vec::new())),
        status_engine,
        assembly,
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let execution = make_execution(
        fresh_request(
            "attempt-134-status",
            vec![fresh_user("fresh-inbound", "new work")],
            &model,
            "fresh-inbound",
        ),
        ToolRegistry::new(),
        &cancellation,
        runtime,
        &tool_runtime,
    )
    .await;
    let audit = finish_execution(execution, &tool_runtime, &publication).await;

    assert_eq!(model.requests().len(), 2);
    assert_eq!(model.requests()[0], model.requests()[1]);
    assert_eq!(contributor_calls.load(Ordering::SeqCst), 1);
    assert_eq!(seam.capture_count(AgentStatusModuleId::Time), 1);
    assert_eq!(seam.evaluate_count(AgentStatusModuleId::Time), 1);
    assert_eq!(
        audit.snapshot_history()[0].context_generation,
        audit.snapshot_history()[1].context_generation
    );
    assert_eq!(
        audit.snapshot_history()[0].agent_status,
        audit.snapshot_history()[1].agent_status
    );
}

/// Partial failed output settles as a noncanonical publication audit before
/// the next stream opens; only the successful request reaches canonical
/// Assistant acceptance.
#[tokio::test]
async fn failed_partial_publication_settles_before_retry_and_stays_noncanonical() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "failed partial".to_owned(),
            }),
            FakeStep::Emit(transient_transport_failure("temporary", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "final answer".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let execution = make_execution(
        continuation_request(
            "attempt-134-publication",
            vec![user("seed", "hello")],
            &model,
        ),
        ToolRegistry::new(),
        &cancellation,
        runtime(Vec::new()),
        &tool_runtime,
    )
    .await;
    let audit = finish_execution(execution, &tool_runtime, &publication).await;

    assert_eq!(model.requests().len(), 2);
    assert_eq!(audit.snapshot_history().len(), 2);
    let r0 = audit.snapshot_history()[0].request_id.clone();
    let r1 = audit.snapshot_history()[1].request_id.clone();
    assert_ne!(r0, r1);
    assert_eq!(
        audit
            .snapshot_history()
            .iter()
            .map(|snapshot| snapshot.identity.retry_number)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        retry_schedules(&audit.event_history),
        vec![(r0.clone(), 1, Some(0))]
    );
    assert!(audit.event_history.iter().any(|event| matches!(
        event,
        RuntimeEvent::ModelRequestFailed {
            request_id,
            error,
            ..
        } if request_id == &r0
            && error.kind == ModelErrorKind::Transport
            && error.retry_disposition == ModelRetryDisposition::Transient
    )));
    assert_eq!(publication.audits().len(), 1);
    assert_eq!(
        publication.audits()[0].kind,
        rustx::publication::PublicationAuditKind::Incomplete
    );
    assert!(publication.audits()[0].content.iter().any(|block| {
        matches!(
            block,
            rustx::publication::PublicationAuditBlock::Text { text, .. }
                if text == "failed partial"
        )
    }));
    let trace = publication.trace();
    assert!(matches!(
        trace.as_slice(),
        [
            common::PublicationObservation::Opened(_),
            common::PublicationObservation::Settled(
                _,
                rustx::publication::PublicationAuditKind::Incomplete
            ),
            common::PublicationObservation::Opened(_),
        ]
    ));
    assert_eq!(assistant_texts(&audit), vec!["final answer".to_owned()]);
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AssistantMessageCommitted { .. }))
            .count(),
        1
    );
    assert!(publication.released_text().contains("failed partial"));
}

/// A completed proposed `ToolCall` from a failed request remains an audit-only
/// publication and never crosses the Tool Plane execution frontier.
#[tokio::test]
async fn failed_tool_call_proposal_never_executes() {
    let scripted = ScriptedCall {
        id: "failed-call",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let first = std::iter::once(FakeStep::Emit(ModelEvent::Started))
        .chain(
            tool_call_events(0, &scripted)
                .into_iter()
                .map(FakeStep::Emit),
        )
        .chain(std::iter::once(FakeStep::Emit(
            transient_transport_failure("temporary", Some(0)),
        )))
        .collect();
    let model = fake_model(vec![
        first,
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(completed()),
        ],
    ]);
    let fake_tool = FakeTool::new(
        common::tool("alpha", "tool-alpha"),
        support::fake::success_result("should not run"),
    );
    let calls = fake_tool.calls();
    let mut tools = ToolRegistry::new();
    fake_tool.register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let execution = make_execution(
        continuation_request(
            "attempt-134-tool-proposal",
            vec![user("seed", "hello")],
            &model,
        ),
        tools,
        &cancellation,
        runtime(Vec::new()),
        &tool_runtime,
    )
    .await;
    let audit = finish_execution(execution, &tool_runtime, &publication).await;

    assert!(calls.borrow().is_empty(), "failed proposal never executes");
    assert_eq!(audit.publication_audits.len(), 1);
    assert_eq!(
        audit.publication_audits[0].proposed_call_ids(),
        vec![rustx::runtime::identity::ToolCallId::new("failed-call")]
    );
    assert!(audit.event_history.iter().all(|event| {
        !matches!(
            event,
            RuntimeEvent::ToolExecutionStarted { .. } | RuntimeEvent::ToolExecutionCompleted { .. }
        )
    }));
}

/// Cancellation is placed after the schedule commit and captured deadline,
/// while the retry waits. The existing attempt cancellation settlement wins;
/// no retry start or provider invocation occurs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_during_retry_backoff_prevents_next_start() {
    use crate::agent::execution::test_sync::RetryBackoffPause;

    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure("temporary", None)),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(completed()),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let clock = Arc::new(ManualMonotonicClock::new());
    let (pause, mut reached, release) = RetryBackoffPause::install();
    let execution = make_execution(
        continuation_request(
            "attempt-134-cancel-backoff",
            vec![user("seed", "hello")],
            &model,
        ),
        ToolRegistry::new(),
        &cancellation,
        runtime(Vec::new()),
        &tool_runtime,
    )
    .await;
    let mut execution = execution;
    execution.install_publication_policy(
        rustx::publication::CoalescePolicy::default(),
        clock as Arc<dyn MonotonicClock>,
    );
    execution.install_retry_backoff_pause(pause);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        reached
            .wait_for(|count| *count >= 1)
            .await
            .expect("retry deadline captured");
        // At this point ModelRetryScheduled is durably committed and the
        // backoff hook has captured the absolute deadline, but R1 has not
        // entered start arbitration.
        controller_cancellation.cancel();
        release.send(()).expect("release cancelled backoff");
    });
    let audit = finish_execution(execution, &tool_runtime, &publication).await;
    controller.await.expect("cancellation controller completes");

    assert_eq!(model.requests().len(), 1);
    assert_eq!(started_ids(&audit.event_history).len(), 1);
    assert_eq!(retry_schedules(&audit.event_history).len(), 1);
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
            .count(),
        1
    );
    assert!(matches!(
        audit.event_history.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
}

/// The retry start uses the same pre-start cancellation/start gate as an
/// ordinary model request: cancellation before that gate's arbitration
/// prevents both the durable start fact and provider invocation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_at_retry_start_frontier_uses_normal_arbitration() {
    use crate::agent::execution::test_sync::StartBoundaryPause;

    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure("temporary", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(completed()),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
    let mut pre_start = pre_start.expect("pre-start phase installed");
    let execution = make_execution(
        continuation_request(
            "attempt-134-cancel-frontier",
            vec![user("seed", "hello")],
            &model,
        ),
        ToolRegistry::new(),
        &cancellation,
        runtime(Vec::new()),
        &tool_runtime,
    )
    .await;
    let mut execution = execution;
    execution.install_start_boundary_pause(pause);
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        pre_start.await_park(1).await;
        pre_start.release();
        pre_start.await_park(2).await;
        controller_cancellation.cancel();
        pre_start.release();
    });
    let audit = finish_execution(execution, &tool_runtime, &publication).await;
    controller.await.expect("frontier controller completes");

    assert_eq!(model.requests().len(), 1);
    assert_eq!(started_ids(&audit.event_history).len(), 1);
    assert_eq!(retry_schedules(&audit.event_history).len(), 1);
    assert!(matches!(
        audit.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
}

/// Known cumulative usage before a failed stream is retained exactly once;
/// missing usage stays `None`, and no generated-text estimate is fabricated.
#[tokio::test]
async fn failed_request_usage_is_latest_known_snapshot_and_not_duplicated() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::UsageUpdate {
                usage: ModelUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    total_tokens: 12,
                    details: None,
                },
            }),
            FakeStep::Emit(ModelEvent::UsageUpdate {
                usage: ModelUsage {
                    input_tokens: 11,
                    output_tokens: 3,
                    total_tokens: 14,
                    details: None,
                },
            }),
            FakeStep::Emit(transient_failure("temporary", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(completed_with_usage(20, 4)),
        ],
    ]);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let execution = make_execution(
        continuation_request("attempt-134-usage", vec![user("seed", "hello")], &model),
        ToolRegistry::new(),
        &cancellation,
        runtime(Vec::new()),
        &tool_runtime,
    )
    .await;
    let audit = finish_execution(execution, &tool_runtime, &publication).await;

    let failed_usage = audit.event_history.iter().find_map(|event| match event {
        RuntimeEvent::ModelRequestFailed { usage, .. } => usage.clone(),
        _ => None,
    });
    assert_eq!(
        failed_usage,
        Some(ModelUsage {
            input_tokens: 11,
            output_tokens: 3,
            total_tokens: 14,
            details: None,
        })
    );
    assert_eq!(
        audit
            .event_history
            .iter()
            .filter_map(|event| match event {
                RuntimeEvent::ModelRequestFailed { usage, .. } => Some(usage.clone()),
                _ => None,
            })
            .count(),
        1
    );
    assert!(audit.event_history.iter().any(|event| {
        matches!(
            event,
            RuntimeEvent::ModelRequestCompleted {
                usage: Some(ModelUsage {
                    input_tokens: 20,
                    output_tokens: 4,
                    total_tokens: 24,
                    ..
                }),
                ..
            }
        )
    }));

    let no_usage_model = fake_model(vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(transient_failure("no usage evidence", Some(0))),
    ]]);
    let no_usage_tool_runtime = common::tool_runtime(CONVERSATION);
    let no_usage_publication = common::RecordingPublicationObserver::default();
    let no_usage_execution = make_execution(
        continuation_request(
            "attempt-134-no-usage",
            vec![user("seed-no-usage", "hello")],
            &no_usage_model,
        ),
        ToolRegistry::new(),
        &cancellation,
        runtime(Vec::new()),
        &no_usage_tool_runtime,
    )
    .await;
    let no_usage_audit = finish_execution(
        no_usage_execution,
        &no_usage_tool_runtime,
        &no_usage_publication,
    )
    .await;
    assert!(
        no_usage_audit
            .event_history
            .iter()
            .any(|event| { matches!(event, RuntimeEvent::ModelRequestFailed { usage: None, .. }) })
    );
}

/// A successful prior turn establishes continuation; a transient failure in
/// the next logical step replays that same continuation without mutation.
#[tokio::test]
async fn transient_retry_preserves_frozen_provider_continuation() {
    let continuation = rustx::runtime::continuation::OpenAiResponsesContinuation::Stored {
        previous_response_id: "response-134".to_owned(),
    };
    let call = ScriptedCall {
        id: "continuation-call",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    };
    let first_turn = std::iter::once(FakeStep::Emit(ModelEvent::Started))
        .chain(std::iter::once(FakeStep::Emit(
            ModelEvent::ContinuationState {
                block_index: ContentBlockIndex::new(0),
                state: rustx::runtime::continuation::ProviderContinuationState::OpenAiResponses(
                    continuation.clone(),
                ),
            },
        )))
        .chain(tool_call_events(1, &call).into_iter().map(FakeStep::Emit))
        .chain(std::iter::once(FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
        })))
        .collect();
    let model = fake_model(vec![
        first_turn,
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure("temporary", Some(0))),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(completed()),
        ],
    ]);
    let mut tools = ToolRegistry::new();
    FakeTool::new(
        common::tool("alpha", "tool-alpha"),
        support::fake::success_result("tool result"),
    )
    .register(&mut tools);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let publication = common::RecordingPublicationObserver::default();
    let execution = make_execution(
        continuation_request(
            "attempt-134-continuation",
            vec![user("seed", "hello")],
            &model,
        ),
        tools,
        &cancellation,
        runtime(Vec::new()),
        &tool_runtime,
    )
    .await;
    let audit = finish_execution(execution, &tool_runtime, &publication).await;

    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[1].continuation,
        Some(
            rustx::runtime::continuation::ProviderContinuationState::OpenAiResponses(
                continuation.clone(),
            ),
        )
    );
    assert_eq!(requests[2].continuation, requests[1].continuation);
    assert_eq!(
        audit
            .snapshot_history()
            .iter()
            .map(|snapshot| snapshot.identity.retry_number)
            .collect::<Vec<_>>(),
        vec![0, 0, 1]
    );
}

/// The retry event serializes its failed request correlation and next ordinal;
/// it does not persist a speculative next request identity.
#[test]
fn retry_schedule_wire_shape_has_failed_request_and_no_next_request_id() {
    let event = RuntimeEvent::ModelRetryScheduled {
        failed_request_id: RequestId::new("request-r0"),
        retry_number: 1,
        retry_delay_ms: Some(2_000),
    };
    let value = serde_json::to_value(event).expect("serialize retry schedule");
    assert_eq!(value["failed_request_id"], "request-r0");
    assert_eq!(value["retry_number"], 1);
    assert_eq!(value["retry_delay_ms"], 2_000);
    assert!(value.get("next_request_id").is_none());
}
