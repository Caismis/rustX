use super::publish_model_turn_start_events;
use crate::agent::cancellation::AgentCancellation;
use crate::agent::execution::start::RequestObservationIdentity;
use crate::agent::execution::test_fixtures::*;
use crate::agent::execution::test_sync::StartBoundaryPause;
use crate::agent::execution::{AgentExecution, AgentExecutionRequest};
use crate::context::{ContextRuntime, ObservedAnchor};
use crate::conversation::ConversationState;
use crate::durable::inbox::ConversationStore;
use crate::durable::{ModelTurnStartCommit, ModelTurnStartCommitDisposition};
use crate::events::types::{AttemptOutcome, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{ContextKind, InboundKind, MessageBlock, UserContentBlock, UserSource};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::{AttemptId, ConversationId, EventId, MessageId, RequestId, TurnId};
use crate::runtime::inbound::InitialTurnTrigger;
use crate::runtime::types::CancellationReason;
use crate::scripted_suites::common::{capability_lease, tool_runtime, tool_runtime_with_store};
use crate::scripted_suites::support::fake::inbound_message;
use crate::tools::executor::ToolRegistry;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tokio::sync::watch;

fn observer_start_commit(disposition: ModelTurnStartCommitDisposition) -> ModelTurnStartCommit {
    let attempt_id = AttemptId::new("observer-attempt");
    let turn = TurnId::new("1");
    let request_id = RequestId::new("observer-request");
    let message_id = MessageId::new("observer-status");
    let timestamp = chrono::DateTime::from_timestamp(0, 0).expect("timestamp");
    let emission = crate::message::types::ContributionEmission {
        key: "active_actionable".to_owned(),
        fingerprint: "observer-fingerprint".to_owned(),
    };
    ModelTurnStartCommit {
        verified_snapshot: None,
        status_sources: Vec::new(),
        context_receipts: Vec::new(),
        started: RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: EventId::new("observer-start"),
            sequence: 1,
            conversation_id: ConversationId::new("conv_2287bce3-d81b-7f16-8c57-98493481c6cc"),
            attempt_id: Some(attempt_id.clone()),
            turn_id: Some(turn.clone()),
            timestamp,
            event: RuntimeEvent::ModelRequestStarted {
                request_id: request_id.clone(),
                model: "observer-model".to_owned(),
            },
        },
        contribution_emissions: vec![RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: EventId::new("observer-emission"),
            sequence: 2,
            conversation_id: ConversationId::new("conv_2287bce3-d81b-7f16-8c57-98493481c6cc"),
            attempt_id: Some(attempt_id),
            turn_id: Some(turn),
            timestamp,
            event: RuntimeEvent::ContextContributionEmitted {
                producer: crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus,
                ),
                request_id,
                message_id,
                emission,
                logical_step_origin: 1,
            },
        }],
        disposition,
    }
}

#[test]
fn idempotent_start_receipt_does_not_republish_live_facts() {
    let observer = RecordingObserver::default();
    let attempt_id = AttemptId::new("observer-attempt");
    let first = observer_start_commit(ModelTurnStartCommitDisposition::NewlyCommitted);
    let replay = observer_start_commit(ModelTurnStartCommitDisposition::IdempotentReplay);

    publish_model_turn_start_events(Some(&observer), &attempt_id, &first);
    publish_model_turn_start_events(Some(&observer), &attempt_id, &replay);

    let expected = first
        .events()
        .map(|event| event.event.clone())
        .collect::<Vec<_>>();
    assert_eq!(observer.events(), expected);
}

