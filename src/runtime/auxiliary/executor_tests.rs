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
use crate::runtime::monotonic::SystemMonotonicClock;

#[derive(Clone, Copy)]
enum Script {
    Complete,
    PanicAfterUsage,
    AwaitCancellation,
}

struct ConsumerAdapter {
    store: Arc<SqliteConversationStore>,
    call: Mutex<Option<AuxiliaryCallId>>,
    entries: AtomicUsize,
    waiting: Arc<Notify>,
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
}

fn fixture(script: Script) -> Fixture {
    let store = Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
    let clock = Arc::new(SystemMonotonicClock::new());
    let executor = AuxiliaryExecutor::for_test(store.clone(), clock.clone()).unwrap();
    let scope = executor.scope(
        CompactionBudget::new(
            CompactionOperationId::new(uuid::Uuid::now_v7().to_string()),
            clock,
        ),
        &CancellationSignal::new(),
    );
    let reservation = executor.reserve_general(&scope).unwrap();
    let adapter = Arc::new(ConsumerAdapter {
        store: store.clone(),
        call: Mutex::new(Some(reservation.call_id().clone())),
        entries: AtomicUsize::new(0),
        waiting: Arc::new(Notify::new()),
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
        images: Default::default(),
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
    let reply = fixture.reservation.submit(fixture.frozen).unwrap();
    fixture.adapter.waiting.notified().await;
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
    let other_scope = fixture.executor.scope(
        CompactionBudget::new(
            CompactionOperationId::new(uuid::Uuid::now_v7().to_string()),
            fixture.executor.0.resources.monotonic.clone(),
        ),
        &CancellationSignal::new(),
    );
    drop(fixture.executor.reserve_general(&other_scope).unwrap());
    fixture.executor.drain().await;
}
