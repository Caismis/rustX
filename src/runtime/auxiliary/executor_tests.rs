use super::*;
use crate::durable::{AuxiliaryStore, CanonicalStore, JournalStore};
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
    fixture_in_process(script, super::super::capacity::ProcessCapacity::new())
}

fn fixture_in_process(
    script: Script,
    process: Arc<super::super::capacity::ProcessCapacity>,
) -> Fixture {
    let store = Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
    let clock = Arc::new(ManualMonotonicClock::new());
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
    for corruption in 0..7 {
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
            // Frozen model authority is valid; the real Store refuses this
            // nonexistent historical input reference without poisoning health.
            6 => fixture
                .frozen
                .start
                .selected_message_ids
                .push(MessageId::new("missing-selected-input")),
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

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::too_many_lines)] // One held physical owner versus independent retirement in the same saturated process.
async fn an_unexited_owner_does_not_block_another_conversations_retirement() {
    use super::super::capacity::{CapacityDomain, ProcessCapacity};
    use futures_util::FutureExt as _;

    let process = ProcessCapacity::new();
    let first = fixture_in_process(Script::Complete, process.clone());
    let second = fixture_in_process(Script::Complete, process.clone());
    let _close_first = CloseScopeOnDrop(first.scope.clone());
    let _close_second = CloseScopeOnDrop(second.scope.clone());
    let first_record = first.reservation.record.clone();
    let second_record = second.reservation.record.clone();
    let domains = (0..8).map(|_| process.conversation()).collect::<Vec<_>>();
    let _held = (0..30)
        .map(|index| domains[index / 4].reserve_compaction().unwrap())
        .collect::<Vec<_>>();
    let probe = process.conversation();
    assert_eq!(
        probe.reserve_compaction().unwrap_err().domain,
        CapacityDomain::ProcessTotal
    );
    let (owner_exiting, owner_exit_entered) = oneshot::channel();
    let (allow_exit, held_exit) = mpsc::channel();
    let explicitly_released = Arc::new(AtomicBool::new(false));
    *first.adapter.exit_gate.lock().unwrap() = Some(OwnerExitGate {
        entered: Some(owner_exiting),
        release: held_exit,
        explicitly_released: explicitly_released.clone(),
    });
    let mut first_reply = first.reservation.submit(first.frozen).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), owner_exit_entered)
        .await
        .unwrap()
        .unwrap();
    assert!(first.executor.drain().now_or_never().is_none());
    assert!(matches!(
        first_reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let second_reply = second.reservation.submit(second.frozen).unwrap();
    let completed = tokio::time::timeout(std::time::Duration::from_secs(10), second_reply)
        .await
        .expect("unrelated completed Call must retire before the held owner exits")
        .unwrap()
        .unwrap();
    assert_eq!(completed.output.unwrap().text, "traduit");
    assert!(
        lock(&first_record.state).permit.is_some(),
        "the unexited owner must retain its physical permit"
    );
    assert!(lock(&second_record.state).permit.is_none());
    assert!(
        lock(&second_record.state).worker.is_none(),
        "B delivered only after its actual owner join"
    );
    assert!(matches!(
        first_reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let recovered = probe
        .reserve_compaction()
        .expect("B released its process allocation independently");
    assert_eq!(
        probe.reserve_compaction().unwrap_err().domain,
        CapacityDomain::ProcessTotal
    );
    drop(recovered);
    second.executor.drain().await;
    allow_exit.send(()).unwrap();
    let completed = tokio::time::timeout(std::time::Duration::from_secs(10), first_reply)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(completed.output.unwrap().text, "traduit");
    first.executor.drain().await;
    assert!(explicitly_released.load(Ordering::SeqCst));
    assert!(lock(&first_record.state).worker.is_none());
    assert!(lock(&first_record.state).permit.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn saturated_active_admission_remains_busy_until_cancellation_wins() {
    let fixture = fixture(Script::Complete);
    let domains = (0..8)
        .map(|_| fixture.process.conversation())
        .collect::<Vec<_>>();
    let _held = (0..31)
        .map(|index| domains[index / 4].reserve_compaction().unwrap())
        .collect::<Vec<_>>();
    let error = fixture
        .executor
        .reserve(&fixture.scope)
        .err()
        .expect("active saturated admission refuses");
    assert_eq!(error.kind, ContextErrorKind::AuxiliaryBusy);
    fixture.scope.cancellation.cancel();
    let error = fixture
        .executor
        .reserve(&fixture.scope)
        .err()
        .expect("cancelled saturated admission refuses");
    assert_eq!(error.kind, ContextErrorKind::Cancelled);
}

#[tokio::test(flavor = "current_thread")]
async fn production_dispatch_grant_arbitrates_cancellation_and_parent_close() {
    for after_grant in [false, true] {
        for close_parent in [false, true] {
            let fixture = fixture(Script::AwaitCancellation);
            let _close = CloseScopeOnDrop(fixture.scope.clone());
            let record = fixture.reservation.record.clone();
            let call_id = record.call_id.clone();
            let (entered, ready) = oneshot::channel();
            let (release, held) = mpsc::channel();
            lock(&record.state).dispatch_gate = Some(DispatchTestGate {
                after_grant,
                entered,
                release: held,
            });
            let reply = fixture.reservation.submit(fixture.frozen).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(10), ready)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 0);
            assert_eq!(lock(&record.state).dispatched, after_grant);
            let started = fixture.store.read_events(None, 32).unwrap().events;
            assert_eq!(
                started.len(),
                2,
                "both starts are confirmed before the real grant boundary"
            );
            if close_parent {
                fixture.scope.close();
            } else {
                fixture.scope.cancellation.cancel();
            }
            release.send(()).unwrap();
            let result = tokio::time::timeout(std::time::Duration::from_secs(10), reply)
                .await
                .unwrap()
                .unwrap();
            match result {
                Ok(generation) => assert_eq!(
                    generation.output.unwrap_err().kind,
                    ModelErrorKind::Cancelled
                ),
                Err(error) => assert_eq!(error.kind, ContextErrorKind::Cancelled),
            }
            assert_eq!(
                fixture.adapter.entries.load(Ordering::SeqCst),
                usize::from(after_grant)
            );
            let cut = fixture.store.auxiliary_audit_cut().unwrap();
            let call = fixture
                .store
                .load_auxiliary_call(&call_id, cut)
                .unwrap()
                .unwrap();
            assert_eq!(call.outcome, Some(AuxiliaryCallOutcome::Cancelled));
            let requests = fixture
                .store
                .read_auxiliary_call_requests(&call_id, cut, None, 32)
                .unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].request_id, record.request_id);
            assert!(requests[0].terminal_sequence.is_some());
            assert_eq!(
                requests[0].normalized_failure_kind,
                Some(ModelErrorKind::Cancelled)
            );
            assert_eq!(
                requests[0]
                    .request_evidence
                    .as_ref()
                    .unwrap()
                    .remote_execution_uncertainty,
                after_grant
            );
            assert!(
                record.grant_dispatch().is_err(),
                "cancelled/consumed dispatch cannot be re-entered"
            );
            assert_eq!(
                fixture.adapter.entries.load(Ordering::SeqCst),
                usize::from(after_grant)
            );
            fixture.scope.wait_closed().await;
            fixture.executor.drain().await;
        }
    }
}

async fn submit_with_known_failure_cancellation(
    reservation: AuxiliaryReservation,
    frozen: FrozenCall,
    coincident_cancel: bool,
) -> Result<ConfirmedExecution, ContextError> {
    let record = reservation.record.clone();
    let probe = coincident_cancel.then(|| {
        SqliteConversationStore::hold_auxiliary_start_probe_for_test(
            record.request_id.to_string(),
            crate::durable::sqlite::AuxiliaryStartProbe::Orphan,
        )
    });
    let reply = reservation.submit(frozen).unwrap();
    if let Some((entered, release)) = probe {
        tokio::time::timeout(std::time::Duration::from_secs(10), entered)
            .await
            .unwrap()
            .unwrap();
        let (failed, release_failure) = lock(&record.writer)
            .as_ref()
            .unwrap()
            .hold_known_failure_for_test();
        let cancelled = lock(&record.writer)
            .as_ref()
            .unwrap()
            .observe_start_cancellation_for_test();
        drop(release);
        tokio::time::timeout(std::time::Duration::from_secs(10), failed)
            .await
            .unwrap()
            .unwrap();
        // The real Storage error and proven rollback precede cancellation;
        // its evidence is still awaiting delivery to the native owner.
        record.scope.cancellation.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(10), cancelled)
            .await
            .unwrap()
            .unwrap();
        release_failure.send(()).unwrap();
    }
    reply.await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn required_request_start_rollback_refunds_budget_but_fails_native_durability() {
    for (automatic_rollback, coincident_cancel) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let fixture = fixture(Script::Complete);
        let call_id = fixture.reservation.call_id().clone();
        let seen = fixture
            .store
            .fail_auxiliary_request_insert_once_for_test(automatic_rollback);
        let _close = CloseScopeOnDrop(fixture.scope.clone());
        let result = Box::pin(submit_with_known_failure_cancellation(
            fixture.reservation,
            fixture.frozen,
            coincident_cancel,
        ))
        .await;
        assert_eq!(
            seen.load(Ordering::SeqCst),
            1,
            "the real RequestStart INSERT fault was consumed"
        );
        assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 0);
        let cut = fixture.store.auxiliary_audit_cut().unwrap();
        let call = fixture
            .store
            .load_auxiliary_call(&call_id, cut)
            .unwrap()
            .unwrap();
        if coincident_cancel {
            assert_eq!(
                call.outcome,
                Some(AuxiliaryCallOutcome::Cancelled),
                "logical cancellation remains separate from the required Storage failure"
            );
        } else {
            assert_eq!(
                call.outcome,
                Some(AuxiliaryCallOutcome::Failed),
                "Call-only settlement succeeds after RequestStart rollback"
            );
        }
        assert!(
            fixture
                .store
                .read_auxiliary_call_requests(&call_id, cut, None, 32)
                .unwrap()
                .is_empty()
        );
        let journal = fixture.store.read_events(None, 32).unwrap().events;
        assert_eq!(
            journal.len(),
            2,
            "only CallStarted and CallSettled survive rollback"
        );
        let fresh = CancellationSignal::new();
        let allowance = (0..12)
            .map(|_| {
                fixture
                    .scope
                    .budget
                    .reserve_start(
                        crate::runtime::identity::RequestId::new(uuid::Uuid::now_v7().to_string()),
                        &fresh,
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            allowance.len(),
            12,
            "proven noncommit refunds the start allowance"
        );
        assert!(
            fixture.executor.0.resources.durability.failure().is_some(),
            "required storage failure must reach native durability despite successful rollback/settlement"
        );
        assert_eq!(
            result.expect_err("failed required start").kind,
            ContextErrorKind::AuxiliaryDurabilityFailed
        );
        let scope = AuxiliaryExecutor::scope(
            CompactionBudget::new(
                CompactionOperationId::new(uuid::Uuid::now_v7().to_string()),
                fixture.clock.clone(),
            ),
            &fresh,
        );
        assert_eq!(
            fixture.executor.reserve_general(&scope).err().unwrap().kind,
            ContextErrorKind::AuxiliaryDurabilityFailed
        );
        fixture.executor.drain().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_request_start_rollback_refunds_budget_without_failing_durability() {
    let fixture = fixture(Script::Complete);
    let _close = CloseScopeOnDrop(fixture.scope.clone());
    let call_id = fixture.reservation.call_id().clone();
    let entered = fixture
        .store
        .park_auxiliary_request_insert_until_interrupted_for_test();
    let reply = fixture.reservation.submit(fixture.frozen).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), entered)
        .await
        .unwrap()
        .unwrap();
    fixture.scope.cancellation.cancel();
    let error = tokio::time::timeout(std::time::Duration::from_secs(10), reply)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        error.kind,
        ContextErrorKind::Cancelled,
        "the real interrupted INSERT rolls back as cancellation"
    );
    assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 0);
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
    assert!(
        fixture
            .store
            .read_auxiliary_call_requests(&call_id, cut, None, 32)
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.store.read_events(None, 32).unwrap().events.len(), 2);
    let fresh = CancellationSignal::new();
    let allowances = (0..12)
        .map(|_| {
            fixture
                .scope
                .budget
                .reserve_start(
                    crate::runtime::identity::RequestId::new(uuid::Uuid::now_v7().to_string()),
                    &fresh,
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(allowances.len(), 12);
    assert!(
        fixture.executor.0.resources.durability.failure().is_none(),
        "confirmed cancellation rollback does not poison native authority"
    );
    let scope = AuxiliaryExecutor::scope(
        CompactionBudget::new(
            CompactionOperationId::new(uuid::Uuid::now_v7().to_string()),
            fixture.clock.clone(),
        ),
        &fresh,
    );
    drop(
        fixture
            .executor
            .reserve_general(&scope)
            .expect("later active admission remains healthy"),
    );
    fixture.executor.drain().await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_during_shared_start_probes_refunds_budget_without_durability_failure() {
    use crate::durable::sqlite::AuxiliaryStartProbe;
    for probe in [AuxiliaryStartProbe::Orphan, AuxiliaryStartProbe::StartEvent] {
        let fixture = fixture(Script::Complete);
        let _close = CloseScopeOnDrop(fixture.scope.clone());
        let record = fixture.reservation.record.clone();
        let (entered, release) = SqliteConversationStore::hold_auxiliary_start_probe_for_test(
            record.request_id.to_string(),
            probe,
        );
        let reply = fixture.reservation.submit(fixture.frozen).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), entered)
            .await
            .unwrap()
            .unwrap();
        let cancelled = lock(&record.writer)
            .as_ref()
            .unwrap()
            .observe_start_cancellation_for_test();
        fixture.scope.cancellation.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(10), cancelled)
            .await
            .unwrap()
            .unwrap();
        // The native cancellation reached this exact active connection lease
        // while its real SELECT/progress callback remained parked.
        drop(release);
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), reply)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 0);
        let cut = fixture.store.auxiliary_audit_cut().unwrap();
        assert_eq!(
            fixture
                .store
                .load_auxiliary_call(&record.call_id, cut)
                .unwrap()
                .unwrap()
                .outcome,
            Some(AuxiliaryCallOutcome::Cancelled)
        );
        assert!(
            fixture
                .store
                .read_auxiliary_call_requests(&record.call_id, cut, None, 32)
                .unwrap()
                .is_empty()
        );
        let fresh = CancellationSignal::new();
        let allowances = (0..12)
            .map(|_| {
                fixture
                    .scope
                    .budget
                    .reserve_start(
                        crate::runtime::identity::RequestId::new(uuid::Uuid::now_v7().to_string()),
                        &fresh,
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            allowances.len(),
            12,
            "real SELECT rollback restores the full start allowance: {probe:?}"
        );
        assert_eq!(
            result.unwrap_err().kind,
            ContextErrorKind::Cancelled,
            "interruption retains its control cause: {probe:?}"
        );
        assert!(
            fixture.executor.0.resources.durability.failure().is_none(),
            "normal probe cancellation must not poison native admission: {probe:?}"
        );
        let next = AuxiliaryExecutor::scope(
            CompactionBudget::new(
                CompactionOperationId::new(uuid::Uuid::now_v7().to_string()),
                fixture.clock.clone(),
            ),
            &fresh,
        );
        drop(
            fixture
                .executor
                .reserve_general(&next)
                .expect("later active admission remains usable"),
        );
        fixture.executor.drain().await;
    }
}

