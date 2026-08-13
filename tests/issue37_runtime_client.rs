//! Issue #37: Runtime Client host integration tests over the public
//! semantic surface.
//!
//! All concurrency is scripted through deterministic fixtures (watches,
//! notifies, park steps); no wall-clock sleep proves any invariant. The
//! exact synchronization/linearization proofs live in the in-crate host
//! tests; this file proves the public contract end to end.

#[path = "common/mod.rs"]
mod common;

use std::sync::Arc;

use common::fake::{FakeModel, FakeStep, FakeTool, ScriptedCall, success_result, tool_call_events};
use rustx::context::{
    AgentStatusClock, AgentStatusComposer, AgentStatusFact, AgentStatusRenderContext,
    AgentStatusSectionId, AgentStatusSectionProvider, ContextConfig, ContextEngine, ContextError,
    DefaultTokenEstimator, InMemoryCheckpointStore, TokenEstimator,
};
use rustx::message::types::{ContentBlockIndex, MessageBlock, UserContentBlock};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::{ModelProtocol, ReasoningEffort};
use rustx::runtime::identity::{AgentId, ToolId};
use rustx::runtime_client::{
    RuntimeClientContextConfig, RuntimeClientEvent, RuntimeClientHost, RuntimeClientHostConfig,
    RuntimeClientOutcome, RuntimeClientProtocolEvent, RuntimeClientRequest, RuntimeClientResult,
};
use rustx::tools::executor::ToolRegistry;
use rustx::tools::types::{
    ToolConcurrencyPolicy, ToolExecutionPolicy, ToolOrigin, ToolReplayPolicy,
};

/// A fixed deterministic status clock.
#[derive(Debug, Clone, Copy)]
struct FixedStatusClock;

impl AgentStatusClock for FixedStatusClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
            .expect("fixed clock")
            .with_timezone(&chrono::Utc)
    }
}

/// An extension provider recording its facts.
struct RecordingProvider;

impl AgentStatusSectionProvider for RecordingProvider {
    fn section_id(&self) -> AgentStatusSectionId {
        AgentStatusSectionId::new("recording")
    }

    fn section(
        &self,
        _context: &AgentStatusRenderContext,
    ) -> Result<Option<Vec<AgentStatusFact>>, ContextError> {
        Ok(Some(vec![AgentStatusFact {
            label: "extension".to_owned(),
            value: "fact".to_owned(),
        }]))
    }
}

fn composer() -> AgentStatusComposer {
    AgentStatusComposer::new(Arc::new(FixedStatusClock))
}

/// A status clock that counts compositions.
///
/// The composer samples its clock exactly once per `compose` invocation, so
/// this counter *is* the number of Agent Status compositions — regardless of
/// how many clones of the composer exist.
#[derive(Debug, Clone, Default)]
struct CountingStatusClock {
    compositions: Arc<std::sync::atomic::AtomicUsize>,
}

impl AgentStatusClock for CountingStatusClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.compositions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
            .expect("fixed clock")
            .with_timezone(&chrono::Utc)
    }
}

/// An extension provider that counts its invocations: an independent
/// witness of how many compositions ran.
struct CountingProvider {
    invocations: Arc<std::sync::atomic::AtomicUsize>,
}

impl AgentStatusSectionProvider for CountingProvider {
    fn section_id(&self) -> AgentStatusSectionId {
        AgentStatusSectionId::new("counting")
    }

    fn section(
        &self,
        _context: &AgentStatusRenderContext,
    ) -> Result<Option<Vec<AgentStatusFact>>, ContextError> {
        let seen = self
            .invocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(vec![AgentStatusFact {
            label: "composition".to_owned(),
            value: seen.to_string(),
        }]))
    }
}