/// A provider-reported measurement is evidence about the exact request
/// the provider measured. The opaque provider continuation is part of
/// that request and is invisible to the conversation projection, so a
/// changed continuation withdraws the evidence instead of letting the
/// context engine anchor a confident, provider-attributed number onto a
/// request no provider ever measured.
#[tokio::test]
async fn a_measurement_does_not_survive_a_changed_provider_continuation() {
    use crate::context::ProviderObservedInput;
    // The continuation value comes from a scripted helper: this kernel
    // treats continuation state as opaque and must never name a
    // provider protocol, not even in a test.
    use crate::scripted_suites::support::model::scripted_continuation;

    let adapter = Arc::new(ScriptedAdapter::new(Vec::new()));
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let mut execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("execution construction");

    // A request measured with no continuation at all.
    let measured_identity = Arc::new(RequestObservationIdentity {
        invocation: execution.request.model.primary().invocation_config(),
        continuation: None,
    });
    execution.observed = Some(ProviderObservedInput {
        input_tokens: 1_234,
        anchor: ObservedAnchor::of_selected(
            crate::conversation::SurfaceRevision::new(0),
            &crate::model::SelectedModelInput::default(),
            "",
            &[],
        ),
    });
    execution.observed_request_identity = Some(measured_identity);
    assert!(
        execution.reusable_observation().is_some(),
        "the measurement applies to the request it was taken under"
    );

    // The next request continues a stored provider generation. Its
    // visible messages may still extend the measured ones, but the
    // provider now bills the referenced generation as input too.
    execution.pending_continuation = Some(scripted_continuation("generation-1"));
    assert!(
        execution.reusable_observation().is_none(),
        "a measurement from a different continuation is not evidence about this request"
    );

    // A different continued generation is just as foreign as the first.
    execution.pending_continuation = Some(scripted_continuation("generation-2"));
    assert!(execution.reusable_observation().is_none());

    // Returning to the measured identity returns the evidence: the gate
    // withholds a measurement, it does not destroy one.
    execution.pending_continuation = None;
    assert!(execution.reusable_observation().is_some());
}

/// The provider boundary is strictly after the durable request-start
/// transaction. A deterministic request-start fault therefore leaves no
/// started snapshot and the adapter is never called.
#[tokio::test]
async fn provider_is_not_invoked_before_durable_request_start_commit() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }]]));
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .expect("in-memory store"),
    );
    store.arm_request_start_fault_script([
        crate::durable::sqlite::RequestStartFaultOperation::BeforeContextAppend,
    ]);
    let tool_runtime = tool_runtime_with_store(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        Some(store.clone()),
    );
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);

    let result = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("execution construction")
    .run()
    .await;

    assert_eq!(
        adapter.request_count(),
        0,
        "provider starts after durable commit"
    );
    assert!(result.durable_failure.is_some());
    assert!(
        store
            .read_request_snapshots(None, 32)
            .expect("request snapshots")
            .snapshots
            .is_empty()
    );
    assert!(
        !store
            .read_events(None, 32)
            .expect("event journal")
            .events
            .iter()
            .any(|envelope| matches!(envelope.event, RuntimeEvent::ModelRequestStarted { .. }))
    );
}

/// A request + context runtime with one counting test contributor that
/// returns a single User-context proposal.
fn counting_context_request(
    adapter: &Arc<ScriptedAdapter>,
) -> (
    AgentExecutionRequest,
    ContextRuntime,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let invocation_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count_for_contributor = Arc::clone(&invocation_count);
    let mut assembly = crate::context::ContextAssembly::new();
    assembly
        .register_extension(
            "test.context",
            Some("package-v1".to_owned()),
            Arc::new(move |_: &crate::context::ContributorInputSnapshot| {
                count_for_contributor.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(vec![crate::context::ContextProposal::UserMessage(
                    crate::context::UserMessageProposal {
                        content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                            text: "proposal exists before start".to_owned(),
                        })],
                    },
                )])
            }),
        )
        .expect("register test contributor");
    let request = request(adapter);
    let runtime = ContextRuntime::for_attempt_with_assembly(
        crate::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(crate::context::DefaultTokenEstimator),
        Some(crate::context::AgentStatusEngine::default()),
        assembly,
        &request.model,
        crate::model::ModelTimeoutPolicy::default(),
        crate::scripted_suites::support::default_monotonic_clock(),
    )
    .expect("valid context runtime");
    (request, runtime, invocation_count)
}

