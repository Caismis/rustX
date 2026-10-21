use super::*;
use futures_util::StreamExt as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::durable::SqliteConversationStore;
use crate::message::content::TextBlock;
use crate::message::types::{
    ContentBlockIndex, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::{
    AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryFingerprint,
    AuxiliaryInputEvidence, AuxiliaryUsageCompleteness, ModelAdapter, ModelErrorKind, ModelEvent,
    ModelFinishReason, ModelInputMessage, ModelProtocol, ModelStream, ModelStreamItem, ModelUsage,
};
use crate::runtime::identity::{CompactionOperationId, ConversationId, MessageId};
use crate::runtime::monotonic::ManualMonotonicClock;

#[derive(Clone, Copy)]
enum Script {
    Complete,
    CompleteAfterRelease,
    PanicAtEntry,
    PanicAfterUsage,
    AwaitCancellation,
}

struct ConsumerAdapter {
    store: Arc<SqliteConversationStore>,
    call: Mutex<Option<AuxiliaryCallId>>,
    entries: AtomicUsize,
    waiting: Arc<Notify>,
    release: Arc<Notify>,
    exit_gate: Mutex<Option<OwnerExitGate>>,
    stream_exited: Arc<AtomicBool>,
    script: Script,
}

fn event(event: ModelEvent) -> ModelStreamItem {
    ModelStreamItem::Event(event)
}
fn usage(tokens: u64) -> ModelUsage {
    ModelUsage {
        input_tokens: 5,
        output_tokens: tokens,
        total_tokens: 5 + tokens,
        details: None,
    }
}

impl ModelAdapter for ConsumerAdapter {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }
    fn stream(&self, request: ModelRequest, cancellation: CancellationSignal) -> ModelStream {
        if let Some(gate) = self.exit_gate.lock().unwrap().take() {
            OWNER_EXIT_GATE.with(|slot| {
                assert!(slot.borrow().is_none());
                *slot.borrow_mut() = Some(gate);
            });
        }
        assert_eq!(
            self.entries.fetch_add(1, Ordering::SeqCst),
            0,
            "one physical entry per RequestId"
        );
        let call_id = self.call.lock().unwrap().clone().unwrap();
        let cut = self.store.auxiliary_audit_cut().unwrap();
        let call = self
            .store
            .load_auxiliary_call(&call_id, cut)
            .unwrap()
            .unwrap();
        let requests = self
            .store
            .read_auxiliary_call_requests(&call_id, cut, None, 32)
            .unwrap();
        assert_eq!(call.purpose.as_str(), "translation");
        assert_eq!(
            requests.len(),
            1,
            "adapter entry requires a real confirmed RequestStarted"
        );
        assert_eq!(requests[0].ordinal, 0);
        assert_eq!(requests[0].model, request.model());
        assert!(requests[0].started_sequence > call.started_sequence);
        assert!(requests[0].terminal_sequence.is_none());
        assert!(request.tools.is_empty());
        assert!(request.continuation.is_none());
        assert!(request.continuation_source.is_none());
        let prefix = futures_util::stream::iter([
            event(ModelEvent::Started),
            event(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "traduit".into(),
            }),
            event(ModelEvent::UsageUpdate { usage: usage(3) }),
            event(ModelEvent::UsageUpdate { usage: usage(7) }),
        ]);
        let stream: ModelStream = match self.script {
            Script::PanicAtEntry => panic!("adapter panicked before returning a stream"),
            Script::Complete => Box::pin(prefix.chain(futures_util::stream::iter([event(
                ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                },
            )]))),
            Script::PanicAfterUsage => Box::pin(prefix.chain(futures_util::stream::poll_fn(
                |_| -> std::task::Poll<Option<ModelStreamItem>> {
                    panic!("private consumer panicked after trusted usage")
                },
            ))),
            Script::CompleteAfterRelease => {
                let waiting = self.waiting.clone();
                let release = self.release.clone();
                Box::pin(prefix.chain(futures_util::stream::once(async move {
                    waiting.notify_one();
                    release.notified().await;
                    event(ModelEvent::Completed {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    })
                })))
            }
            Script::AwaitCancellation => {
                let waiting = self.waiting.clone();
                Box::pin(prefix.chain(futures_util::stream::once(async move {
                    waiting.notify_one();
                    cancellation.cancelled().await;
                    event(ModelEvent::Failed {
                        error: super::super::stream::local_error(
                            ModelErrorKind::Cancelled,
                            "adapter cancellation",
                        ),
                    })
                })))
            }
        };
        Box::pin(TrackedStream {
            inner: stream,
            exited: self.stream_exited.clone(),
        })
    }
}