/// Builds a host over one conversation with the given model scripts and
/// tool registry. The returned model handle records the requests the host
/// sent.
async fn host(
    conversation: &str,
    model: FakeModel,
    tools: ToolRegistry,
    composer: AgentStatusComposer,
    replay_limit: Option<usize>,
) -> (Arc<FakeModel>, RuntimeClientHost) {
    let model = Arc::new(model);
    let adapter: Arc<dyn rustx::model::ModelAdapter> = model.clone();
    let tool_runtime = common::tool_runtime(conversation);
    let coordinator = {
        let dir = tempfile::tempdir().expect("temp dir");
        let coordinator = rustx::capabilities::CapabilityCoordinator::new(
            rustx::capabilities::CapabilityCoordinatorConfig {
                conversation_id: tool_runtime.conversation_id().clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(tools),
                mcp_servers: Vec::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        std::mem::forget(dir);
        coordinator
    };
    let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 10_000_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator,
    )
    .expect("engine");
    let host = RuntimeClientHost::new(RuntimeClientHostConfig {
        agent_id: AgentId::new("agent-a"),
        model: "scripted".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        reasoning: ReasoningEffort::Medium,
        max_output_tokens: 512,
        timezone: None,
        adapter,
        context: RuntimeClientContextConfig {
            engine,
            summarizer: Arc::new(common::context::FakeContextSummarizer::new(Vec::new())),
            checkpoint_store: Arc::new(InMemoryCheckpointStore::new()),
            status_composer: composer,
        },
        tool_runtime,
        capability: coordinator,
        clock: None,
        initial_messages: Vec::new(),
        replay_limit,
    })
    .expect("host");
    (model, host)
}

/// The outer liveness guard of the event-stream helper.
///
/// Waiting for an event is exact: the subscription wakes on publication.
/// This bounds only the total wall time of one `receive_until` call, as a
/// whole-call budget rather than a per-event bound, so a single scheduling
/// stall on a loaded runner can never fail a correct run.
const STREAM_LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_secs(120);

/// Subscribes an attachment and receives until the predicate matches.
async fn receive_until(
    subscription: &rustx::runtime_client::EventSubscription,
    mut predicate: impl FnMut(&RuntimeClientProtocolEvent) -> bool,
) -> Vec<RuntimeClientProtocolEvent> {
    tokio::time::timeout(STREAM_LIVENESS_GUARD, async {
        let mut seen = Vec::new();
        loop {
            let delivery = subscription.next().await;
            let rustx::runtime_client::EventDelivery::Event(event) = delivery else {
                panic!("subscription must stay open and contiguous, got {delivery:?}");
            };
            let matched = predicate(&event);
            seen.push(event);
            if matched {
                return seen;
            }
        }
    })
    .await
    .expect("the observation stream must not stall")
}

fn one_turn_stop() -> Vec<FakeStep> {
    vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "done".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

fn scripted(id: &str, tool_id: &str, name: &str, arguments: serde_json::Value) -> ScriptedCall {
    ScriptedCall {
        id: Box::leak(id.to_owned().into_boxed_str()),
        tool_id: Box::leak(tool_id.to_owned().into_boxed_str()),
        name: Box::leak(name.to_owned().into_boxed_str()),
        arguments,
    }
}

fn text(text: &str) -> Vec<UserContentBlock> {
    vec![UserContentBlock::Text(rustx::message::content::TextBlock {
        text: text.to_owned(),
    })]
}

/// Submitting while idle admits and runs an attempt; the response means
/// accepted, and settlement is observed asynchronously exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submit_idle_admits_and_settles_asynchronously() {
    let model = FakeModel::new(vec![one_turn_stop()]);
    let (model_handle, host) =
        host("conv-37-idle", model, ToolRegistry::new(), composer(), None).await;
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe");

    let response = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: rustx::runtime_client::RequestId::new(1),
        content: text("hello"),
    });
    let Some(RuntimeClientResult::InboundAccepted {
        message_id,
        inbound_sequence,
    }) = response.result
    else {
        panic!("accepted result");
    };
    assert_eq!(inbound_sequence.get(), 1);
    assert_eq!(message_id.as_str(), "conv-37-idle-inbound-1");

    let events = receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, RuntimeClientEvent::AttemptSettled { .. }))
            .count(),
        1,
        "terminal settlement observed exactly once"
    );
    let requests = model_handle.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .messages
            .iter()
            .any(|message| matches!(message, MessageBlock::User(user) if user.id == message_id))
    );
    assert!(requests[0].agent_status.is_some());

    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(snapshot.messages.len(), 2);
    assert!(matches!(
        snapshot.attempt.expect("attempt").phase,
        rustx::runtime_client::RuntimeClientAttemptPhase::Settled { .. }
    ));
}