/// Issue #12 (M9b), cancellation wins before start: Context Assembly is
/// allowed to finish while cancellation is observable, but the one
/// model-turn start gate is the linearization point. The execution parks
/// immediately before the arbitration; the cancellation fully completes
/// while parked (provably linearized first); the released arbitration
/// observes it and discards the prepared turn: no provider request, no
/// `ModelRequestStarted`, no started Request Snapshot, and no
/// request-scoped context commit.
///
/// `TurnStarted` is present: it records that the loop began preparing
/// the logical turn, which genuinely happened; it implies no request,
/// no context commit, and no provider side effect. The attempt terminal
/// is unique and last.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_before_start_arbitration_commits_nothing() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }]]));
    let (request, runtime, invocation_count) = counting_context_request(&adapter);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
    let mut pre_start = pre_start.expect("pre-start phase installed");
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        pre_start.await_park(1).await;
        controller_cancellation.cancel();
        // The cancellation fully completed while the execution was
        // parked before the arbitration: it provably linearized first.
        assert!(controller_cancellation.is_cancelled());
        pre_start.release();
    });
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let mut execution = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_start_boundary_pause(pause);
    let result = execution.run().await;
    controller.await.expect("cancellation controller");
    let store = tool_runtime.durable_store();
    let snapshots = request_snapshot_history(store.as_ref());
    let events = event_history(store.as_ref());

    assert_eq!(
        invocation_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the finite contributor ran once and was never rerun"
    );
    assert!(matches!(
        result.outcome,
        crate::events::types::AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
    assert!(snapshots.is_empty(), "no started Request Snapshot exists");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. })),
        "no request-start fact exists"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::TurnStarted)),
        "TurnStarted records that turn preparation began"
    );
    assert!(
        matches!(events.last(), Some(RuntimeEvent::AttemptCancelled { .. }))
            && events
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
                .count()
                == 1,
        "the cancellation terminal is unique and last: {events:?}"
    );
    assert_eq!(
        result.conversation.revision(),
        crate::conversation::SurfaceRevision::INITIAL
    );
    assert!(
        result.messages().is_empty(),
        "no request-scoped context was committed"
    );
    assert_eq!(
        adapter.request_count(),
        0,
        "the provider request never started"
    );
}

/// Issue #129: a `FreshInbound` status prepared before the start gate is
/// provisional. If cancellation wins that gate, neither its canonical
/// User context message nor its structured observation is published.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_before_model_start_publishes_no_agent_status() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }]]));
    let mut request = request(&adapter);
    let inbound = inbound_message("fresh-1", "fresh input", UserSource::Human);
    request.conversation = ConversationState::from_messages(vec![MessageBlock::User(inbound)])
        .expect("canonical inbound history");
    request.initial_turn_trigger = InitialTurnTrigger::FreshInbound(
        crate::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("fresh-1")])
            .expect("fresh trigger"),
    );
    let runtime = runtime(&adapter);
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
    let mut pre_start = pre_start.expect("pre-start phase installed");
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        pre_start.await_park(1).await;
        controller_cancellation.cancel();
        assert!(controller_cancellation.is_cancelled());
        pre_start.release();
    });
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let observer = RecordingObserver::default();
    let mut execution = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_start_boundary_pause(pause);
    execution.observe(&observer);
    let result = execution.run().await;
    controller.await.expect("cancellation controller");

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Cancelled {
            reason: CancellationReason::UserRequested
        }
    ));
    assert_eq!(adapter.request_count(), 0, "the model never started");
    assert!(
        observer
            .committed
            .lock()
            .expect("observer committed lock")
            .iter()
            .all(|block| !matches!(
                block,
                MessageBlock::User(user)
                    if matches!(
                        &user.kind,
                        InboundKind::Context(ContextKind::AgentStatus(_))
                    )
            )),
        "the provisional status message never became canonical"
    );
    assert!(
        observer
            .statuses
            .lock()
            .expect("observer status lock")
            .is_empty(),
        "structured status is published only after model-turn start commit"
    );
    assert!(
        result.messages().iter().all(|block| !matches!(
            block,
            MessageBlock::User(user)
                if matches!(
                    &user.kind,
                    InboundKind::Context(ContextKind::AgentStatus(_))
                )
        )),
        "canonical history contains no Agent Status message"
    );
}