struct TrackedStream {
    inner: ModelStream,
    exited: Arc<AtomicBool>,
}
impl futures_util::Stream for TrackedStream {
    type Item = ModelStreamItem;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(context)
    }
}
impl Drop for TrackedStream {
    fn drop(&mut self) {
        self.exited.store(true, Ordering::SeqCst);
    }
}

struct Fixture {
    executor: AuxiliaryExecutor,
    scope: Arc<AuxiliaryScope>,
    reservation: AuxiliaryReservation,
    frozen: FrozenCall,
    adapter: Arc<ConsumerAdapter>,
    store: Arc<SqliteConversationStore>,
    process: Arc<super::super::capacity::ProcessCapacity>,
    clock: Arc<ManualMonotonicClock>,
}

fn fixture(script: Script) -> Fixture {
    let store = Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
    let clock = Arc::new(ManualMonotonicClock::new());
    let process = super::super::capacity::ProcessCapacity::new();
    let lifecycle = ConversationLifecycle::new();
    assert!(lifecycle.activate());
    let executor = AuxiliaryExecutor::new(
        store.clone(),
        Arc::new(crate::runtime::types::SystemClock),
        clock.clone(),
        lifecycle,
        Arc::new(DurabilityGate::new()),
        process.conversation(),
    )
    .unwrap();
    let scope = AuxiliaryExecutor::scope(
        CompactionBudget::new(
            CompactionOperationId::new(uuid::Uuid::now_v7().to_string()),
            clock.clone(),
        ),
        &CancellationSignal::new(),
    );
    let reservation = executor.reserve_general(&scope).unwrap();
    let adapter = Arc::new(ConsumerAdapter {
        store: store.clone(),
        call: Mutex::new(Some(reservation.call_id().clone())),
        entries: AtomicUsize::new(0),
        waiting: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
        exit_gate: Mutex::new(None),
        stream_exited: Arc::new(AtomicBool::new(false)),
        script,
    });
    let invocation = crate::scripted_suites::support::attempt_model_with_window(
        adapter.clone(),
        "consumer",
        32768,
        128,
    )
    .summary_invocation()
    .clone();
    let request = ModelRequest {
        invocation: invocation.invocation_config(),
        messages: vec![ModelInputMessage::Canonical(MessageBlock::User(
            UserMessageBlock {
                id: MessageId::new("temporary-input"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "translate this".into(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            },
        ))]
        .into(),
        tools: Vec::new(),
        effective_system_prompt: "Translate into French".into(),
        images: std::collections::BTreeMap::default(),
        continuation: None,
        continuation_source: None,
    };
    let start = AuxiliaryCallStart {
        call_id: reservation.call_id().clone(),
        purpose: AuxiliaryAttribution::new("translation").unwrap(),
        trigger: AuxiliaryAttribution::new("private_fixture").unwrap(),
        business_source: AuxiliaryBusinessSource::Independent,
        input: AuxiliaryInputEvidence::Temporary,
        selected_message_ids: Vec::new(),
        semantic_input_fingerprint: AuxiliaryFingerprint::new(request.semantic_input_fingerprint())
            .unwrap(),
        invocation_fingerprint: invocation_fingerprint(&request.invocation, 7).unwrap(),
    };
    Fixture {
        executor,
        scope,
        reservation,
        frozen: FrozenCall {
            start,
            request,
            invocation,
            timeout: ModelTimeoutPolicy::default(),
            resource_revision: 7,
        },
        adapter,
        store,
        process,
        clock,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_private_second_consumer_uses_the_same_confirmed_single_request_executor() {
    let fixture = fixture(Script::Complete);
    let call_id = fixture.reservation.call_id().clone();
    let result = fixture
        .reservation
        .submit(fixture.frozen)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.output.unwrap().text, "traduit");
    assert!(fixture.adapter.stream_exited.load(Ordering::SeqCst));
    assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 1);
    let cut = fixture.store.auxiliary_audit_cut().unwrap();
    let call = fixture
        .store
        .load_auxiliary_call(&call_id, cut)
        .unwrap()
        .unwrap();
    assert_eq!(call.outcome, Some(AuxiliaryCallOutcome::Completed));
    let requests = fixture
        .store
        .read_auxiliary_call_requests(&call_id, cut, None, 32)
        .unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].usage, Some(usage(7)));
    assert_eq!(
        requests[0]
            .request_evidence
            .as_ref()
            .unwrap()
            .usage_completeness,
        AuxiliaryUsageCompleteness::Partial
    );
    fixture.scope.wait_closed().await;
    fixture.executor.drain().await;
}