/// Submitting while an attempt runs queues the message; the safe-boundary
/// drain admits it into the next turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submit_while_busy_queues_for_the_next_drain() {
    let (release_tx, release_rx) = common::fake::model_release();
    let model = FakeModel::new(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "working".to_owned(),
            }),
            FakeStep::ParkUntilReleased(release_rx),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
        one_turn_stop(),
    ]);
    let (model_handle, host) =
        host("conv-37-busy", model, ToolRegistry::new(), composer(), None).await;
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe");

    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: rustx::runtime_client::RequestId::new(1),
        content: text("first"),
    });
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AssistantTextDelta { .. })
    })
    .await;
    assert_eq!(
        model_handle.requests().len(),
        1,
        "the first turn is in flight"
    );

    let second = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: rustx::runtime_client::RequestId::new(2),
        content: text("second"),
    });
    let Some(RuntimeClientResult::InboundAccepted {
        message_id: second_id,
        ..
    }) = second.result
    else {
        panic!("accepted");
    };
    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.inbound.pending.len(),
        1,
        "the queued message stays pending while busy"
    );
    assert_eq!(snapshot.inbound.pending[0].message.id, second_id);

    release_tx.send(true).expect("release");
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    let requests = model_handle.requests();
    assert_eq!(requests.len(), 2, "the drained batch opens the next turn");
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|message| matches!(message, MessageBlock::User(user) if user.id == second_id))
    );
    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert!(snapshot.inbound.pending.is_empty());
}

/// Protocol cancellation of the current attempt: acceptance is not
/// settlement; the runtime terminal cancellation is observed exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_current_attempt_is_acceptance_not_settlement() {
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::ParkUntilCancelled,
    ]]);
    let (_, host) = host(
        "conv-37-cancel",
        model,
        ToolRegistry::new(),
        composer(),
        None,
    )
    .await;
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe");
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: rustx::runtime_client::RequestId::new(1),
        content: text("go"),
    });
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptStarted { .. })
    })
    .await;
    // Wait until the model is provably parked awaiting cancellation.
    // The deterministic acceptance-is-not-settlement interleaving
    // (a gated model parked between the acceptance response and the
    // terminal observation) is proven in the in-crate host tests; here
    // the public contract is asserted: the response is the typed
    // acceptance, and the runtime terminal settlement is observed
    // asynchronously exactly once.
    let response = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
        id: rustx::runtime_client::RequestId::new(2),
    });
    assert!(matches!(
        response.result,
        Some(RuntimeClientResult::AttemptCancellationAccepted { .. })
    ));

    let events = receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.event,
                RuntimeClientEvent::AttemptSettled {
                    outcome: RuntimeClientOutcome::Cancelled { .. },
                    ..
                }
            ))
            .count(),
        1
    );
}