fn historical_summary_input(
    fixture: &Fixture,
) -> (
    crate::context::summarizer::SummaryRequest,
    crate::context::resources::ResourceReadCut,
    crate::model::AuxiliaryAuditCut,
) {
    use crate::context::summarizer::SummaryRequest;
    use crate::durable::{ConversationStore, ProjectionStore};
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    let message = match &fixture.frozen.request.messages[0] {
        ModelInputMessage::Canonical(message) => message.clone(),
        ModelInputMessage::Derived(_) | ModelInputMessage::RequestOnly(_) => unreachable!(),
    };
    fixture
        .store
        .initialize(std::slice::from_ref(&message))
        .unwrap();
    fixture
        .store
        .append_event(RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new(uuid::Uuid::now_v7().to_string()),
            sequence: 0,
            conversation_id: fixture.store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            tool_occurrence: None,
            observation: None,
            timestamp: chrono::Utc::now(),
            event: RuntimeEvent::CompactionStarted {
                operation_id: fixture.scope.budget.operation_id().clone(),
            },
        })
        .unwrap();
    let cut = fixture.store.compaction_resource_cut().unwrap();
    let before = fixture.store.auxiliary_audit_cut().unwrap();
    (
        SummaryRequest {
            retired: vec![ModelInputMessage::Canonical(message)],
        },
        cut,
        before,
    )
}