/// Issue #12 (M9b): the exact arbitration race, start winner. The
/// execution is parked **inside** the arbitration — the start gate is
/// held, the cancellation check has passed, and the durable commit is
/// pending. A concurrent canceller's `cancel()` provably cannot complete
/// while the gate is held; once the commit is released the start fact
/// is durable, the provider request starts, and the late cancellation
/// settles that started request as post-start cancellation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_commit_wins_the_arbitration_race_against_cancellation() {
    let adapter = Arc::new(ParkedUntilCancelledAdapter::default());
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (pause, _, pre_commit) = StartBoundaryPause::install(false, true);
    let mut pre_commit = pre_commit.expect("pre-commit phase installed");
    let tool_runtime = tool_runtime("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let mut execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.install_start_boundary_pause(pause);
    let canceller = cancellation.clone();
    let controller = tokio::spawn(async move {
        pre_commit.await_park(1).await;
        // Release the parked commit, then cancel: the cancel can only
        // complete after the gate is dropped at the end of the durable
        // start commit, so cancellation provably linearizes after the
        // start fact. The parked provider stream then makes the attempt
        // outcome deterministic regardless of the post-commit schedule.
        pre_commit.release();
        canceller.cancel();
    });
    let result = execution.run().await;
    controller.await.expect("canceller controller");

    assert_eq!(
        adapter.request_count(),
        1,
        "the provider request started exactly once"
    );
    let events = event_history(tool_runtime.durable_store().as_ref());
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        1,
        "exactly one start fact: the durable commit won the race"
    );
    assert_eq!(
        request_snapshot_history(tool_runtime.durable_store().as_ref()).len(),
        1,
        "exactly one started Request Snapshot"
    );
    assert!(
        matches!(
            result.outcome,
            crate::events::types::AttemptOutcome::Cancelled {
                reason: CancellationReason::UserRequested
            }
        ),
        "the post-start cancellation settles the started request"
    );
    assert!(
        matches!(events.last(), Some(RuntimeEvent::AttemptCancelled { .. }))
            && events
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
                .count()
                == 1,
        "the cancellation terminal is unique and last: {events:?}"
    );
    assert!(
        !result
            .messages()
            .iter()
            .any(|message| matches!(message, MessageBlock::Assistant(_))),
        "the cancelled in-flight request commits no assistant message"
    );
}

/// A contributor returning one static User-context proposal.
struct StaticContributor;

impl crate::context::ContextContributor for StaticContributor {
    fn contribute<'a>(
        &'a self,
        _input: &'a crate::context::ContributorInputSnapshot,
    ) -> BoxFuture<
        'a,
        Result<Vec<crate::context::ContextProposal>, crate::context::ContextAssemblyError>,
    > {
        Box::pin(async {
            Ok(vec![crate::context::ContextProposal::UserMessage(
                crate::context::UserMessageProposal {
                    content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                        text: "staged context".to_owned(),
                    })],
                },
            )])
        })
    }
}

/// Issue #12 (M9b): a durable failure **inside** the fused start
/// transaction — after the request-scoped context append — rolls the
/// whole transaction back: no context, no snapshot, no
/// `ModelRequestStarted`, no provider invocation. The attempt settles
/// with the honest durable-store failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn failed_start_commit_after_context_append_rolls_back_everything() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }]]));
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .expect("store"),
    );
    store.arm_request_start_fault_script([
        crate::durable::sqlite::RequestStartFaultOperation::AfterContextAppend,
    ]);
    let mut assembly = crate::context::ContextAssembly::new();
    assembly
        .register_extension(
            "static.test",
            Some("package-v1".to_owned()),
            Arc::new(StaticContributor),
        )
        .expect("register static contributor");
    let request = request(&adapter);
    let runtime = ContextRuntime::for_attempt_with_assembly(
        crate::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(crate::context::DefaultTokenEstimator),
        Some(crate::context::AgentStatusEngine::default()),
        assembly,
        &request.model,
        crate::model::ModelTimeoutPolicy::default(),
        crate::scripted_suites::support::default_monotonic_clock(),
    )
    .expect("valid context runtime");
    let tool_runtime = tool_runtime_with_store(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        Some(store.clone()),
    );
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;

    assert!(
        matches!(
            &result.outcome,
            crate::events::types::AttemptOutcome::Failed { error }
                if format!("{error:?}").contains("request start could not be committed durably")
        ),
        "the attempt settles with the honest durable-store failure: {:?}",
        result.outcome
    );
    assert_eq!(
        adapter.request_count(),
        0,
        "the provider request never started"
    );
    assert!(
        store
            .read_request_snapshots(None, 32)
            .expect("snapshots")
            .snapshots
            .is_empty(),
        "no started Request Snapshot exists"
    );
    let journal = store.read_events(None, 128).expect("journal").events;
    assert!(
        !journal.iter().any(|envelope| matches!(
            envelope.event,
            crate::events::types::RuntimeEvent::ModelRequestStarted { .. }
        )),
        "no request-start fact exists"
    );
    assert!(
        store.load_canonical().expect("canonical").is_empty(),
        "the staged context rolled back with the start transaction"
    );
    assert!(
        store
            .read_request_snapshots(None, 32)
            .expect("snapshots")
            .snapshots
            .is_empty(),
        "no snapshot exists"
    );
    assert!(
        matches!(
            journal.last().map(|envelope| &envelope.event),
            Some(crate::events::types::RuntimeEvent::AttemptFailed { .. })
        ),
        "the attempt settles with the durable failure terminal"
    );
}

