//! Issue #137: one-shot unresolved model-output carryover.
//!
//! These tests drive the real Agent Loop and durable `SQLite` authority. The
//! publication audit is produced by one failed primary request, selected by
//! identity, consumed by the next eligible primary start, and reconstructed
//! from the frozen Request Snapshot without consulting live pending state.

use super::{common, support};

use chrono::TimeZone;
use std::sync::Arc;

use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest, AttemptLifecycle};
use rustx::context::{
    AgentStatusEngine, CompactionBudgets, ContextConfig, ContextEngine, ContextRuntime,
};
use rustx::conversation::ConversationState;
use rustx::durable::{ConversationStore, SqliteConversationStore};
use rustx::events::types::{AttemptFailure, AttemptOutcome, RuntimeEvent};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    ContentBlockIndex, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::error::{ModelError, ModelErrorKind, ModelRetryDisposition};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::{
    CarryoverBlockKind, CarryoverDetailLevel, CarryoverOmissionCounts, ModelInputMessage,
    RenderedCarryoverRecord, RenderedCarryoverText, RenderedUnresolvedOutputCarryover,
    RequestOnlyInsertionAnchor, RequestOnlyModelContext, UnresolvedOutputSettlement,
};
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId};
use rustx::runtime::inbound::{FreshInboundTurn, InitialTurnTrigger};
use rustx::runtime::types::{CancellationReason, RuntimeClock};
use rustx::tools::executor::ToolRegistry;
use support::context::{FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator};
use support::fake::{FakeModel, FakeStep, fake_model};

const CONVERSATION: &str = "conv-137";

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

fn timestamped_user(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: Some(
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 28, 12, 1, 0)
                .single()
                .expect("valid fresh inbound timestamp"),
        ),
    })
}

fn runtime_with_window(context_window_tokens: u64) -> ContextRuntime {
    runtime_with_estimator(
        context_window_tokens,
        Arc::new(ScriptedEstimator::new(10, 10, 10)),
        Vec::new(),
    )
}

fn runtime_with_estimator(
    context_window_tokens: u64,
    estimator: Arc<ScriptedEstimator>,
    summarizer_steps: Vec<FakeSummaryStep>,
) -> ContextRuntime {
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator,
    )
    .expect("valid context configuration");
    ContextRuntime::with_scripted_summarizer(
        engine,
        Arc::new(FakeContextSummarizer::new(summarizer_steps)),
        AgentStatusEngine::default(),
        CompactionBudgets::new(1, 1, 1_000_000),
    )
}

fn runtime() -> ContextRuntime {
    runtime_with_window(10_000_000)
}

fn request(
    attempt: &str,
    conversation: ConversationState,
    model: &Arc<FakeModel>,
) -> AgentExecutionRequest {
    request_with_window(attempt, conversation, model, 10_000_000, 512)
}

fn request_with_window(
    attempt: &str,
    conversation: ConversationState,
    model: &Arc<FakeModel>,
    context_window_tokens: u64,
    max_output_tokens: u32,
) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-137"),
        conversation_id: ConversationId::new(CONVERSATION),
        attempt_id: AttemptId::new(attempt),
        conversation,
        initial_turn_trigger: InitialTurnTrigger::Continuation,
        model: support::attempt_model_with_window(
            model.clone(),
            "fake-model",
            context_window_tokens,
            max_output_tokens,
        ),
    }
}

async fn execution<'a>(
    request: AgentExecutionRequest,
    cancellation: &'a AgentCancellation,
    tool_runtime: &'a common::ToolRuntimeFixture,
) -> AgentExecution<'a> {
    execution_with_runtime(request, cancellation, tool_runtime, runtime()).await
}

async fn execution_with_runtime<'a>(
    request: AgentExecutionRequest,
    cancellation: &'a AgentCancellation,
    tool_runtime: &'a common::ToolRuntimeFixture,
    context_runtime: ContextRuntime,
) -> AgentExecution<'a> {
    let capability = common::capability_lease(ToolRegistry::new(), tool_runtime).await;
    AgentExecution::new(
        request,
        capability.into_lease(),
        cancellation,
        support::default_execution_policy(),
        context_runtime,
        tool_runtime,
        AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
}