#[tokio::test(flavor = "current_thread")]
async fn historical_summary_call_start_cancellation_preserves_native_health() {
    use crate::context::summarizer::SummaryBackend;
    use crate::durable::sqlite::AuxiliaryStartProbe;
    for probe in [
        AuxiliaryStartProbe::HistoricalCut,
        AuxiliaryStartProbe::CompactionFrontier,
    ] {
        let fixture = fixture(Script::Complete);
        let (input, cut, before) = historical_summary_input(&fixture);
        drop(fixture.reservation);
        let signal = CancellationSignal::new();
        let operation = fixture
            .executor
            .operation(
                fixture.frozen.invocation,
                ModelTimeoutPolicy::default(),
                7,
                fixture.scope.budget.clone(),
                signal.clone(),
            )
            .unwrap();
        let prepared = operation.prepare().unwrap();
        let record = lock(&fixture.executor.0.state)
            .calls
            .values()
            .next()
            .unwrap()
            .clone();
        let (entered, release) = SqliteConversationStore::hold_auxiliary_start_probe_for_test(
            record.call_id.to_string(),
            probe,
        );
        // Exercise the production Summary adapter, Historical input, and native
        // Compaction attribution, rather than rewriting a Temporary FrozenCall.
        let reply = tokio::spawn(prepared.summarize(input, Some(cut)));
        tokio::time::timeout(std::time::Duration::from_secs(10), entered)
            .await
            .unwrap()
            .unwrap();
        let cancelled = lock(&record.writer)
            .as_ref()
            .unwrap()
            .observe_start_cancellation_for_test();
        signal.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(10), cancelled)
            .await
            .unwrap()
            .unwrap();
        drop(release);
        let error = tokio::time::timeout(std::time::Duration::from_secs(10), reply)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error.kind,
            ContextErrorKind::Cancelled,
            "Historical probe {probe:?} retains interruption identity"
        );
        assert_eq!(fixture.adapter.entries.load(Ordering::SeqCst), 0);
        let after = fixture.store.auxiliary_audit_cut().unwrap();
        assert_eq!(
            after, before,
            "CallStart rollback cannot fabricate start or settlement facts"
        );
        assert!(
            fixture
                .store
                .load_auxiliary_call(&record.call_id, after)
                .unwrap()
                .is_none()
        );
        assert!(fixture.executor.0.resources.durability.failure().is_none());
        let fresh = CancellationSignal::new();
        let next = AuxiliaryExecutor::scope(
            CompactionBudget::new(
                CompactionOperationId::new(uuid::Uuid::now_v7().to_string()),
                fixture.clock.clone(),
            ),
            &fresh,
        );
        drop(
            fixture
                .executor
                .reserve_general(&next)
                .expect("later active admission remains healthy"),
        );
        operation.close().await;
        fixture.executor.drain().await;
    }
}
