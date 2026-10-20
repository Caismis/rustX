use crate::agent::cancellation::AgentCancellation;
use crate::agent::execution::AgentExecution;
use crate::agent::execution::test_fixtures::*;
use crate::durable::inbox::ConversationStore;
use crate::events::types::{AttemptOutcome, RuntimeEvent};
use crate::message::types::ContentBlockIndex;
use crate::model::adapter::{ModelAdapter, ModelStream, ModelStreamItem};
use crate::model::chat_protocol;
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::types::{ModelProtocol, ModelRequest};
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
use crate::runtime::types::CancellationReason;
use crate::scripted_suites::common::{capability_lease, tool_runtime_with_store};
use crate::scripted_suites::support::model::scripted_session_model;
use crate::tools::executor::ToolRegistry;
use crate::tools::types::ToolCall;
use std::sync::Arc;

/// A committed start, controlled preparation gap, then actual adapter dispatch.
/// The stream also delays EOF to prove it cannot move the provider terminal.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn generation_bridge_survives_settlement_and_reopen() {
    use crate::model::types::ModelUsage;
    use crate::observation::trace::TraceKind;
    use crate::runtime::monotonic::{ManualMonotonicClock, MonotonicClock};
    use crate::runtime_client::trace::TraceProjection;
    fn events(store: &dyn ConversationStore) -> Vec<RuntimeEvent> {
        store
            .read_events(None, 32)
            .unwrap()
            .events
            .into_iter()
            .map(|e| e.event)
            .collect()
    }
    struct TimedAdapter {
        clock: Arc<ManualMonotonicClock>,
        store: Arc<crate::durable::SqliteConversationStore>,
    }
    impl ModelAdapter for TimedAdapter {
        fn protocol(&self) -> ModelProtocol {
            chat_protocol()
        }
        fn stream(&self, _request: ModelRequest, _cancellation: CancellationSignal) -> ModelStream {
            assert_eq!(
                self.clock.now_millis(),
                400,
                "dispatch follows the controlled gap"
            );
            assert!(
                events(self.store.as_ref())
                    .iter()
                    .any(|e| matches!(e, RuntimeEvent::ModelRequestStarted { .. }))
            );
            let clock = self.clock.clone();
            let mut index = 0;
            Box::pin(futures_util::stream::poll_fn(move |_| {
                let item = match index {
                    0 => Some(ModelEvent::Started),
                    1 => {
                        clock.advance(320);
                        Some(ModelEvent::TextDelta {
                            block_index: ContentBlockIndex::new(0),
                            text: "hello".into(),
                        })
                    }
                    2 => {
                        clock.advance(1_200);
                        Some(ModelEvent::TextDelta {
                            block_index: ContentBlockIndex::new(0),
                            text: " world".into(),
                        })
                    }
                    3 => Some(ModelEvent::UsageUpdate {
                        usage: ModelUsage {
                            input_tokens: 40,
                            output_tokens: 120,
                            total_tokens: 160,
                            details: None,
                        },
                    }),
                    4 => {
                        clock.advance(80);
                        Some(ModelEvent::Completed {
                            finish_reason: ModelFinishReason::Stop,
                            usage: None,
                        })
                    }
                    _ => {
                        clock.advance(500);
                        None
                    }
                };
                index += 1;
                std::task::Poll::Ready(item.map(ModelStreamItem::Event))
            }))
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("timing.sqlite");
    let seed = Arc::new(ScriptedAdapter::new(vec![]));
    let mut request = request(&seed);
    let id = request.conversation_id.clone();
    let store = Arc::new(crate::durable::SqliteConversationStore::open(id.clone(), &path).unwrap());
    let clock = Arc::new(ManualMonotonicClock::new());
    let adapter = Arc::new(TimedAdapter {
        clock: clock.clone(),
        store: store.clone(),
    });
    request.model = scripted_session_model(adapter).snapshot().unwrap();
    let tool_runtime = tool_runtime_with_store(id.as_str(), Some(store.clone()));
    let (lease, coordinator) = capability_lease(ToolRegistry::new(), &tool_runtime)
        .await
        .into_lease_and_coordinator();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let observer = RecordingObserver {
        start_gap: Some((clock.clone(), store.clone())),
        ..Default::default()
    };
    let mut execution = AgentExecution::new(
        request,
        lease,
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime(&seed),
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .unwrap();
    execution
        .install_publication_policy(crate::publication::CoalescePolicy::default(), clock.clone());
    execution.observe(&observer);
    let result = execution.run().await;
    assert!(matches!(result.outcome, AttemptOutcome::Completed { .. }));
    let evidence = events(store.as_ref())
        .into_iter()
        .find_map(|event| match event {
            RuntimeEvent::ModelRequestCompleted { generation, .. } => generation,
            _ => None,
        })
        .expect("settled evidence");
    assert_eq!(evidence.dispatch_after_start_ms, Some(400));
    assert_eq!(evidence.first_output_ms, Some(320));
    assert_eq!(evidence.last_output_ms, Some(1_520));
    assert_eq!(evidence.terminal_ms, 1_600);
    assert_eq!(evidence.generation_ms(), Some(1_280));
    let page = TraceProjection::new(store.as_ref())
        .unwrap()
        .page(None, 32)
        .unwrap();
    let record = page
        .records
        .iter()
        .find(|r| r.kind == TraceKind::Request)
        .unwrap();
    let generation = record.request.as_ref().unwrap().generation.unwrap();
    let timeline = generation.timeline.unwrap();
    assert_eq!(timeline.dispatch_ms, 400);
    assert_eq!(timeline.first_output_ms, Some(720));
    assert_eq!(timeline.last_output_ms, Some(1_920));
    assert_eq!(timeline.terminal_ms, 2_000);
    assert_eq!(generation.output_tokens_per_second, Some(120.0 / 1.28));
    drop(result);
    drop(observer);
    drop(tool_runtime);
    drop(coordinator);
    assert_eq!(
        Arc::strong_count(&store),
        1,
        "close every store owner before reopen"
    );
    drop(store);
    clock.advance(100_000);
    let reopened = crate::durable::SqliteConversationStore::open(id, &path).unwrap();
    assert_eq!(
        TraceProjection::new(&reopened)
            .unwrap()
            .page(None, 32)
            .unwrap(),
        page
    );
    assert!(
        events(&reopened).iter().any(|event| matches!(event,
        RuntimeEvent::ModelRequestCompleted { generation: Some(value), .. } if *value == evidence))
    );
}

/// A long scripted tool loop grows the durable authorities while the
/// active execution retains only its current working state. Event and
/// Request Snapshot inspection deliberately walks the store in bounded
/// pages; the settlement result has no historical trace collections to
/// retain or transfer.
#[tokio::test]
async fn long_attempt_history_is_durable_and_boundedly_inspectable() {
    const TOOL_TURNS: usize = 40;
    const PAGE_SIZE: usize = 7;

    let mut scripts = Vec::with_capacity(TOOL_TURNS + 1);
    for turn in 0..TOOL_TURNS {
        let call = ToolCall {
            id: ToolCallId::new(format!("call-{turn}")),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".to_owned(),
            arguments: serde_json::json!({}),
        };
        scripts.push(tool_call_script(&call));
    }
    scripts.push(vec![
        ModelEvent::Started,
        ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "final".to_owned(),
        },
        ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
    ]);

    let adapter = Arc::new(ScriptedAdapter::new(scripts));
    let store = Arc::new(
        crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .expect("in-memory store"),
    );
    let tool_runtime = tool_runtime_with_store(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        Some(store.clone()),
    );
    let mut tools = ToolRegistry::new();
    tools
        .register(
            InstantTool::definition("tool-alpha", "alpha"),
            Arc::new(InstantTool),
        )
        .expect("register scripted tool");
    let lease = capability_lease(tools, &tool_runtime).await.into_lease();
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
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;

    assert!(matches!(
        result.outcome,
        AttemptOutcome::Completed {
            finish_reason: ModelFinishReason::Stop
        }
    ));
    assert_eq!(adapter.request_count(), TOOL_TURNS + 1);

    assert_bounded_event_history(store.as_ref(), PAGE_SIZE, TOOL_TURNS);
    assert_bounded_request_history(
        store.as_ref(),
        PAGE_SIZE,
        TOOL_TURNS + 1,
        &adapter.requests(),
    );
}

fn assert_bounded_event_history(
    store: &dyn ConversationStore,
    page_size: usize,
    minimum_event_count: usize,
) {
    let mut cursor = None;
    let mut sequences = Vec::new();
    let mut pages = 0;
    let mut last_event = None;
    loop {
        let page = store
            .read_events(cursor, page_size)
            .expect("event journal page");
        assert!(page.events.len() <= page_size);
        if page.events.is_empty() {
            break;
        }
        pages += 1;
        sequences.extend(page.events.iter().map(|event| event.sequence));
        last_event = page.events.last().map(|event| event.event.clone());
        cursor = page.next_sequence;
    }
    assert!(pages > 1, "the journal must be inspected in pages");
    assert!(!sequences.is_empty());
    assert!(
        sequences.len() > minimum_event_count,
        "the long attempt must commit more events than tool turns"
    );
    assert!(sequences.windows(2).all(|window| window[0] < window[1]));
    assert!(matches!(
        store
            .read_events(sequences.last().copied(), page_size)
            .expect("terminal journal page")
            .events
            .as_slice(),
        []
    ));
    assert!(matches!(
        last_event,
        Some(RuntimeEvent::AttemptCompleted { .. })
    ));
}

fn assert_bounded_request_history(
    store: &dyn ConversationStore,
    page_size: usize,
    expected_count: usize,
    provider_requests: &[ModelRequest],
) {
    let mut cursor = None;
    let mut snapshots = Vec::new();
    let mut pages = 0;
    loop {
        let page = store
            .read_request_snapshots(cursor, page_size)
            .expect("request snapshot page");
        assert!(page.snapshots.len() <= page_size);
        if page.snapshots.is_empty() {
            break;
        }
        pages += 1;
        snapshots.extend(page.snapshots);
        cursor = page.next_sequence;
    }
    assert!(pages > 1, "request history must be inspected in pages");
    assert_eq!(snapshots.len(), expected_count);
    assert!(
        snapshots
            .windows(2)
            .all(|window| window[0].request_id != window[1].request_id)
    );
    assert!(snapshots.iter().enumerate().all(|(index, snapshot)| {
        snapshot.identity.turn.as_str() == (index + 1).to_string()
            && snapshot.identity.retry_number == 0
    }));
    for (snapshot, request) in snapshots.iter().zip(provider_requests.iter()) {
        assert_eq!(
            store
                .reconstruct_model_request(&snapshot.request_id)
                .expect("historical request reconstruction"),
            *request
        );
    }
}