#[derive(Debug, Clone, Copy)]
struct RecoveryClock;

impl RuntimeClock for RecoveryClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 8, 28, 12, 0, 0)
            .single()
            .expect("valid fixed recovery time")
    }
}

fn failed_partial_output() -> ModelEvent {
    ModelEvent::Failed {
        error: ModelError {
            kind: ModelErrorKind::Transport,
            message: "connection interrupted after partial output".to_owned(),
            retry_disposition: ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
        },
    }
}

fn completed() -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }
}

fn transient_failure() -> ModelEvent {
    ModelEvent::Failed {
        error: ModelError {
            kind: ModelErrorKind::RateLimit,
            message: "temporary provider failure".to_owned(),
            retry_disposition: ModelRetryDisposition::Transient,
            retry_after_ms: Some(0),
            provider_code: Some("rate_limit_error".to_owned()),
            context_overflow: None,
        },
    }
}

fn carryover_from(request: &rustx::model::ModelRequest) -> &RenderedUnresolvedOutputCarryover {
    request
        .messages
        .iter()
        .find_map(|message| match message {
            ModelInputMessage::RequestOnly(RequestOnlyModelContext::UnresolvedOutputCarryover(
                carryover,
            )) => Some(carryover),
            ModelInputMessage::Canonical(_) => None,
        })
        .expect("request contains carryover")
}

#[tokio::test]
async fn live_terminal_transition_installs_then_consumes_one_frozen_carryover() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "partial answer".to_owned(),
            }),
            FakeStep::Emit(failed_partial_output()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "recovered answer".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let first_cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut first_execution = execution(
        request(
            "attempt-137-source",
            ConversationState::from_messages(vec![user("user-1", "continue")])
                .expect("valid conversation"),
            &model,
        ),
        &first_cancellation,
        &tool_runtime,
    )
    .await;
    let first_publication = common::RecordingPublicationObserver::default();
    first_execution.observe(&first_publication);
    let first_audit = common::durable_agent_result_with_publication(
        first_execution.run().await,
        tool_runtime.durable_store().as_ref(),
        &first_publication,
    );

    assert!(matches!(
        first_audit.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model { .. }
        }
    ));
    assert_eq!(first_audit.publication_audits.len(), 1);
    let source = first_audit.publication_audits[0].stream_id.clone();
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("pending source lookup"),
        Some(source.clone())
    );
    assert!(
        first_audit
            .messages()
            .iter()
            .all(|message| { !matches!(message, MessageBlock::Assistant(_)) })
    );
    let first_conversation = first_audit.result.conversation;

    let second_cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut second_execution = execution(
        request("attempt-137-consumer", first_conversation, &model),
        &second_cancellation,
        &tool_runtime,
    )
    .await;
    let second_publication = common::RecordingPublicationObserver::default();
    second_execution.observe(&second_publication);
    let second_audit = common::durable_agent_result_with_publication(
        second_execution.run().await,
        tool_runtime.durable_store().as_ref(),
        &second_publication,
    );

    assert!(
        matches!(
            second_audit.outcome,
            AttemptOutcome::Completed {
                finish_reason: ModelFinishReason::Stop
            }
        ),
        "second outcome: {:?}; events: {:?}",
        second_audit.outcome,
        second_audit.event_history
    );
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("pending source consumed"),
        None
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let carryover_position = requests[1]
        .messages
        .iter()
        .position(|message| {
            matches!(
                message,
                ModelInputMessage::RequestOnly(RequestOnlyModelContext::UnresolvedOutputCarryover(
                    carryover
                )) if carryover.source_stream_id == source
                    && carryover.source_settlement == UnresolvedOutputSettlement::Incomplete
            )
        })
        .expect("the second eligible primary request carries the source");
    assert_eq!(
        carryover_position, 1,
        "continuation anchor is after canonical history"
    );
    assert!(matches!(
        requests[1].messages[0],
        ModelInputMessage::Canonical(MessageBlock::User(_))
    ));
    let snapshot = second_audit
        .snapshot_history()
        .first()
        .expect("consumer snapshot");
    assert_eq!(snapshot.unresolved_output_carryover_source, Some(source));
    assert_eq!(
        snapshot
            .unresolved_output_carryover
            .as_ref()
            .map(|carryover| carryover.source_settlement),
        Some(UnresolvedOutputSettlement::Incomplete)
    );
    assert_eq!(
        snapshot.unresolved_output_carryover_anchor,
        Some(rustx::model::RequestOnlyInsertionAnchor::AfterCanonical,)
    );
    let reconstructed = tool_runtime
        .durable_store()
        .reconstruct_model_request(&snapshot.request_id)
        .expect("historical request reconstruction");
    assert_eq!(reconstructed, requests[1]);
    assert_eq!(
        carryover_from(&reconstructed).source_settlement,
        UnresolvedOutputSettlement::Incomplete,
        "snapshot reconstruction preserves the frozen settlement without rereading the audit"
    );
    assert!(second_audit.event_history.iter().any(|event| matches!(
        event,
        RuntimeEvent::ModelRequestStarted { request_id, .. }
            if request_id == &snapshot.request_id
    )));
}