/// A pre-step policy that counts its evaluations, then always enters.
struct CountingEnterPolicy {
    evaluations: Arc<std::sync::atomic::AtomicUsize>,
}

impl crate::agent::PreStepPolicy for CountingEnterPolicy {
    fn evaluate<'a>(
        &'a self,
        _batch: &'a crate::agent::PreStepBatch<'a>,
    ) -> BoxFuture<'a, Result<crate::agent::PreStepDecision, crate::agent::LifecycleError>> {
        self.evaluations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Ok(crate::agent::PreStepDecision::Enter) })
    }
}

/// Issue #12 (M9b): an inert lifecycle and an attached-but-empty
/// lifecycle (a counting `AlwaysEnter` policy plus the native
/// no-deferred-context observer) reach the same start boundary and
/// produce the same never-started outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inert_and_attached_lifecycles_share_the_start_boundary() {
    for attached in [false, true] {
        let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }]]));
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, pre_start, _) = StartBoundaryPause::install(true, false);
        let mut pre_start = pre_start.expect("pre-start phase installed");
        // Each iteration gets its own in-memory durable authority; the
        // default temp-dir store would be shared across iterations.
        let tool_runtime = tool_runtime_with_store(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
            Some(Arc::new(
                crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
                ))
                .expect("store"),
            )),
        );
        let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
            .await
            .into_lease();
        let evaluations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let lifecycle = if attached {
            crate::agent::AttemptLifecycle::inert()
                .with_pre_step_policy(Arc::new(CountingEnterPolicy {
                    evaluations: Arc::clone(&evaluations),
                }))
                .with_native_tool_result_observer(Arc::new(crate::agent::NoDeferredContext))
                .expect("native observer binding")
        } else {
            crate::agent::AttemptLifecycle::inert()
        };
        let mut execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime(&adapter),
            &tool_runtime,
            lifecycle,
        )
        .expect("conversation identity matches the tool runtime");
        execution.install_start_boundary_pause(pause);
        let controller_cancellation = cancellation.clone();
        let controller = tokio::spawn(async move {
            pre_start.await_park(1).await;
            controller_cancellation.cancel();
            pre_start.release();
        });
        let result = execution.run().await;
        controller.await.expect("controller task");

        assert!(
            matches!(
                result.outcome,
                crate::events::types::AttemptOutcome::Cancelled {
                    reason: CancellationReason::UserRequested
                }
            ),
            "attached={attached}: cancellation before start settles cancelled"
        );
        assert_eq!(
            adapter.request_count(),
            0,
            "attached={attached}: the provider request never started"
        );
        assert!(
            request_snapshot_history(tool_runtime.durable_store().as_ref()).is_empty(),
            "attached={attached}: no started Request Snapshot"
        );
        assert!(
            result.messages().is_empty(),
            "attached={attached}: no request-scoped context committed"
        );
        if attached {
            assert!(
                evaluations.load(std::sync::atomic::Ordering::SeqCst) >= 1,
                "the attached policy was genuinely evaluated"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue383_noop_and_absent_extensions_share_the_cancellation_boundary() {
    for composed in [false, true] {
        let adapter = Arc::new(ScriptedAdapter::new(vec![]));
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
            ))
            .unwrap(),
        );
        let probe = composed.then(|| {
            crate::extensions::probe::ProbeConfig::new(
                crate::runtime::identity::NativeContextContributor::TestAlpha,
                crate::extensions::probe::Behavior::Empty,
            )
        });
        let (_dir, tool_runtime) = native_probe_runtime(probe, store.clone());
        let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
            .await
            .into_lease();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let (pause, parked, _) = StartBoundaryPause::install(true, false);
        let mut parked = parked.unwrap();
        let mut execution = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .unwrap();
        execution.install_start_boundary_pause(pause);
        let canceller = cancellation.clone();
        let controller = tokio::spawn(async move {
            parked.await_park(1).await;
            canceller.cancel();
            parked.release();
        });
        let result = execution.run().await;
        controller.await.unwrap();
        assert!(matches!(result.outcome, AttemptOutcome::Cancelled { .. }));
        assert_eq!(adapter.request_count(), 0);
        assert!(request_snapshot_history(store.as_ref()).is_empty());
        let events = event_history(store.as_ref());
        assert!(!events.iter().any(|e| matches!(
            e,
            RuntimeEvent::ContextContributionEmitted { .. }
                | RuntimeEvent::ModelRequestStarted { .. }
        )));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, RuntimeEvent::AttemptCancelled { .. }))
                .count(),
            1
        );
        assert!(matches!(
            events.last(),
            Some(RuntimeEvent::AttemptCancelled { .. })
        ));
    }
}