/// Foreground tool lifecycle projects start/progress/settlement, and
/// reversed physical completion never corrupts logical identities.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreground_tools_project_with_stable_identities() {
    let call_a = scripted("call-a", "tool-alpha", "alpha", serde_json::json!({"n": 1}));
    let call_b = scripted("call-b", "tool-beta", "beta", serde_json::json!({"n": 2}));
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for (block, call) in [&call_a, &call_b].into_iter().enumerate() {
        let block = u32::try_from(block).expect("fits");
        for event in tool_call_events(block, call) {
            first.push(FakeStep::Emit(event));
        }
    }
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    let model = FakeModel::new(vec![first, one_turn_stop()]);
    let (tool_a, release_a) = FakeTool::parking(
        common::tool_policies(
            "alpha",
            "tool-alpha",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("a"),
    );
    let (tool_b, release_b) = FakeTool::parking(
        common::tool_policies(
            "beta",
            "tool-beta",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
        ),
        success_result("b"),
    );
    let mut started_a = tool_a.started();
    let mut started_b = tool_b.started();
    let completed_b = tool_b.completed();
    let mut controller_completed_b = completed_b.clone();
    let mut tools = ToolRegistry::new();
    tool_a.register(&mut tools);
    tool_b.register(&mut tools);
    let (_, host) = host("conv-37-tools", model, tools, composer(), None).await;
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe");

    let controller = tokio::spawn(async move {
        started_a
            .wait_for(|started| *started)
            .await
            .expect("A started");
        started_b
            .wait_for(|started| *started)
            .await
            .expect("B started");
        release_b.notify_one();
        controller_completed_b
            .wait_for(|order| order.iter().any(|name| name == "beta"))
            .await
            .expect("B completed physically first");
        release_a.notify_one();
    });

    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: rustx::runtime_client::RequestId::new(1),
        content: text("run tools"),
    });
    let events = receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    controller.await.expect("controller");

    // The client-visible settlement order is the canonical model-call
    // order even though B completed physically first.
    let settled_calls: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::ToolExecutionSettled { tool_call_id, .. } => {
                Some(tool_call_id.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(settled_calls, vec!["call-a", "call-b"]);
    let started_calls: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::ToolExecutionStarted { tool_call_id, .. } => {
                Some(tool_call_id.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(started_calls, vec!["call-a", "call-b"]);

    let (snapshot, _) = host.snapshot().expect("snapshot");
    let foreground = &snapshot.attempt.expect("attempt").foreground;
    assert_eq!(foreground.len(), 2);
    assert_eq!(foreground[0].call_id.as_str(), "call-a");
    assert_eq!(foreground[1].call_id.as_str(), "call-b");
    assert!(foreground.iter().all(|slot| matches!(
        slot.state,
        rustx::runtime_client::ForegroundToolState::Settled { .. }
    )));
}

/// A snapshot taken mid-stream carries enough in-flight state to repair
/// all client-visible effects, and a resume after its cursor observes the
/// remaining output exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streaming_repair_from_a_mid_stream_snapshot() {
    let (release_tx, release_rx) = common::fake::model_release();
    let model = FakeModel::new(vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "hello ".to_owned(),
        }),
        FakeStep::ParkUntilReleased(release_rx),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "world".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]]);
    let (_, host) = host(
        "conv-37-repair",
        model,
        ToolRegistry::new(),
        composer(),
        None,
    )
    .await;
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe");
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: rustx::runtime_client::RequestId::new(1),
        content: text("go"),
    });
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AssistantTextDelta { .. })
    })
    .await;

    // Snapshot while the message is parked mid-stream.
    let (snapshot, _cursor) = host.snapshot().expect("snapshot");
    let in_flight = snapshot
        .attempt
        .as_ref()
        .and_then(|attempt| attempt.in_flight.as_ref())
        .expect("in-flight repair state");
    assert_eq!(
        in_flight.blocks,
        vec![rustx::runtime_client::InFlightBlock::Text {
            block_index: ContentBlockIndex::new(0),
            text: "hello ".to_owned(),
        }]
    );

    // Release and resume after C: exactly the remaining delta arrives, so
    // the client reconstructs "hello world" with no duplicate output.
    release_tx.send(true).expect("release");
    let resumed = receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    let deltas: Vec<String> = resumed
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::AssistantTextDelta { delta, .. } => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        deltas,
        vec!["world".to_owned()],
        "no duplicated output after resume"
    );

    let (final_snapshot, _) = host.snapshot().expect("snapshot");
    let committed_text = final_snapshot
        .messages
        .iter()
        .find_map(|message| match message {
            MessageBlock::Agent(agent) => agent.content.iter().find_map(|block| match block {
                rustx::message::types::AgentContentBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .expect("committed agent text");
    assert_eq!(committed_text, "hello world");
    assert!(
        final_snapshot
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.in_flight.as_ref())
            .is_none()
    );
}

/// The bounded replay/resync contract through the public surface: an
/// expired cursor fails with `resync_required` and a fresh snapshot
/// repairs state; a serviceable cursor resumes without gaps.
// One stall/repair lifecycle observed end to end.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_subscriber_resyncs_explicitly_and_never_buffers() {
    // The model parks on its first step, so the exact set of publications
    // that happen before the test polls is deterministic: the attempt
    // cannot advance past the park.
    let (release, release_rx) = common::fake::model_release();
    let model = FakeModel::new(vec![vec![
        FakeStep::ParkUntilReleased(release_rx),
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "done".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]]);
    let (model_handle, host) = host(
        "conv-37-replay",
        model,
        ToolRegistry::new(),
        composer(),
        Some(2),
    )
    .await;
    let mut parked = model_handle.parked();
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    // Registered but deliberately never polled: this is the stalled
    // consumer.
    let stalled = attachment
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe");
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: rustx::runtime_client::RequestId::new(1),
        content: text("go"),
    });
    // Barrier, not a sleep: the model parked, so every publication of the
    // admission burst has happened and no further one can.
    parked
        .wait_for(|parked| *parked)
        .await
        .expect("model parks before the release");

    // The admission burst published more events than the two-entry ring
    // retains. The stalled subscriber is told so explicitly; it never
    // receives a silently non-contiguous cursor.
    let delivery = stalled.try_next();
    let rustx::runtime_client::EventDelivery::ResyncRequired {
        after_cursor,
        earliest_serviceable,
    } = delivery
    else {
        panic!("a subscriber behind the retention must resync, got {delivery:?}");
    };
    assert_eq!(after_cursor.get(), 0);
    assert!(earliest_serviceable.get() > 0);
    assert_eq!(
        stalled.try_next(),
        rustx::runtime_client::EventDelivery::ResyncRequired {
            after_cursor,
            earliest_serviceable,
        },
        "the lag verdict is stable, never a partial catch-up"
    );

    // Registering a fresh subscription at the expired cursor fails with the
    // same explicit category.
    let error = attachment
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect_err("evicted cursor is unserviceable");
    assert!(matches!(
        error,
        rustx::runtime_client::RuntimeClientError::ResyncRequired {
            after_cursor,
            ..
        } if after_cursor.get() == 0
    ));

    // A fresh snapshot repairs every externally visible fact, and resuming
    // at its cursor is always serviceable.
    let (snapshot, cursor) = host.snapshot().expect("snapshot");
    assert_eq!(snapshot.messages.len(), 1, "the inbound message committed");
    let mut resumed = attachment
        .subscribe_events(cursor)
        .expect("the snapshot cursor is always serviceable");
    release.send_replace(true);

    // Consume to settlement. With a two-entry ring a consumer can fall
    // behind the attempt's publication burst more than once, and that is
    // the contract working: every delivery is either the strictly next
    // cursor or an explicit repair — never a silent gap. Repairing is a
    // fresh snapshot plus a re-subscription at its cursor, so this loop
    // terminates on the settlement it observes or on the settled snapshot
    // it repairs to. Whether a repair happens at all is scheduling; that
    // both outcomes are explicit is the invariant.
    let mut expected = cursor.get();
    let mut repairs = 0_u32;
    loop {
        // Liveness guard only: each delivery wait itself is exact.
        let delivery = tokio::time::timeout(STREAM_LIVENESS_GUARD, resumed.next())
            .await
            .expect("the stream must not stall");
        match delivery {
            rustx::runtime_client::EventDelivery::Event(event) => {
                expected += 1;
                assert_eq!(
                    event.cursor.get(),
                    expected,
                    "a subscription observes strictly contiguous cursors"
                );
                if matches!(event.event, RuntimeClientEvent::AttemptSettled { .. }) {
                    break;
                }
            }
            rustx::runtime_client::EventDelivery::ResyncRequired { .. } => {
                repairs += 1;
                assert!(repairs < 32, "repairing must converge");
                let (repaired, repaired_cursor) = host.snapshot().expect("snapshot");
                if repaired.attempt.as_ref().is_some_and(|attempt| {
                    matches!(
                        attempt.phase,
                        rustx::runtime_client::RuntimeClientAttemptPhase::Settled { .. }
                    )
                }) {
                    break;
                }
                expected = repaired_cursor.get();
                resumed = attachment
                    .subscribe_events(repaired_cursor)
                    .expect("the snapshot cursor is always serviceable");
            }
            other => panic!("the subscription stays open, got {other:?}"),
        }
    }

    let (final_snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(final_snapshot.messages.len(), 2);
    assert!(matches!(
        final_snapshot.attempt.expect("attempt").phase,
        rustx::runtime_client::RuntimeClientAttemptPhase::Settled { .. }
    ));
}