#[tokio::test]
async fn fresh_inbound_anchor_places_carryover_before_the_pending_inbound_turn() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "partial before fresh inbound".to_owned(),
            }),
            FakeStep::Emit(failed_partial_output()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "fresh inbound accepted".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let first = execution(
        request(
            "attempt-137-fresh-source",
            ConversationState::from_messages(vec![user("user-1", "old history")])
                .expect("valid conversation"),
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    let _first = common::durable_agent_result(first, tool_runtime.durable_store().as_ref());
    let source = tool_runtime
        .durable_store()
        .load_pending_unresolved_output_stream_id()
        .expect("pending source")
        .expect("source exists");

    let fresh = timestamped_user("fresh-1", "new inbound semantics");
    tool_runtime
        .durable_store()
        .append_canonical(&fresh)
        .expect("append fresh inbound to canonical authority");
    let conversation = ConversationState::from_messages(
        tool_runtime
            .durable_store()
            .load_canonical()
            .expect("reload canonical history"),
    )
    .expect("rebuild conversation with fresh inbound");
    let mut consumer_request = request("attempt-137-fresh-consumer", conversation, &model);
    consumer_request.initial_turn_trigger = InitialTurnTrigger::FreshInbound(
        FreshInboundTurn::new(vec![MessageId::new("fresh-1")])
            .expect("valid fresh inbound trigger"),
    );
    let consumer = execution(consumer_request, &cancellation, &tool_runtime)
        .await
        .run()
        .await;
    let consumer = common::durable_agent_result(consumer, tool_runtime.durable_store().as_ref());

    assert!(matches!(
        consumer.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    let request = &model.requests()[1];
    let fresh_position = request
        .messages
        .iter()
        .position(|message| {
            matches!(
                message,
                ModelInputMessage::Canonical(MessageBlock::User(user))
                    if user.id == MessageId::new("fresh-1")
            )
        })
        .expect("fresh inbound is in the request");
    assert!(fresh_position > 0);
    assert!(matches!(
        &request.messages[fresh_position - 1],
        ModelInputMessage::RequestOnly(RequestOnlyModelContext::UnresolvedOutputCarryover(carryover))
            if carryover.source_stream_id == source
    ));
    assert_eq!(
        consumer.snapshot_history()[0].unresolved_output_carryover_anchor,
        Some(RequestOnlyInsertionAnchor::BeforeMessage(MessageId::new(
            "fresh-1",
        )))
    );
    let reconstructed = tool_runtime
        .durable_store()
        .reconstruct_model_request(&consumer.snapshot_history()[0].request_id)
        .expect("fresh request reconstructs exactly");
    assert_eq!(reconstructed, *request);
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("fresh consumer consumes source"),
        None
    );
}

#[tokio::test]
async fn live_producer_terminal_and_carryover_pointer_are_one_durable_transition() {
    let store = Arc::new(
        SqliteConversationStore::in_memory(ConversationId::new(CONVERSATION))
            .expect("in-memory store"),
    );
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "partial before the terminal transaction".to_owned(),
            }),
            FakeStep::Emit(failed_partial_output()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "accepted after recovery".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let tool_runtime = common::tool_runtime_with_store(
        CONVERSATION,
        Some(store.clone() as Arc<dyn ConversationStore>),
    );
    store.arm_fail_next_terminal_event();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let failed = execution(
        request(
            "attempt-137-live-producer",
            ConversationState::from_messages(vec![user("user-1", "recover this")])
                .expect("valid conversation"),
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;

    assert!(matches!(
        failed.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model { .. }
        }
    ));
    assert_eq!(store.terminal_event_attempts(), 1);
    assert_eq!(
        store
            .load_pending_unresolved_output_stream_id()
            .expect("pending source"),
        None,
        "the terminal transaction failure cannot expose a pointer without its terminal event"
    );
    let snapshots = common::read_request_snapshot_history(store.as_ref(), &failed.attempt_id);
    let source = rustx::runtime::identity::PublicationStreamId::for_request(
        &snapshots[0].identity.attempt_id,
        &snapshots[0].provisional_message_id,
    );
    assert!(
        store
            .load_publication_audit(&source)
            .expect("durable publication audit")
            .is_some(),
        "the publication evidence precedes the producer transition"
    );
    let events = common::read_event_history(store.as_ref(), &failed.attempt_id);
    assert!(
        events.iter().all(|event| {
            !matches!(
                event,
                RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptTimedOut { .. }
                    | RuntimeEvent::AttemptLimitExceeded { .. }
                    | RuntimeEvent::AttemptFailed { .. }
            )
        }),
        "the failed terminal transaction leaves no half-committed terminal fact"
    );

    let recovered = rustx::runtime::recovery::recover(store.as_ref(), &RecoveryClock)
        .expect("recovery retries the semantic producer transition");
    assert_eq!(
        recovered.reconciliation().attempt_terminal,
        Some(failed.attempt_id.clone())
    );
    assert_eq!(
        store
            .load_pending_unresolved_output_stream_id()
            .expect("recovered source"),
        Some(source.clone())
    );
    let repeated = rustx::runtime::recovery::recover(store.as_ref(), &RecoveryClock)
        .expect("a second recovery is idempotent");
    assert!(repeated.reconciliation().is_empty());
    assert_eq!(
        store
            .load_pending_unresolved_output_stream_id()
            .expect("source remains pending"),
        Some(source.clone())
    );

    let consumer = execution(
        request("attempt-137-live-consumer", failed.conversation, &model),
        &cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    assert!(matches!(
        consumer.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert_eq!(
        store
            .load_pending_unresolved_output_stream_id()
            .expect("source consumed"),
        None
    );
    assert_eq!(
        carryover_from(&model.requests()[1]).source_stream_id,
        source
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_before_consumer_start_preserves_pointer_until_a_later_start() {
    use crate::agent::execution::test_sync::StartBoundaryPause;

    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "partial source".to_owned(),
            }),
            FakeStep::Emit(failed_partial_output()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "eventually accepted".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let first_cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let first = execution(
        request(
            "attempt-137-cancel-source",
            ConversationState::from_messages(vec![user("user-1", "continue")])
                .expect("valid conversation"),
            &model,
        ),
        &first_cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    let first = common::durable_agent_result(first, tool_runtime.durable_store().as_ref());
    let source = tool_runtime
        .durable_store()
        .load_pending_unresolved_output_stream_id()
        .expect("pending source")
        .expect("source exists");

    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
    let mut pre_start = pre_start.expect("pre-start pause");
    let cancel_for_controller = cancellation.clone();
    let controller = tokio::spawn(async move {
        pre_start.await_park(1).await;
        cancel_for_controller.cancel();
        pre_start.release();
    });
    let mut consumer = execution(
        request(
            "attempt-137-cancelled-consumer",
            first.result.conversation,
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await;
    consumer.install_start_boundary_pause(pause);
    let cancelled = consumer.run().await;
    controller.await.expect("cancellation controller");
    assert!(matches!(
        cancelled.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
    assert_eq!(
        model.requests().len(),
        1,
        "the cancelled consumer never reached the provider"
    );
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("pending source survives cancellation"),
        Some(source.clone())
    );

    let later_cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let later = execution(
        request("attempt-137-later-consumer", cancelled.conversation, &model),
        &later_cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    assert!(matches!(
        later.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert_eq!(model.requests().len(), 2);
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("later successful start consumes once"),
        None
    );
}

#[tokio::test]
async fn transient_retry_reuses_the_same_frozen_carryover_and_consumes_once() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "partial source".to_owned(),
            }),
            FakeStep::Emit(failed_partial_output()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(transient_failure()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "retry accepted".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let first = execution(
        request(
            "attempt-137-retry-source",
            ConversationState::from_messages(vec![user("user-1", "continue")])
                .expect("valid conversation"),
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    let first = common::durable_agent_result(first, tool_runtime.durable_store().as_ref());
    let first_conversation = first.result.conversation;
    let consumer_cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let consumer = execution(
        request("attempt-137-retry-consumer", first_conversation, &model),
        &consumer_cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    let consumer = common::durable_agent_result(consumer, tool_runtime.durable_store().as_ref());

    assert!(matches!(
        consumer.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(carryover_from(&requests[1]), carryover_from(&requests[2]));
    assert_eq!(
        carryover_from(&requests[1]).source_stream_id,
        carryover_from(&requests[2]).source_stream_id
    );
    assert_eq!(
        consumer
            .snapshot_history()
            .iter()
            .map(|snapshot| snapshot.identity.retry_number)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert!(consumer.snapshot_history().iter().all(|snapshot| {
        snapshot.unresolved_output_carryover_source.is_some()
            && snapshot.unresolved_output_carryover_anchor
                == Some(RequestOnlyInsertionAnchor::AfterCanonical)
    }));
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("pointer consumed by initial logical-step start"),
        None
    );
}

#[tokio::test]
async fn internal_retry_success_does_not_install_carryover_from_recovered_generations() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "partial retry generation zero".to_owned(),
            }),
            FakeStep::Emit(transient_failure()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "partial retry generation one".to_owned(),
            }),
            FakeStep::Emit(transient_failure()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "accepted retry generation two".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut run = execution(
        request(
            "attempt-137-internal-retry-success",
            ConversationState::from_messages(vec![user("user-1", "retry internally")])
                .expect("valid conversation"),
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await;
    let publication = common::RecordingPublicationObserver::default();
    run.observe(&publication);
    let audit = common::durable_agent_result_with_publication(
        run.run().await,
        tool_runtime.durable_store().as_ref(),
        &publication,
    );

    assert!(matches!(
        audit.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert_eq!(model.requests().len(), 3);
    assert_eq!(audit.publication_audits.len(), 2);
    assert!(
        audit
            .snapshot_history()
            .iter()
            .all(|snapshot| snapshot.unresolved_output_carryover_source.is_none())
    );
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("internal retry success has no carryover"),
        None
    );
    assert!(audit.messages().iter().any(|message| {
        matches!(
            message,
            MessageBlock::Assistant(assistant)
                if assistant.content.iter().any(|block| matches!(
                    block,
                    rustx::message::types::AssistantContentBlock::Text(text)
                        if text.text == "accepted retry generation two"
                ))
        )
    }));
}

#[tokio::test]
async fn retry_exhaustion_selects_one_highest_meaningful_generation() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "generation zero".to_owned(),
            }),
            FakeStep::Emit(transient_failure()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "generation one".to_owned(),
            }),
            FakeStep::Emit(transient_failure()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "generation two".to_owned(),
            }),
            FakeStep::Emit(transient_failure()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(failed_partial_output()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "accepted after exhaustion".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let failed = execution(
        request(
            "attempt-137-exhaustion",
            ConversationState::from_messages(vec![user("user-1", "retry until exhausted")])
                .expect("valid conversation"),
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    let failed = common::durable_agent_result(failed, tool_runtime.durable_store().as_ref());

    assert!(matches!(
        failed.outcome,
        AttemptOutcome::Failed {
            error: AttemptFailure::Model { .. }
        }
    ));
    assert_eq!(model.requests().len(), 4);
    assert_eq!(failed.snapshot_history().len(), 4);
    let selected = rustx::runtime::identity::PublicationStreamId::for_request(
        &failed.snapshot_history()[2].identity.attempt_id,
        &failed.snapshot_history()[2].provisional_message_id,
    );
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("selected exhaustion source"),
        Some(selected.clone())
    );
    let consumer = execution(
        request(
            "attempt-137-exhaustion-consumer",
            failed.result.conversation,
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    assert!(matches!(
        consumer.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert_eq!(
        carryover_from(&model.requests()[4]).source_stream_id,
        selected
    );
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("exhaustion source consumed once"),
        None
    );
}

#[tokio::test]
async fn a_new_unresolved_step_replaces_the_consumed_source_instead_of_accumulating() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "source A".to_owned(),
            }),
            FakeStep::Emit(failed_partial_output()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "source B".to_owned(),
            }),
            FakeStep::Emit(failed_partial_output()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "accepted after replacement".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let first = execution(
        request(
            "attempt-137-replacement-source",
            ConversationState::from_messages(vec![user("user-1", "continue")])
                .expect("valid conversation"),
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    let first = common::durable_agent_result(first, tool_runtime.durable_store().as_ref());
    let source_a = tool_runtime
        .durable_store()
        .load_pending_unresolved_output_stream_id()
        .expect("source A pointer")
        .expect("source A exists");

    let second = execution(
        request(
            "attempt-137-replacement-step",
            first.result.conversation,
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    let second = common::durable_agent_result(second, tool_runtime.durable_store().as_ref());
    let source_b = tool_runtime
        .durable_store()
        .load_pending_unresolved_output_stream_id()
        .expect("source B pointer")
        .expect("new unresolved source replaces A");
    assert_ne!(source_a, source_b);

    let third = execution(
        request(
            "attempt-137-replacement-final",
            second.result.conversation,
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    assert!(matches!(
        third.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));

    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(carryover_from(&requests[1]).source_stream_id, source_a);
    assert_eq!(carryover_from(&requests[2]).source_stream_id, source_b);
    assert_ne!(
        carryover_from(&requests[2]).source_stream_id,
        carryover_from(&requests[1]).source_stream_id
    );
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("replacement source is consumed"),
        None
    );
}

#[tokio::test]
async fn canonical_frontier_has_no_audit_carryover_or_request_only_item() {
    let model = fake_model(vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "canonical answer".to_owned(),
        }),
        FakeStep::Emit(completed()),
    ]]);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let mut run = execution(
        request(
            "attempt-137-canonical-frontier",
            ConversationState::from_messages(vec![user("user-1", "answer")])
                .expect("valid conversation"),
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await;
    let publication = common::RecordingPublicationObserver::default();
    run.observe(&publication);
    let audit = common::durable_agent_result_with_publication(
        run.run().await,
        tool_runtime.durable_store().as_ref(),
        &publication,
    );
    assert!(matches!(
        audit.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert!(audit.publication_audits.is_empty());
    assert!(
        model.requests()[0]
            .messages
            .iter()
            .all(|message| matches!(message, ModelInputMessage::Canonical(_)))
    );
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("no unresolved source"),
        None
    );
}

#[test]
fn request_only_anchors_and_degradation_are_deterministic() {
    let source = rustx::runtime::identity::PublicationStreamId::new("audit-source");
    let carryover = RenderedUnresolvedOutputCarryover {
        source_stream_id: source.clone(),
        source_settlement: UnresolvedOutputSettlement::Incomplete,
        records: vec![RenderedCarryoverRecord::Text(RenderedCarryoverText {
            kind: CarryoverBlockKind::Text,
            text: Some("tail".to_owned()),
            omitted_prefix_bytes: 4,
            omitted_detail_bytes: 0,
        })],
        omitted_blocks: CarryoverOmissionCounts::default(),
    };
    let canonical = vec![user("history", "old")];
    let fresh = vec![user("fresh-1", "new"), user("fresh-2", "also new")];
    let before_fresh = rustx::model::input::assemble_model_input(
        &canonical,
        &fresh,
        Some(&carryover),
        Some(&RequestOnlyInsertionAnchor::BeforeMessage(MessageId::new(
            "fresh-1",
        ))),
    )
    .expect("fresh anchor");
    assert!(matches!(
        before_fresh[1],
        ModelInputMessage::RequestOnly(RequestOnlyModelContext::UnresolvedOutputCarryover(_))
    ));
    assert!(matches!(
        before_fresh[2],
        ModelInputMessage::Canonical(MessageBlock::User(_))
    ));
    let after_canonical = rustx::model::input::assemble_model_input(
        &canonical,
        &[],
        Some(&carryover),
        Some(&RequestOnlyInsertionAnchor::AfterCanonical),
    )
    .expect("continuation anchor");
    assert!(matches!(
        after_canonical[1],
        ModelInputMessage::RequestOnly(RequestOnlyModelContext::UnresolvedOutputCarryover(_))
    ));

    let reduced = carryover.degraded(CarryoverDetailLevel::Reduced);
    assert_eq!(reduced, carryover);
    let metadata = reduced.degraded(CarryoverDetailLevel::MetadataOnly);
    let RenderedCarryoverRecord::Text(text) = &metadata.records[0] else {
        panic!("text record remains metadata");
    };
    assert!(text.text.is_none());
    assert_eq!(text.omitted_detail_bytes, 4);
    assert_eq!(
        metadata.source_settlement,
        UnresolvedOutputSettlement::Incomplete
    );
    assert_eq!(metadata.omitted_blocks, carryover.omitted_blocks);
    assert_eq!(metadata.records[0].block_kind(), CarryoverBlockKind::Text);
    assert!(metadata.render().contains("source_settlement=incomplete"));
    let omitted = metadata.degraded(CarryoverDetailLevel::Omitted);
    assert_eq!(omitted.source_stream_id, source);
    assert!(omitted.records.iter().all(|record| match record {
        RenderedCarryoverRecord::Text(text) => text.text.is_none(),
        RenderedCarryoverRecord::ProposedToolCall(call) => call.arguments.is_none(),
    }));
}

#[tokio::test]
async fn carryover_degrades_and_is_omitted_before_cannot_fit_or_compaction() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "source output".to_owned(),
            }),
            FakeStep::Emit(failed_partial_output()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "accepted with auxiliary context omitted".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let first = execution(
        request(
            "attempt-137-fit-source",
            ConversationState::from_messages(vec![user("user-1", "fit this")])
                .expect("valid conversation"),
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    let first = common::durable_agent_result(first, tool_runtime.durable_store().as_ref());

    let second = execution_with_runtime(
        request_with_window(
            "attempt-137-fit-consumer",
            first.result.conversation,
            &model,
            48,
            1,
        ),
        &cancellation,
        &tool_runtime,
        runtime_with_window(48),
    )
    .await
    .run()
    .await;
    let second = common::durable_agent_result(second, tool_runtime.durable_store().as_ref());
    assert!(matches!(
        second.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert!(
        second.event_history.iter().all(|event| {
            !matches!(
                event,
                RuntimeEvent::CompactionStarted | RuntimeEvent::CompactionCompleted { .. }
            )
        }),
        "carryover alone never triggers compaction"
    );
    let snapshot = second
        .snapshot_history()
        .first()
        .expect("fit consumer snapshot");
    assert!(snapshot.unresolved_output_carryover_source.is_some());
    assert!(
        snapshot.unresolved_output_carryover.is_none(),
        "the request-only value reaches the omitted rung before CannotFit"
    );
    assert!(
        model.requests()[1]
            .messages
            .iter()
            .all(|message| matches!(message, ModelInputMessage::Canonical(_)))
    );
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("fit consumer consumes source"),
        None
    );
}

#[tokio::test]
async fn overflow_compaction_only_degrades_the_frozen_carryover_source() {
    let model = fake_model(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "x".repeat(2_048),
            }),
            FakeStep::Emit(failed_partial_output()),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "provider rejected the full request".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Failed {
                error: ModelError {
                    kind: ModelErrorKind::ContextWindowExceeded,
                    message: "provider context window exceeded".to_owned(),
                    retry_disposition: ModelRetryDisposition::Never,
                    retry_after_ms: None,
                    provider_code: Some("context_length_exceeded".to_owned()),
                    context_overflow: None,
                },
            }),
        ],
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "accepted after carryover degradation".to_owned(),
            }),
            FakeStep::Emit(completed()),
        ],
    ]);
    let tool_runtime = common::tool_runtime(CONVERSATION);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let first = execution(
        request(
            "attempt-137-overflow-source",
            ConversationState::from_messages(vec![user("user-1", "make room")])
                .expect("valid conversation"),
            &model,
        ),
        &cancellation,
        &tool_runtime,
    )
    .await
    .run()
    .await;
    let first = common::durable_agent_result(first, tool_runtime.durable_store().as_ref());

    let summary = "s".repeat(360);
    let consumer_runtime = runtime_with_estimator(
        700,
        Arc::new(ScriptedEstimator::new(100, 10, 0)),
        vec![FakeSummaryStep::Return(summary)],
    );
    let consumer = execution_with_runtime(
        request_with_window(
            "attempt-137-overflow-consumer",
            first.result.conversation,
            &model,
            10_000_000,
            512,
        ),
        &cancellation,
        &tool_runtime,
        consumer_runtime,
    )
    .await
    .run()
    .await;
    let consumer = common::durable_agent_result(consumer, tool_runtime.durable_store().as_ref());

    assert!(matches!(
        consumer.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert!(
        consumer
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionCompleted { .. }))
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    let full = carryover_from(&requests[1]);
    let degraded = carryover_from(&requests[2]);
    assert_eq!(full.source_stream_id, degraded.source_stream_id);
    assert_eq!(
        full.source_settlement,
        UnresolvedOutputSettlement::Incomplete
    );
    assert_eq!(
        degraded.source_settlement,
        UnresolvedOutputSettlement::Incomplete
    );
    assert!(full.records.iter().any(|record| {
        matches!(
            record,
            RenderedCarryoverRecord::Text(text) if text.text.is_some()
        )
    }));
    assert!(degraded.records.iter().all(|record| match record {
        RenderedCarryoverRecord::Text(text) => text.text.is_none(),
        RenderedCarryoverRecord::ProposedToolCall(call) => call.arguments.is_none(),
    }));
    let snapshots = consumer.snapshot_history();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(
        snapshots[0].unresolved_output_carryover_source,
        snapshots[1].unresolved_output_carryover_source
    );
    assert_eq!(
        snapshots[0]
            .unresolved_output_carryover
            .as_ref()
            .map(|carryover| carryover.source_settlement),
        Some(UnresolvedOutputSettlement::Incomplete)
    );
    assert_eq!(
        snapshots[1]
            .unresolved_output_carryover
            .as_ref()
            .map(|carryover| carryover.source_settlement),
        Some(UnresolvedOutputSettlement::Incomplete)
    );
    assert_eq!(
        snapshots[0].unresolved_output_carryover_anchor,
        snapshots[1].unresolved_output_carryover_anchor
    );
    assert!(
        snapshots[0]
            .unresolved_output_carryover
            .as_ref()
            .is_some_and(|carryover| carryover.records.iter().any(|record| {
                matches!(
                    record,
                    RenderedCarryoverRecord::Text(text) if text.text.is_some()
                )
            }))
    );
    assert!(
        snapshots[1]
            .unresolved_output_carryover
            .as_ref()
            .is_some_and(
                |carryover| carryover.records.iter().all(|record| match record {
                    RenderedCarryoverRecord::Text(text) => text.text.is_none(),
                    RenderedCarryoverRecord::ProposedToolCall(call) => call.arguments.is_none(),
                })
            )
    );
    assert_eq!(
        tool_runtime
            .durable_store()
            .load_pending_unresolved_output_stream_id()
            .expect("successful consumer clears source"),
        None
    );
}