#[tokio::test(flavor = "current_thread")]
async fn model_panic_keeps_trusted_usage_and_settles_without_reentry() {
    let fixture = fixture(Script::PanicAfterUsage);
    let call_id = fixture.reservation.call_id().clone();
    let error = fixture
        .reservation
        .submit(fixture.frozen)
        .unwrap()
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind, ContextErrorKind::AuxiliarySupervisionFailed);
    assert!(fixture.adapter.stream_exited.load(Ordering::SeqCst));
    assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 1);
    let cut = fixture.store.auxiliary_audit_cut().unwrap();
    assert_eq!(
        fixture
            .store
            .load_auxiliary_call(&call_id, cut)
            .unwrap()
            .unwrap()
            .outcome,
        Some(AuxiliaryCallOutcome::Failed)
    );
    let requests = fixture
        .store
        .read_auxiliary_call_requests(&call_id, cut, None, 32)
        .unwrap();
    let evidence = requests[0].request_evidence.as_ref().unwrap();
    assert_eq!(evidence.usage, Some(usage(7)));
    assert_eq!(
        evidence.usage_completeness,
        AuxiliaryUsageCompleteness::Partial
    );
    assert!(evidence.remote_execution_uncertainty);
    assert!(requests[0].terminal_sequence.is_some());
    fixture.scope.wait_closed().await;
    fixture.executor.drain().await;
}

#[tokio::test(flavor = "current_thread")]
async fn waiter_loss_and_parent_close_leave_cleanup_under_the_native_owner() {
    let fixture = fixture(Script::AwaitCancellation);
    let call_id = fixture.reservation.call_id().clone();
    let mut reply = fixture.reservation.submit(fixture.frozen).unwrap();
    tokio::select! {
        () = fixture.adapter.waiting.notified() => {}
        result = &mut reply => panic!("native execution ended before adapter entry: {result:?}"),
    }
    drop(reply);
    fixture.scope.wait_closed().await;
    assert!(fixture.adapter.stream_exited.load(Ordering::SeqCst));
    assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 1);
    let cut = fixture.store.auxiliary_audit_cut().unwrap();
    assert_eq!(
        fixture
            .store
            .load_auxiliary_call(&call_id, cut)
            .unwrap()
            .unwrap()
            .outcome,
        Some(AuxiliaryCallOutcome::Cancelled)
    );
    let requests = fixture
        .store
        .read_auxiliary_call_requests(&call_id, cut, None, 32)
        .unwrap();
    assert_eq!(
        requests[0].normalized_failure_kind,
        Some(ModelErrorKind::Cancelled)
    );
    assert_eq!(requests[0].usage, Some(usage(7)));
    assert!(
        requests[0]
            .request_evidence
            .as_ref()
            .unwrap()
            .remote_execution_uncertainty
    );
    let other_scope = AuxiliaryExecutor::scope(
        CompactionBudget::new(
            CompactionOperationId::new(uuid::Uuid::now_v7().to_string()),
            fixture.executor.0.resources.monotonic.clone(),
        ),
        &CancellationSignal::new(),
    );
    drop(fixture.executor.reserve_general(&other_scope).unwrap());
    fixture.executor.drain().await;
}