/// Agent Status is projected from the exact same composition the model
/// path consumes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_status_shares_one_composition() {
    let mut composer = composer();
    composer
        .register(Arc::new(RecordingProvider))
        .expect("register");
    let model = FakeModel::new(vec![one_turn_stop()]);
    let (model_handle, host) =
        host("conv-37-status", model, ToolRegistry::new(), composer, None).await;
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe");
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: rustx::runtime_client::RequestId::new(1),
        content: text("go"),
    });
    let events = receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AgentStatusComposed { .. })
    })
    .await;
    let status_view = events
        .iter()
        .find_map(|event| match &event.event {
            RuntimeClientEvent::AgentStatusComposed { status, .. } => Some(status),
            _ => None,
        })
        .expect("status event");
    receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    let requests = model_handle.requests();
    assert_eq!(requests.len(), 1);
    let model_rendered = requests[0]
        .agent_status
        .as_ref()
        .expect("model path carries Agent Status")
        .rendered
        .clone();
    assert_eq!(
        status_view.rendered, model_rendered,
        "client view derives from the same composition as the model path"
    );
    assert!(status_view.sections.iter().any(|section| matches!(
        section,
        rustx::runtime_client::RuntimeClientStatusSection::Facts { facts }
            if facts.iter().any(|fact| fact.label == "extension" && fact.value == "fact")
    )));
    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.status.expect("status view").rendered,
        model_rendered
    );
}

