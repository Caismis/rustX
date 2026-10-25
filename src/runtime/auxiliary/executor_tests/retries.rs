//! Deterministic production retries, including actual SQLite settlement gates.
use super::*;
use crate::events::types::RuntimeEvent;
use crate::model::{ModelError, ModelRetryDisposition};

struct RetryAdapter {
    failures: Vec<ModelError>,
    entries: tokio::sync::mpsc::UnboundedSender<usize>,
    release: Arc<Notify>,
    requests: Mutex<Vec<ModelRequest>>,
    exited: Arc<AtomicBool>,
    exit_advance: Mutex<Option<(Arc<ManualMonotonicClock>, u64)>>,
}

impl ModelAdapter for RetryAdapter {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }
    fn stream(&self, request: ModelRequest, _: CancellationSignal) -> ModelStream {
        assert_eq!(
            crate::model::selected_input::take_materialized_items(),
            0,
            "retries never materialize historical input values"
        );
        assert_eq!(
            crate::model::selected_input::take_node_work(),
            (0, 0, 0, 0),
            "retries retain selected input roots"
        );
        let media_work = crate::model::media::take_work();
        assert_eq!(
            (
                media_work.reads,
                media_work.resolutions,
                media_work.copied_items
            ),
            (0, 0, 0),
            "executor never recaptures or reprojects media"
        );
        assert!(
            self.exited.swap(false, Ordering::SeqCst),
            "preceding locally owned stream must exit before another dispatch"
        );
        let ordinal = {
            let mut requests = self.requests.lock().unwrap();
            if let Some(first) = requests.first() {
                assert_eq!(
                    request, *first,
                    "all semantic input, invocation and captured media remain frozen"
                );
            }
            let ordinal = requests.len();
            requests.push(request);
            ordinal
        };
        self.entries.send(ordinal).unwrap();
        let release = self.release.clone();
        let failure = self.failures.get(ordinal).cloned();
        let prefix = futures_util::stream::iter([
            event(ModelEvent::Started),
            event(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: format!("attempt-{ordinal}"),
            }),
            event(ModelEvent::UsageUpdate {
                usage: usage(ordinal as u64 + 1),
            }),
        ]);
        let terminal = futures_util::stream::once(async move {
            release.notified().await;
            event(failure.map_or(
                ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: Some(usage(9)),
                },
                |error| ModelEvent::Failed { error },
            ))
        });
        Box::pin(RetryOwnedStream {
            inner: TrackedStream {
                inner: Box::pin(prefix.chain(terminal)),
                exited: self.exited.clone(),
            },
            advance: self.exit_advance.lock().unwrap().take(),
        })
    }
}

struct RetryOwnedStream {
    inner: TrackedStream,
    advance: Option<(Arc<ManualMonotonicClock>, u64)>,
}
impl futures_util::Stream for RetryOwnedStream {
    type Item = ModelStreamItem;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_next(cx)
    }
}
impl Drop for RetryOwnedStream {
    fn drop(&mut self) {
        if let Some((clock, elapsed)) = &self.advance {
            clock.advance(*elapsed);
        }
    }
}

fn transient() -> ModelError {
    ModelError {
        kind: ModelErrorKind::Transport,
        message: "connection reset".into(),
        retry_disposition: ModelRetryDisposition::Transient,
        retry_after_ms: None,
        provider_code: None,
        context_overflow: None,
        malformed_tool_proposal: None,
        timeout_phase: None,
        generation: None,
    }
}

fn retry_fixture(
    failures: Vec<ModelError>,
) -> (
    Fixture,
    Arc<RetryAdapter>,
    tokio::sync::mpsc::UnboundedReceiver<usize>,
) {
    let mut fixture = fixture(Script::Complete);
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let adapter = Arc::new(RetryAdapter {
        failures,
        entries: sender,
        release: Arc::new(Notify::new()),
        requests: Mutex::default(),
        exited: Arc::new(AtomicBool::new(true)),
        exit_advance: Mutex::default(),
    });
    let invocation = crate::scripted_suites::support::attempt_model_with_window(
        adapter.clone(),
        "retry-consumer",
        32768,
        128,
    )
    .summary_invocation()
    .clone();
    fixture.frozen.request.invocation = invocation.invocation_config();
    fixture.frozen.start.semantic_input_fingerprint =
        AuxiliaryFingerprint::new(fixture.frozen.request.semantic_input_fingerprint()).unwrap();
    fixture.frozen.start.invocation_fingerprint =
        invocation_fingerprint(&fixture.frozen.request.invocation, 7).unwrap();
    fixture.frozen.invocation = invocation;
    crate::model::selected_input::take_materialized_items();
    crate::model::selected_input::take_node_work();
    crate::model::media::take_work();
    (fixture, adapter, receiver)
}