/// Exact native facts, not only the latest audit projection, are the oracle.
fn assert_completed_journal(store: &SqliteConversationStore, call_id: &AuxiliaryCallId) {
    use crate::events::types::RuntimeEvent;
    use crate::model::ModelRequestOwner;

    let events = store.read_events(None, 32).unwrap().events;
    assert_eq!(
        events.len(),
        4,
        "no replacement writes or invented terminals"
    );
    assert!(
        events
            .windows(2)
            .all(|pair| pair[1].sequence == pair[0].sequence + 1)
    );
    assert!(
        events
            .iter()
            .all(|event| event.attempt_id.is_none() && event.turn_id.is_none())
    );
    assert!(matches!(&events[0].event,
        RuntimeEvent::AuxiliaryCallStarted { call_id: actual, .. } if actual == call_id));
    let request_id = match &events[1].event {
        RuntimeEvent::ModelRequestStarted {
            owner:
                ModelRequestOwner::Auxiliary {
                    call_id: actual,
                    ordinal: 0,
                },
            request_id,
            ..
        } if actual == call_id => request_id,
        event => panic!("expected the only auxiliary request start, got {event:?}"),
    };
    assert!(matches!(&events[2].event,
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Auxiliary { call_id: actual, ordinal: 0 },
            request_id: terminal_id, usage: Some(actual_usage), ..
        } if actual == call_id && terminal_id == request_id && *actual_usage == usage(7)));
    assert!(matches!(&events[3].event,
        RuntimeEvent::AuxiliaryCallSettled {
            call_id: actual,
            outcome: AuxiliaryCallOutcome::Completed,
            request_evidence: Some(evidence),
        } if actual == call_id && evidence.request_id == *request_id
            && evidence.usage == Some(usage(7))
            && evidence.usage_completeness == AuxiliaryUsageCompleteness::Partial
            && !evidence.remote_execution_uncertainty));
    assert_eq!(
        store
            .auxiliary_audit_total(store.auxiliary_audit_cut().unwrap())
            .unwrap(),
        4
    );
    assert!(store.load_canonical().unwrap().is_empty());
}

struct CloseScopeOnDrop(Arc<AuxiliaryScope>);

impl Drop for CloseScopeOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}

// The adapter runs on the real native owner thread. Its TLS destructor runs
// after that thread has handed its record to the reaper, but before thread exit.
// This distinguishes joining the actual owner from merely dropping its handle.
thread_local! {
    static OWNER_EXIT_GATE: std::cell::RefCell<Option<OwnerExitGate>> = const {
        std::cell::RefCell::new(None)
    };
}

struct OwnerExitGate {
    entered: Option<oneshot::Sender<()>>,
    release: mpsc::Receiver<()>,
    explicitly_released: Arc<AtomicBool>,
}