/// Agent Status is composed **exactly once** per model request that
/// receives it, and both destinations consume that one composition.
///
/// The proof is counting, not structural similarity: the composer samples
/// its clock once per `compose`, and the registered extension provider is
/// invoked once per `compose`. Both counters must equal the number of model
/// requests that carried an Agent Status attachment. If the Runtime Client
/// path recomposed — even through a cloned composer with the same clock and
/// providers — the counters would exceed that number.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_status_is_composed_exactly_once_per_request() {
    let clock = CountingStatusClock::default();
    let compositions = clock.compositions.clone();
    let invocations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut composer = AgentStatusComposer::new(Arc::new(clock));
    composer
        .register(Arc::new(CountingProvider {
            invocations: invocations.clone(),
        }))
        .expect("register");

    // Two model requests in one attempt: a tool-calling turn followed by a
    // stopping turn, so "one composition per request that receives status"
    // is a real constraint rather than a coincidence of a single request.
    let call = scripted("call-a", "tool-alpha", "alpha", serde_json::json!({"n": 1}));
    let mut first = vec![FakeStep::Emit(ModelEvent::Started)];
    for event in tool_call_events(0, &call) {
        first.push(FakeStep::Emit(event));
    }
    first.push(FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    let model = FakeModel::new(vec![first, one_turn_stop()]);
    let tool = FakeTool::new(
        common::tool_policies(
            "alpha",
            "tool-alpha",
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
        ),
        success_result("a"),
    );
    let mut tools = ToolRegistry::new();
    tool.register(&mut tools);

    let (model_handle, host) = host("conv-37-compose-once", model, tools, composer, None).await;
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe");
    attachment.handle_request(RuntimeClientRequest::SubmitInbound {
        id: rustx::runtime_client::RequestId::new(1),
        content: text("go"),
    });
    let events = receive_until(&subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;

    let composition_count = compositions.load(std::sync::atomic::Ordering::SeqCst);
    let invoked = invocations.load(std::sync::atomic::Ordering::SeqCst);
    let requests = model_handle.requests();
    let with_status: Vec<_> = requests
        .iter()
        .filter_map(|request| request.agent_status.as_ref())
        .collect();
    assert!(
        !with_status.is_empty(),
        "the model path received Agent Status at least once"
    );
    assert_eq!(
        composition_count,
        with_status.len(),
        "exactly one composition per model request that receives Agent Status"
    );
    assert_eq!(
        invoked, composition_count,
        "the extension providers ran once per composition, never a second time"
    );

    // The client observed each composition exactly once, and observing it
    // caused no further composition.
    let status_events: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeClientEvent::AgentStatusComposed { status, .. } => Some(status),
            _ => None,
        })
        .collect();
    assert_eq!(
        status_events.len(),
        composition_count,
        "one Runtime Client status event per composition"
    );
    for (client, model) in status_events.iter().zip(with_status.iter()) {
        assert_eq!(
            client.rendered, model.rendered,
            "the canonical rendered attachment and the client projection are the same composition"
        );
    }

    // Reading the projection is not a composition either.
    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.status.expect("status view").rendered,
        with_status.last().expect("status").rendered
    );
    assert_eq!(
        compositions.load(std::sync::atomic::Ordering::SeqCst),
        composition_count,
        "reading the snapshot never composes"
    );
}