async fn next_entry(receiver: &mut tokio::sync::mpsc::UnboundedReceiver<usize>) -> usize {
    tokio::time::timeout(std::time::Duration::from_secs(10), receiver.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn transient_retry_confirms_failure_before_fresh_start_and_counts_settlement_latency() {
    let (fixture, adapter, mut entries) = retry_fixture(vec![transient()]);
    *adapter.exit_advance.lock().unwrap() = Some((fixture.clock.clone(), 1_000));
    let call_id = fixture.reservation.call_id().clone();
    let reply = fixture.reservation.submit(fixture.frozen).unwrap();
    assert_eq!(next_entry(&mut entries).await, 0);
    let gates = fixture.executor.hold_settlement_for_test();
    adapter.release.notify_one();
    gates.committed.await.unwrap();
    assert!(adapter.exited.load(Ordering::SeqCst));
    assert!(entries.try_recv().is_err());
    let cut = fixture.store.auxiliary_audit_cut().unwrap();
    let requests = fixture
        .store
        .read_auxiliary_call_requests(&call_id, cut, None, 32)
        .unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].terminal_sequence.is_some());
    assert!(
        requests[0]
            .request_evidence
            .as_ref()
            .unwrap()
            .remote_execution_uncertainty
    );
    assert_eq!(
        fixture.clock.now_millis(),
        1_000,
        "local stream teardown consumed the first second of backoff"
    );
    fixture.clock.advance(1_500);
    // Lose the real committed acknowledgement, then hold exact confirmation.
    drop(gates.release_ack);
    gates.confirming.await.unwrap();
    assert!(
        entries.try_recv().is_err(),
        "unconfirmed local settlement cannot dispatch"
    );
    gates.release_confirmation.send(()).unwrap();
    assert_eq!(
        next_entry(&mut entries).await,
        1,
        "elapsed persistence delay must not be slept again"
    );
    adapter.release.notify_one();
    let completed = reply.await.unwrap().unwrap();
    assert!(completed.usage.remote_execution_uncertainty);
    assert!(completed.usage.incomplete);
    assert_eq!(completed.usage.output_tokens, Some(10));
    assert_eq!(completed.usage.reasoning_tokens, None);
    assert_eq!(
        completed.output.unwrap().into_text().unwrap().text,
        "attempt-1",
        "failed provisional text was discarded"
    );
    let cut = fixture.store.auxiliary_audit_cut().unwrap();
    let requests = fixture
        .store
        .read_auxiliary_call_requests(&call_id, cut, None, 32)
        .unwrap();
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0].request_id, requests[1].request_id);
    assert_eq!(requests[0].usage, Some(usage(1)));
    assert_eq!(requests[1].usage, Some(usage(9)));
    assert_eq!(fixture.budget.authority().counts(), (2, 0));
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_after_retry_scheduled_is_call_only_and_retains_permits_through_confirmation()
{
    let (fixture, adapter, mut entries) = retry_fixture(vec![transient()]);
    let call_id = fixture.reservation.call_id().clone();
    crate::model::selected_input::take_materialized_items();
    crate::model::selected_input::take_node_work();
    crate::model::media::take_work();
    let reply = fixture.reservation.submit(fixture.frozen).unwrap();
    next_entry(&mut entries).await;
    let gates = fixture.executor.hold_settlement_for_test();
    adapter.release.notify_one();
    gates.committed.await.unwrap();
    let other = fixture
        .executor
        .general_scope(3, &CancellationSignal::new());
    let one = fixture.executor.reserve_general(&other).unwrap();
    let two = fixture.executor.reserve_general(&other).unwrap();
    assert!(
        fixture.executor.reserve_general(&other).is_err(),
        "backoff retains the Call permit"
    );
    fixture.scope.close();
    drop(gates.release_ack);
    gates.confirming.await.unwrap();
    assert!(entries.try_recv().is_err());
    gates.release_confirmation.send(()).unwrap();
    assert_eq!(
        reply.await.unwrap().unwrap_err().kind,
        ContextErrorKind::Cancelled
    );
    drop((one, two));
    let cut = fixture.store.auxiliary_audit_cut().unwrap();
    let requests = fixture
        .store
        .read_auxiliary_call_requests(&call_id, cut, None, 32)
        .unwrap();
    assert_eq!(requests.len(), 1);
    let call = fixture
        .store
        .load_auxiliary_call(&call_id, cut)
        .unwrap()
        .unwrap();
    assert_eq!(call.outcome, Some(AuxiliaryCallOutcome::Cancelled));
    let events = fixture.store.read_events(None, 32).unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, RuntimeEvent::ModelRequestFailed { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, RuntimeEvent::AuxiliaryRetryScheduled { .. }))
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn retries_exhaust_at_three_fresh_physical_requests() {
    let (fixture, adapter, mut entries) = retry_fixture(vec![transient(); 3]);
    let call_id = fixture.reservation.call_id().clone();
    let reply = fixture.reservation.submit(fixture.frozen).unwrap();
    for ordinal in 0..3 {
        assert_eq!(next_entry(&mut entries).await, ordinal);
        if ordinal < 2 {
            let gates = fixture.executor.hold_settlement_for_test();
            adapter.release.notify_one();
            gates.committed.await.unwrap();
            fixture.clock.advance(2_000 << ordinal);
            drop(gates.release_ack);
            gates.confirming.await.unwrap();
            gates.release_confirmation.send(()).unwrap();
        } else {
            adapter.release.notify_one();
        }
    }
    assert!(reply.await.unwrap().unwrap().output.is_err());
    let cut = fixture.store.auxiliary_audit_cut().unwrap();
    assert_eq!(
        fixture
            .store
            .read_auxiliary_call_requests(&call_id, cut, None, 32)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(fixture.budget.authority().counts(), (3, 0));
}

#[tokio::test(flavor = "current_thread")]
async fn retry_after_above_sixty_seconds_is_declined_without_clamping() {
    let mut failure = transient();
    failure.retry_after_ms = Some(60_001);
    let (fixture, adapter, mut entries) = retry_fixture(vec![failure]);
    crate::model::selected_input::take_materialized_items();
    crate::model::selected_input::take_node_work();
    crate::model::media::take_work();
    let reply = fixture.reservation.submit(fixture.frozen).unwrap();
    next_entry(&mut entries).await;
    adapter.release.notify_one();
    assert!(reply.await.unwrap().unwrap().output.is_err());
    assert_eq!(adapter.requests.lock().unwrap().len(), 1);
    assert_eq!(fixture.clock.now_millis(), 0);
}

fn business_child(
    executor: &AuxiliaryExecutor,
    scope: &Arc<AuxiliaryScope>,
    store: &Arc<SqliteConversationStore>,
    template: &ModelRequest,
    semantic_input: &str,
    script: Script,
) -> (AuxiliaryReservation, FrozenCall, Arc<ConsumerAdapter>) {
    let reservation = executor.reserve_general(scope).unwrap();
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
        "business-child",
        32768,
        128,
    )
    .summary_invocation()
    .clone();
    let mut request = template.clone();
    request.invocation = invocation.invocation_config();
    request.effective_system_prompt = semantic_input.into();
    let start = AuxiliaryCallStart {
        call_id: reservation.call_id().clone(),
        purpose: AuxiliaryAttribution::new("translation").unwrap(),
        trigger: AuxiliaryAttribution::new("synthetic_business_step").unwrap(),
        business_source: AuxiliaryBusinessSource::Business {
            namespace: AuxiliaryAttribution::new("bounded-fixture").unwrap(),
            reference: AuxiliaryAttribution::new("one-native-owner").unwrap(),
        },
        input: AuxiliaryInputEvidence::Temporary,
        selected_message_ids: Vec::new(),
        semantic_input_fingerprint: AuxiliaryFingerprint::new(request.semantic_input_fingerprint())
            .unwrap(),
        invocation_fingerprint: invocation_fingerprint(&request.invocation, 7).unwrap(),
    };
    (
        reservation,
        FrozenCall {
            start,
            request,
            invocation,
            timeout: ModelTimeoutPolicy::default(),
            resource_revision: 7,
            dispatch_check: None,
            title_source: None,
        },
        adapter,
    )
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_classification_repair_is_a_new_call_under_original_parent_allowance() {
    let Fixture {
        executor,
        store,
        clock,
        frozen,
        reservation,
        ..
    } = fixture(Script::Complete);
    drop(reservation);
    let scope = executor.general_scope(2, &CancellationSignal::new());
    let original_deadline = scope.authority.deadline_millis();
    let (first, frozen_first, _) = business_child(
        &executor,
        &scope,
        &store,
        &frozen.request,
        "Classify into a numeric category",
        Script::Complete,
    );
    let first_id = first.call_id().clone();
    let first_result = first.submit(frozen_first).unwrap().await.unwrap().unwrap();
    assert!(
        first_result
            .output
            .unwrap()
            .into_text()
            .unwrap()
            .text
            .parse::<u8>()
            .is_err(),
        "consumer rejects valid execution without a technical retry"
    );
    clock.advance(1_000);
    let (second, mut frozen_second, _) = business_child(
        &executor,
        &scope,
        &store,
        &frozen.request,
        "Corrective classification input: choose a supported textual category",
        Script::Complete,
    );
    frozen_second.start.business_source = AuxiliaryBusinessSource::Business {
        namespace: AuxiliaryAttribution::new("classification-repair").unwrap(),
        reference: AuxiliaryAttribution::new(first_id.to_string()).unwrap(),
    };
    let second_id = second.call_id().clone();
    assert_ne!(first_id, second_id);
    assert_eq!(
        second
            .submit(frozen_second)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .output
            .unwrap()
            .into_text()
            .unwrap()
            .text,
        "traduit"
    );
    assert_eq!(scope.authority.deadline_millis(), original_deadline);
    assert_eq!(scope.authority.counts(), (2, 0));
    assert!(executor.reserve_general(&scope).is_err());
    let cut = store.auxiliary_audit_cut().unwrap();
    for id in [first_id, second_id] {
        let requests = store
            .read_auxiliary_call_requests(&id, cut, None, 32)
            .unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].ordinal, 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_fanout_uses_ordinary_children_and_scope_close_settles_existing_calls() {
    let Fixture {
        executor,
        store,
        frozen,
        reservation,
        ..
    } = fixture(Script::Complete);
    drop(reservation);
    let scope = executor.general_scope(3, &CancellationSignal::new());
    let (candidate, first, first_adapter) = business_child(
        &executor,
        &scope,
        &store,
        &frozen.request,
        "Generate candidate A",
        Script::CompleteAfterRelease,
    );
    let (comparison, second, second_adapter) = business_child(
        &executor,
        &scope,
        &store,
        &frozen.request,
        "Evaluate candidate B independently",
        Script::CompleteAfterRelease,
    );
    let remaining = executor.reserve_general(&scope).unwrap();
    assert!(
        executor.reserve_general(&scope).is_err(),
        "fanout never queues or borrows protected Compaction capacity"
    );
    drop(remaining);
    let candidate = candidate.submit(first).unwrap();
    let comparison = comparison.submit(second).unwrap();
    first_adapter.waiting.notified().await;
    second_adapter.waiting.notified().await;
    scope.close();
    assert!(executor.reserve_general(&scope).is_err());
    for reply in [candidate, comparison] {
        let completed = reply.await.unwrap().unwrap();
        assert_eq!(
            completed.output.unwrap_err().kind,
            ModelErrorKind::Cancelled
        );
    }
    scope.wait_closed().await;
    assert_eq!(scope.authority.counts(), (2, 0));
    assert!(lock(&scope.state).children.is_empty());
    assert!(first_adapter.stream_exited.load(Ordering::SeqCst));
    assert!(second_adapter.stream_exited.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn bounded_candidate_evaluation_and_selection_share_three_starts_and_causal_inputs() {
    let Fixture {
        executor,
        store,
        frozen,
        reservation,
        ..
    } = fixture(Script::Complete);
    drop(reservation);
    let scope = executor.general_scope(3, &CancellationSignal::new());
    let mut preceding = String::from("Generate a bounded candidate");
    let mut ids = Vec::new();
    for stage in ["candidate", "evaluation", "selection"] {
        let (child, mut input, _) = business_child(
            &executor,
            &scope,
            &store,
            &frozen.request,
            &format!("{stage}: {preceding}"),
            Script::Complete,
        );
        if let Some(parent) = ids.last() {
            input.start.business_source = AuxiliaryBusinessSource::Business {
                namespace: AuxiliaryAttribution::new("candidate-selection").unwrap(),
                reference: AuxiliaryAttribution::new(format!("{parent}")).unwrap(),
            };
        }
        ids.push(child.call_id().clone());
        let result = child.submit(input).unwrap().await.unwrap().unwrap();
        assert_eq!(result.usage.output_tokens, Some(7 * ids.len() as u64));
        preceding = result.output.unwrap().into_text().unwrap().text;
    }
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 3);
    assert_eq!(scope.authority.counts(), (3, 0));
    assert!(executor.reserve_general(&scope).is_err());
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::too_many_lines)] // One capture/mutation/retry sequence preserves its exact resource witness.
async fn retry_keeps_selected_occurrences_and_captured_media_after_source_mutation() {
    use crate::model::media::MediaSnapshot;
    use crate::scripted_suites::support::model::{
        FixtureModel, ScriptedAdapterFactory, fixture_session_model,
    };
    use crate::tools::artifacts::ArtifactStore;
    let (mut fixture, adapter, mut entries) = retry_fixture(vec![transient()]);
    let root = tempfile::tempdir().unwrap();
    let artifacts =
        ArtifactStore::new(fixture.store.conversation_id().clone(), root.path()).unwrap();
    let mut bytes = Vec::new();
    png::Encoder::new(&mut bytes, 1, 1)
        .write_header()
        .unwrap()
        .write_image_data(&[0])
        .unwrap();
    let image_id = artifacts.put_bounded(&bytes).unwrap();
    let invocation = fixture_session_model(
        &[
            FixtureModel::text("fixture/media-retry", ModelProtocol::OpenAiChatCompletions)
                .claiming_input("image"),
        ],
        "fixture/media-retry",
        &ScriptedAdapterFactory::new(adapter.clone()),
    )
    .snapshot()
    .unwrap()
    .summary_invocation()
    .clone();
    fixture.frozen.invocation = invocation.clone();
    fixture.frozen.request.invocation = invocation.invocation_config();
    let selected = ModelInputMessage::Canonical(MessageBlock::User(UserMessageBlock {
        id: MessageId::new("selected-media"),
        content: vec![
            UserContentBlock::Text(TextBlock {
                text: "selected occurrence".into(),
            }),
            UserContentBlock::Image(crate::message::content::ImageReference {
                artifact_id: image_id.clone(),
                alt: None,
            }),
        ],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    }));
    let mut history = fixture.frozen.request.messages.clone();
    history.push(selected.clone());
    history.push(selected);
    history.push(fixture.frozen.request.messages[0].clone());
    fixture.frozen.request.messages = history.range(1..3);
    let media = MediaSnapshot::capture_selected(
        None,
        &history,
        &fixture.frozen.request.messages,
        Some(&artifacts),
        Some(true),
        None,
        &CancellationSignal::new(),
    )
    .unwrap();
    fixture.frozen.request.images = media
        .selected_images_input(&fixture.frozen.request.messages)
        .unwrap();
    fixture.frozen.start.semantic_input_fingerprint =
        AuxiliaryFingerprint::new(fixture.frozen.request.semantic_input_fingerprint()).unwrap();
    fixture.frozen.start.invocation_fingerprint =
        invocation_fingerprint(&fixture.frozen.request.invocation, 7).unwrap();
    crate::model::selected_input::take_materialized_items();
    crate::model::selected_input::take_node_work();
    crate::model::media::take_work();
    let reply = fixture.reservation.submit(fixture.frozen).unwrap();
    next_entry(&mut entries).await;
    let gates = fixture.executor.hold_settlement_for_test();
    std::fs::write(
        artifacts.root().join(format!("{}.bin", image_id.as_str())),
        b"changed source bytes",
    )
    .unwrap();
    assert!(
        MediaSnapshot::capture_selected(
            None,
            &history,
            &history,
            Some(&artifacts),
            Some(true),
            None,
            &CancellationSignal::new()
        )
        .is_err(),
        "the changed source cannot authorize a fresh capture"
    );
    drop(media);
    drop(artifacts);
    crate::model::selected_input::take_materialized_items();
    crate::model::selected_input::take_node_work();
    crate::model::media::take_work();
    adapter.release.notify_one();
    gates.committed.await.unwrap();
    fixture.clock.advance(2_000);
    drop(gates.release_ack);
    gates.confirming.await.unwrap();
    gates.release_confirmation.send(()).unwrap();
    next_entry(&mut entries).await;
    adapter.release.notify_one();
    assert!(reply.await.unwrap().unwrap().output.is_ok());
    let requests = adapter.requests.lock().unwrap();
    assert_eq!(requests[1].images[&image_id], bytes);
    assert_eq!(
        requests[1].messages.len(),
        2,
        "ordered duplicate occurrences remain selected"
    );
    assert_eq!(requests[1].messages[0], requests[1].messages[1]);
}