impl Drop for OwnerExitGate {
    fn drop(&mut self) {
        let _ = self.entered.take().unwrap().send(());
        // Never panic in a thread destructor; the test asserts the release was
        // explicit. Dropping the sender also unblocks failure-path cleanup.
        self.explicitly_released.store(
            self.release
                .recv_timeout(std::time::Duration::from_secs(10))
                .is_ok(),
            Ordering::SeqCst,
        );
    }
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::too_many_lines)] // One real ownership chain, including its deliberately failed confirmation.
async fn saturated_native_owner_retains_permits_through_lost_settlement_ack_and_confirmation() {
    use super::super::capacity::CapacityDomain;
    use futures_util::FutureExt as _;

    for confirmation_panics in [false, true] {
        for drop_reply in [false, true] {
            let fixture = fixture(Script::CompleteAfterRelease);
            let _cancel_on_failure = CloseScopeOnDrop(fixture.scope.clone());
            let call_id = fixture.reservation.call_id().clone();
            let record = fixture.reservation.record.clone();
            let (owner_exiting, owner_exit_entered) = oneshot::channel();
            let (allow_exit, held_exit) = mpsc::channel();
            let explicitly_released = Arc::new(AtomicBool::new(false));
            *fixture.adapter.exit_gate.lock().unwrap() = Some(OwnerExitGate {
                entered: Some(owner_exiting),
                release: held_exit,
                explicitly_released: explicitly_released.clone(),
            });
            let mut reply = Some(fixture.reservation.submit(fixture.frozen).unwrap());
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                fixture.adapter.waiting.notified(),
            )
            .await
            .unwrap();
            let original_head = fixture.store.load_head().unwrap();
            let (committed, lose_ack, confirming, release_confirmation) = {
                let writer = record.writer.lock().unwrap();
                let writer = writer
                    .as_ref()
                    .expect("native record owns a physical writer");
                let (committed, lose_ack) = writer.hold_next_commit_for_test();
                let (confirming, release_confirmation) = writer.hold_confirmation_for_test();
                (committed, lose_ack, confirming, release_confirmation)
            };
            // 31 other live reservations leave exactly this Call's slot occupied.
            let mut retained = (0..2)
                .map(|_| fixture.executor.0.capacity.reserve_general().unwrap())
                .collect::<Vec<_>>();
            retained.push(fixture.executor.0.capacity.reserve_compaction().unwrap());
            for _ in 0..7 {
                let conversation = fixture.process.conversation();
                retained.extend((0..4).map(|_| conversation.reserve_compaction().unwrap()));
            }
            let unrelated = fixture.process.conversation();
            assert_eq!(
                unrelated.reserve_compaction().unwrap_err().domain,
                CapacityDomain::ProcessTotal
            );
            fixture.adapter.release.notify_one();
            tokio::time::timeout(std::time::Duration::from_secs(10), committed)
                .await
                .unwrap()
                .unwrap();
            assert!(fixture.adapter.stream_exited.load(Ordering::SeqCst));
            assert_completed_journal(&fixture.store, &call_id);
            let committed_events = fixture.store.read_events(None, 32).unwrap().events;
            assert!(
                matches!(
                    reply.as_mut().unwrap().try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ),
                "a committed settlement is not yet a confirmed completion"
            );
            if drop_reply {
                drop(reply.take());
            }
            fixture
                .clock
                .advance(crate::context::compaction_budget::COMPACTION_DEADLINE_MILLIS);
            fixture.scope.close();
            assert!(
                fixture.executor.drain().now_or_never().is_none(),
                "drain cannot infer physical exit from cancelled inference or committed SQL"
            );
            assert_eq!(
                unrelated.reserve_compaction().unwrap_err().domain,
                CapacityDomain::ProcessTotal
            );
            drop(lose_ack); // Panic only after the real settlement committed and returned its lease.
            tokio::time::timeout(std::time::Duration::from_secs(10), confirming)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                fixture.store.read_events(None, 32).unwrap().events,
                committed_events,
                "exact confirmation must not submit a replacement transaction"
            );
            assert!(fixture.executor.drain().now_or_never().is_none());
            assert_eq!(
                unrelated.reserve_compaction().unwrap_err().domain,
                CapacityDomain::ProcessTotal,
                "confirmation is still live owned work even though SQL has exited"
            );
            if let Some(reply) = reply.as_mut() {
                assert!(matches!(
                    reply.try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
            }
            if confirmation_panics {
                drop(release_confirmation);
            } else {
                release_confirmation.send(()).unwrap();
            }
            tokio::time::timeout(std::time::Duration::from_secs(10), owner_exit_entered)
                .await
                .unwrap()
                .unwrap();
            assert!(
                fixture.executor.drain().now_or_never().is_none(),
                "reaper handoff and SQL exit cannot replace the actual native-thread join"
            );
            assert_eq!(
                unrelated.reserve_compaction().unwrap_err().domain,
                CapacityDomain::ProcessTotal,
                "the real owner thread has not physically exited yet"
            );
            if let Some(reply) = reply.as_mut() {
                assert!(matches!(
                    reply.try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
            }
            allow_exit.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(10), fixture.executor.drain())
                .await
                .unwrap();
            assert!(
                explicitly_released.load(Ordering::SeqCst),
                "thread exit was explicitly released, not a timeout"
            );
            if let Some(reply) = reply {
                let result = tokio::time::timeout(std::time::Duration::from_secs(10), reply)
                    .await
                    .unwrap()
                    .unwrap();
                if confirmation_panics {
                    assert_eq!(
                        result.unwrap_err().kind,
                        ContextErrorKind::AuxiliaryDurabilityFailed
                    );
                } else {
                    assert_eq!(
                        result.unwrap().output.unwrap().text,
                        "traduit",
                        "cancellation after the sealed outcome cannot rewrite successful execution"
                    );
                }
            }
            assert!(
                lock(&record.state).worker.is_none(),
                "the native reaper consumed the actual join"
            );
            assert!(lock(&fixture.executor.0.state).calls.is_empty());
            assert_eq!(
                lock(&fixture.executor.0.state).exited_obligations.len(),
                usize::from(confirmation_panics)
            );
            assert_eq!(
                fixture.executor.0.resources.durability.is_failed(),
                confirmation_panics
            );
            let replacement = unrelated
                .reserve_compaction()
                .expect("proven exit releases process capacity even after durability handoff");
            assert_eq!(
                unrelated.reserve_compaction().unwrap_err().domain,
                CapacityDomain::ProcessTotal,
                "exactly one slot was released, not another Call's permits"
            );
            assert_completed_journal(&fixture.store, &call_id);
            assert_eq!(
                fixture.store.read_events(None, 32).unwrap().events,
                committed_events
            );
            assert_eq!(fixture.store.load_head().unwrap(), original_head);
            assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 1);
            drop(replacement);
            drop(retained);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rejected_frozen_authority_never_starts_a_call_or_leaks_native_reservations() {
    for corruption in 0..6 {
        let mut fixture = fixture(Script::Complete);
        match corruption {
            0 => fixture.frozen.start.call_id = AuxiliaryCallId::generate(),
            1 => {
                fixture.frozen.start.semantic_input_fingerprint =
                    AuxiliaryFingerprint::new("0".repeat(64)).unwrap();
            }
            2 => fixture.frozen.resource_revision += 1,
            3 => fixture.frozen.request.invocation.model = "another-model".into(),
            4 => fixture.frozen.timeout.response_start_timeout = std::time::Duration::ZERO,
            5 => fixture.frozen.timeout.stream_idle_timeout = std::time::Duration::ZERO,
            _ => unreachable!(),
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            fixture.reservation.submit(fixture.frozen).unwrap(),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
        assert_eq!(
            result.kind,
            ContextErrorKind::AuxiliaryNotStarted,
            "case {corruption}"
        );
        assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 0);
        assert!(
            fixture
                .store
                .read_events(None, 32)
                .unwrap()
                .events
                .is_empty(),
            "unregistered durable work must not acquire an invented Call terminal"
        );
        assert_eq!(
            fixture
                .store
                .auxiliary_audit_total(fixture.store.auxiliary_audit_cut().unwrap())
                .unwrap(),
            0
        );
        assert!(!fixture.executor.0.resources.durability.is_failed());
        assert!(lock(&fixture.executor.0.state).calls.is_empty());
        assert!(lock(&fixture.scope.state).children.is_empty());
        fixture
            .scope
            .authorize_commit()
            .expect("failed preparation left no live child");
        let mut permits = Vec::new();
        for _ in 0..8 {
            let conversation = fixture.process.conversation();
            permits.extend((0..3).map(|_| conversation.reserve_general().unwrap()));
            permits.push(conversation.reserve_compaction().unwrap());
        }
        assert_eq!(
            permits.len(),
            32,
            "all process and writer reservations were returned"
        );
        assert!(fixture.process.conversation().reserve_compaction().is_err());
        drop(permits);
        fixture.scope.wait_closed().await;
        fixture.executor.drain().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn adapter_entry_panic_consumes_dispatch_and_preserves_unreported_remote_uncertainty() {
    use crate::events::types::RuntimeEvent;
    use crate::model::ModelRequestOwner;

    let fixture = fixture(Script::PanicAtEntry);
    let record = fixture.reservation.record.clone();
    let call_id = record.call_id.clone();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        fixture.reservation.submit(fixture.frozen).unwrap(),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap_err();
    assert_eq!(result.kind, ContextErrorKind::AuxiliarySupervisionFailed);
    assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 1);
    assert!(
        !fixture.adapter.stream_exited.load(Ordering::SeqCst),
        "there was no stream to drop: native supervision must own settlement"
    );
    assert!(
        !fixture.executor.0.resources.durability.is_failed(),
        "provider panic is not a persistence failure after confirmed settlement"
    );
    assert_eq!(
        record.grant_dispatch().unwrap_err().kind,
        ContextErrorKind::AuxiliarySupervisionFailed,
        "a panicked adapter must not regain its consumed dispatch grant"
    );
    let events = fixture.store.read_events(None, 32).unwrap().events;
    assert_eq!(events.len(), 4);
    assert!(matches!(&events[1].event,
        RuntimeEvent::ModelRequestStarted {
            owner: ModelRequestOwner::Auxiliary { call_id: actual, ordinal: 0 },
            request_id, ..
        } if actual == &call_id && request_id == &record.request_id));
    assert!(matches!(&events[2].event,
        RuntimeEvent::ModelRequestFailed {
            owner: ModelRequestOwner::Auxiliary { call_id: actual, ordinal: 0 },
            request_id, usage: None, ..
        } if actual == &call_id && request_id == &record.request_id));
    assert!(matches!(&events[3].event,
        RuntimeEvent::AuxiliaryCallSettled {
            call_id: actual,
            outcome: AuxiliaryCallOutcome::Failed,
            request_evidence: Some(evidence),
        } if actual == &call_id && evidence.request_id == record.request_id
            && evidence.observed_provider_terminal.is_none()
            && evidence.usage.is_none()
            && evidence.usage_completeness == AuxiliaryUsageCompleteness::Unreported
            && evidence.remote_execution_uncertainty));
    assert_eq!(
        events[3].sequence,
        events[2].sequence + 1,
        "the final Request and Call settle as one atomic pair"
    );
    fixture.scope.wait_closed().await;
    fixture.executor.drain().await;
    assert_eq!(fixture.store.read_events(None, 32).unwrap().events, events);
}