fn native_probe_runtime(
    probe: impl IntoIterator<Item = crate::extensions::probe::ProbeConfig>,
    store: Arc<crate::durable::SqliteConversationStore>,
) -> (
    tempfile::TempDir,
    crate::tools::runtime::ConversationToolRuntime,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("workspace")).unwrap();
    let mut composition = crate::extensions::NativeAgentExtensions::none();
    composition.test_contributors.extend(probe);
    let runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
        store.conversation_id().clone(),
        crate::tools::runtime::ConversationRuntimeConfig {
            durable_binding: Some(crate::durable::ConversationStoreBinding::new(store)),
            ..crate::tools::runtime::ConversationRuntimeConfig::new(
                dir.path().join("workspace"),
                dir.path().join("artifacts"),
            )
            .with_extensions(composition)
        },
    )
    .unwrap();
    (dir, runtime)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue383_native_preparation_cancellation_commits_nothing() {
    let adapter = Arc::new(ScriptedAdapter::new(vec![vec![ModelEvent::Completed {
        finish_reason: ModelFinishReason::Stop,
        usage: None,
    }]]));
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .unwrap(),
    );
    let (entered, mut arrived) = watch::channel(false);
    let (release, wait) = watch::channel(false);
    let mut probe = crate::extensions::probe::ProbeConfig::new(
        crate::runtime::identity::NativeContextContributor::TestAlpha,
        crate::extensions::probe::Behavior::Normal,
    );
    probe.gate = Some((entered, wait));
    let count = probe.captures.clone();
    let (_dir, tool_runtime) = native_probe_runtime([probe], store.clone());
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let canceller = cancellation.clone();
    let controller = tokio::spawn(async move {
        arrived.wait_for(|arrived| *arrived).await.unwrap();
        canceller.cancel();
        assert!(canceller.is_cancelled());
        release.send_replace(true);
    });
    let result = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap()
    .run()
    .await;
    controller.await.unwrap();
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(adapter.request_count(), 0);
    assert!(request_snapshot_history(store.as_ref()).is_empty());
    assert!(
        store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::TestAlpha
                ),
                "active"
            )
            .unwrap()
            .is_none()
    );
    assert!(matches!(result.outcome, AttemptOutcome::Cancelled { .. }));
    let events = event_history(store.as_ref());
    assert!(!events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ModelRequestStarted { .. } | RuntimeEvent::ContextContributionEmitted { .. }
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
            .count(),
        1
    );
    assert!(matches!(
        events.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue383_native_start_wins_and_receipts_remain_truthful() {
    let adapter = Arc::new(ParkedUntilCancelledAdapter::default());
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .unwrap(),
    );
    let probe = crate::extensions::probe::ProbeConfig::new(
        crate::runtime::identity::NativeContextContributor::TestAlpha,
        crate::extensions::probe::Behavior::Normal,
    );
    let (_dir, tool_runtime) = native_probe_runtime([probe], store.clone());
    let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let (pause, _, parked) = StartBoundaryPause::install(false, true);
    let mut parked = parked.unwrap();
    let mut execution = AgentExecution::new(
        request(&adapter),
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&adapter),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap();
    execution.install_start_boundary_pause(pause);
    let canceller = cancellation.clone();
    let controller = tokio::spawn(async move {
        parked.await_park(1).await;
        // Startup owns the cancellation gate. cancel() cannot complete until
        // the durable transaction returns and releases that same gate.
        parked.release();
        canceller.cancel();
    });
    let result = execution.run().await;
    controller.await.unwrap();
    assert!(matches!(result.outcome, AttemptOutcome::Cancelled { .. }));
    assert_eq!(adapter.request_count(), 1);
    let snapshots = request_snapshot_history(store.as_ref());
    assert_eq!(snapshots.len(), 1);
    let receipt = store
        .latest_contribution_emission(
            &crate::runtime::identity::ContextContributorIdentity::Native(
                crate::runtime::identity::NativeContextContributor::TestAlpha,
            ),
            "active",
        )
        .unwrap()
        .unwrap();
    assert!(
        snapshots[0]
            .contributions
            .iter()
            .any(|entry| entry.message_id == receipt.canonical_message_id)
    );
    let events = event_history(store.as_ref());
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ContextContributionEmitted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::AttemptCancelled { .. }))
            .count(),
        1
    );
    assert!(matches!(
        events.last(),
        Some(RuntimeEvent::AttemptCancelled { .. })
    ));
}