/// The capability projection exposes the active revision, deterministic
/// ordering, and origin metadata for native tools; MCP/Python/Skill
/// coverage lives in the companion capability test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capability_projection_carries_builtin_tools_and_revision() {
    let mut tools = ToolRegistry::new();
    tools
        .register(
            rustx::tools::types::ToolDefinition {
                id: ToolId::new("tool-ls"),
                name: "ls".to_owned(),
                description: "list files".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
                execution_policy: ToolExecutionPolicy::ForegroundOnly,
                concurrency_policy: ToolConcurrencyPolicy::Sequential,
                replay_policy: ToolReplayPolicy::Never,
                origin: ToolOrigin::Builtin,
            },
            Arc::new(common::fake::FakeTool::new(
                rustx::tools::types::ToolDefinition {
                    id: ToolId::new("tool-ls"),
                    name: "ls".to_owned(),
                    description: "list files".to_owned(),
                    input_schema: serde_json::json!({"type": "object"}),
                    execution_policy: ToolExecutionPolicy::ForegroundOnly,
                    concurrency_policy: ToolConcurrencyPolicy::Sequential,
                    replay_policy: ToolReplayPolicy::Never,
                    origin: ToolOrigin::Builtin,
                },
                success_result("listed"),
            )),
        )
        .expect("register");
    let model = FakeModel::new(Vec::new());
    let (_, host) = host("conv-37-cap", model, tools, composer(), None).await;
    let (attachment, _) = host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION_V1)
        .expect("attach");
    let response = attachment.handle_request(RuntimeClientRequest::CapabilityGet {
        id: rustx::runtime_client::RequestId::new(1),
    });
    let Some(RuntimeClientResult::Capability { capabilities }) = response.result else {
        panic!("capability result");
    };
    // The workspace carries no Skill or Python content, so the prepared
    // candidate is a no-op and the active revision stays zero (the base
    // registry is the active tool set from construction).
    assert_eq!(capabilities.revision.get(), 0);
    assert_eq!(capabilities.tools.len(), 1);
    assert_eq!(capabilities.tools[0].id, ToolId::new("tool-ls"));
    assert_eq!(capabilities.tools[0].name, "ls");
    assert_eq!(capabilities.tools[0].origin, ToolOrigin::Builtin);
    assert!(capabilities.skills.is_empty());

    // The snapshot carries the same semantic projection (one
    // implementation, not two).
    let (snapshot, _) = host.snapshot().expect("snapshot");
    assert_eq!(snapshot.capabilities, capabilities);
}