#[tokio::test]
async fn issue383_native_start_transaction_failure_rolls_back_receipts() {
    for fault in [
        crate::durable::sqlite::RequestStartFaultOperation::AfterContextAppend,
        crate::durable::sqlite::RequestStartFaultOperation::AfterContributionEventInsert,
        crate::durable::sqlite::RequestStartFaultOperation::AfterContributionHeadUpsert,
    ] {
        let adapter = Arc::new(ScriptedAdapter::new(vec![]));
        let store = Arc::new(
            crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
            ))
            .unwrap(),
        );
        store.arm_request_start_fault_script([fault]);
        let probe = crate::extensions::probe::ProbeConfig::new(
            crate::runtime::identity::NativeContextContributor::TestAlpha,
            crate::extensions::probe::Behavior::Normal,
        );
        let (_dir, tool_runtime) = native_probe_runtime([probe], store.clone());
        let lease = capability_lease(ToolRegistry::new(), &tool_runtime)
            .await
            .into_lease();
        let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
        let result = AgentExecution::new(
            request(&adapter),
            lease,
            &cancellation,
            crate::scripted_suites::support::default_execution_policy(),
            runtime(&adapter),
            &tool_runtime,
            crate::agent::AttemptLifecycle::inert(),
        )
        .unwrap()
        .run()
        .await;
        assert!(matches!(result.outcome, AttemptOutcome::Failed { .. }));
        assert_eq!(adapter.request_count(), 0);
        assert!(request_snapshot_history(store.as_ref()).is_empty());
        assert!(
            store
                .latest_contribution_emission(
                    &crate::runtime::identity::ContextContributorIdentity::Native(
                        crate::runtime::identity::NativeContextContributor::TestAlpha
                    ),
                    "active"
                )
                .unwrap()
                .is_none()
        );
        assert!(!store.load_canonical().unwrap().iter().any(|message| matches!(message, MessageBlock::User(user) if matches!(user.kind, InboundKind::Context(_)))));
        let events = event_history(store.as_ref());
        assert!(!events.iter().any(|event| matches!(
            event,
            RuntimeEvent::ModelRequestStarted { .. }
                | RuntimeEvent::ContextContributionEmitted { .. }
        )));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::AttemptFailed { .. }))
                .count(),
            1
        );
        assert!(matches!(
            events.last(),
            Some(RuntimeEvent::AttemptFailed { .. })
        ));
    }
}
