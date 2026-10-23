// Current-attempt controls select native execution authority, never a display alias.
use super::*;
use crate::durable::JournalStore;
use crate::runtime::conversation_runtime::{
    CancelAttemptError, NativeAttemptOutcome, NativeAttemptSettlement,
};
use crate::runtime_client::attachment::RuntimeAttachment;
use crate::runtime_client::types::{RequestId, RuntimeClientResponse};

struct EventWitnessFault(rusqlite::Connection);
impl EventWitnessFault {
    fn new(fixture: &HostFixture) -> Self {
        let connection =
            rusqlite::Connection::open(fixture.directory.path().join("conversation.sqlite"))
                .unwrap();
        connection.execute_batch(
            "CREATE TRIGGER cancellation_witness_fault BEFORE UPDATE OF observation_witness ON events BEGIN SELECT RAISE(FAIL,'optional cancellation witness'); END;",
        ).unwrap();
        Self(connection)
    }
}
impl Drop for EventWitnessFault {
    fn drop(&mut self) {
        self.0
            .execute_batch("DROP TRIGGER cancellation_witness_fault")
            .unwrap();
    }
}

struct HeldPreparation {
    release: std::sync::mpsc::Sender<()>,
    task: Option<std::thread::JoinHandle<()>>,
}
impl HeldPreparation {
    fn new(inner: Arc<super::super::ClientInner>) -> Self {
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || {
            let _preparation = inner.observation_preparation.lock().unwrap();
            entered.send(()).unwrap();
            let _ = release_rx.recv();
        });
        entered_rx.recv_timeout(BACKGROUND_LIVENESS_GUARD).unwrap();
        Self {
            release,
            task: Some(task),
        }
    }
}
impl Drop for HeldPreparation {
    fn drop(&mut self) {
        let _ = self.release.send(());
        self.task.take().unwrap().join().unwrap();
    }
}

struct ReleaseRead(std::sync::mpsc::Sender<()>);
impl Drop for ReleaseRead {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

fn attach(fixture: &HostFixture) -> Arc<RuntimeAttachment> {
    Arc::new(
        fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap()
            .0,
    )
}

fn parked_script(release: watch::Receiver<bool>) -> Vec<GatedStep> {
    vec![
        GatedStep::Emit(ModelEvent::Started),
        GatedStep::ParkUntilReleased(release),
        GatedStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]
}

async fn settle(runtime: &ConversationRuntime, after: u64) -> NativeAttemptSettlement {
    tokio::time::timeout(
        BACKGROUND_LIVENESS_GUARD,
        runtime.child_attempt_settlement(after),
    )
    .await
    .expect("the original native attempt settles")
}

async fn cancel(attachment: Arc<RuntimeAttachment>, id: u64) -> RuntimeClientResponse {
    tokio::time::timeout(
        BACKGROUND_LIVENESS_GUARD,
        tokio::task::spawn_blocking(move || {
            attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
                id: RequestId::new(id),
            })
        }),
    )
    .await
    .expect("native cancellation does not wait for observation preparation")
    .unwrap()
}

fn accepted(response: RuntimeClientResponse, id: u64) -> AttemptId {
    assert_eq!(response.id, RequestId::new(id));
    assert!(response.error.is_none(), "{response:?}");
    let Some(RuntimeClientResult::AttemptCancellationAccepted { attempt_id }) = response.result
    else {
        panic!("native cancellation acceptance");
    };
    attempt_id
}

fn assert_one_terminal(fixture: &HostFixture, attempt: &AttemptId, cancelled: bool) {
    let events = fixture
        .store
        .as_ref()
        .unwrap()
        .read_events(None, 256)
        .unwrap()
        .events;
    let terminals: Vec<_> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            RuntimeEvent::AttemptCancelled { attempt_id, .. } if attempt_id == attempt => {
                Some(true)
            }
            RuntimeEvent::AttemptCompleted { attempt_id, .. } if attempt_id == attempt => {
                Some(false)
            }
            RuntimeEvent::AttemptFailed { attempt_id, .. }
            | RuntimeEvent::AttemptTimedOut { attempt_id }
            | RuntimeEvent::AttemptLimitExceeded { attempt_id, .. }
                if attempt_id == attempt =>
            {
                panic!("unexpected native terminal: {:?}", envelope.event);
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        terminals,
        vec![cancelled],
        "one exact original native terminal"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_selects_native_successor_while_real_source_fault_retains_settled_display() {
    let (_release, release_rx) = model_release();
    let (model, fixture) = host_fixture(
        vec![one_turn_stop(), parked_script(release_rx)],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let attachment = attach(&fixture);
    attachment
        .submit_inbound(submit_content("complete A"))
        .unwrap();
    let a = settle(&fixture.runtime, 0).await;
    assert!(matches!(a.outcome, NativeAttemptOutcome::Completed));
    fixture.host.projection_settled().await;
    let published_a = fixture.host.checkpoint().unwrap();
    assert_eq!(
        published_a
            .snapshot
            .attempt
            .as_ref()
            .unwrap()
            .attempt_id
            .as_ref(),
        Some(&a.attempt_id)
    );
    assert!(matches!(
        published_a.snapshot.attempt.as_ref().unwrap().phase,
        RuntimeClientAttemptPhase::Settled { .. }
    ));

    // Only the optional derived witness fails. B's accepted input, request,
    // execution and eventual terminal still use their original native owners.
    let fault = EventWitnessFault::new(&fixture);
    attachment.submit_inbound(submit_content("run B")).unwrap();
    await_adapter_request_count(&model, 2).await;
    let b = request_snapshots(&fixture.host.request_history())[1]
        .identity
        .attempt_id
        .clone();
    assert_ne!(a.attempt_id, b);
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let (retained_cursor, retained_attempt) = {
        let published = inner.lock_published_state();
        (
            published.projection.cursor(),
            published.projection.snapshot_ref().attempt.clone(),
        )
    };
    // An accepted-input transition may advance the cursor independently of
    // Attempt publication. The deciding evidence is the original settled A.
    assert!(retained_cursor >= published_a.cursor);
    assert_eq!(retained_attempt, published_a.snapshot.attempt);
    // A control must not drain the observation queue to discover B either.
    let preparation = HeldPreparation::new(inner.clone());
    assert_eq!(accepted(cancel(attachment.clone(), 41).await, 41), b);
    drop(preparation);
    let terminal = settle(&fixture.runtime, a.ordinal).await;
    assert_eq!(terminal.attempt_id, b);
    assert!(matches!(terminal.outcome, NativeAttemptOutcome::Cancelled));
    assert_eq!(model.requests().len(), 2);
    assert_one_terminal(&fixture, &a.attempt_id, false);
    assert_one_terminal(&fixture, &b, true);
    drop(fault);
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn held_cancellation_receipt_keeps_selected_identity_after_native_successor_admission() {
    let (_release_a, release_a_rx) = model_release();
    let (release_b, release_b_rx) = model_release();
    let (model, fixture) = host_fixture(
        vec![parked_script(release_a_rx), parked_script(release_b_rx)],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let attachment = attach(&fixture);
    attachment
        .submit_inbound(submit_content("selected A"))
        .unwrap();
    await_adapter_request_count(&model, 1).await;
    let a = request_snapshots(&fixture.host.request_history())[0]
        .identity
        .attempt_id
        .clone();
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let release_on_drop = ReleaseRead(release);
    *inner.cancellation_receipt_hook.lock().unwrap() = Some(Box::new(move || {
        entered.send(()).unwrap();
        let _ = release_rx.recv();
    }));
    let cancel_attachment = attachment.clone();
    let pending = tokio::task::spawn_blocking(move || {
        cancel_attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
            id: RequestId::new(51),
        })
    });
    entered_rx.await.unwrap(); // Native selection, Goal pause and signal already won.
    let terminal_a = settle(&fixture.runtime, 0).await;
    assert_eq!(terminal_a.attempt_id, a);
    assert!(matches!(
        terminal_a.outcome,
        NativeAttemptOutcome::Cancelled
    ));
    attachment
        .submit_inbound(submit_content("successor B"))
        .unwrap();
    await_adapter_request_count(&model, 2).await;
    let b = request_snapshots(&fixture.host.request_history())[1]
        .identity
        .attempt_id
        .clone();
    assert_ne!(a, b);
    assert!(!pending.is_finished(), "receipt remains physically held");
    drop(release_on_drop);
    assert_eq!(accepted(pending.await.unwrap(), 51), a);
    // Reusing the old exact native target is refused, never transferred to B.
    assert_eq!(
        fixture.runtime.cancel_current_attempt(&a),
        Err(CancelAttemptError::NoCurrentAttempt)
    );
    release_b.send(true).unwrap();
    let terminal_b = settle(&fixture.runtime, terminal_a.ordinal).await;
    assert_eq!(terminal_b.attempt_id, b);
    assert!(matches!(
        terminal_b.outcome,
        NativeAttemptOutcome::Completed
    ));
    assert_eq!(model.requests().len(), 2);
    assert_one_terminal(&fixture, &a, true);
    assert_one_terminal(&fixture, &b, false);
    assert!(matches!(
        cancel(attachment, 52).await.error,
        Some(RuntimeClientError::NoCurrentAttempt)
    ));
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_cancellation_remains_available_during_real_observation_source_failure() {
    let (_release, release_rx) = model_release();
    let (start, start_rx) = model_release();
    let (parked, parked_rx) = tokio::sync::oneshot::channel();
    let (model, fixture) = host_fixture(
        vec![vec![
            GatedStep::ParkUntilReleased(start_rx),
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::Signal(parked),
            GatedStep::ParkUntilReleased(release_rx),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let attachment = attach(&fixture);
    attachment
        .submit_inbound(submit_content("source failure control"))
        .unwrap();
    await_adapter_request_count(&model, 1).await;
    fixture.host.projection_settled().await;
    let before_started = fixture.host.checkpoint().unwrap();

    // Request arrival precedes stream consumption. Force Started to arrive
    // after that settled cut, then fence it before choosing the fault baseline.
    start.send(true).unwrap();
    tokio::time::timeout(BACKGROUND_LIVENESS_GUARD, parked_rx)
        .await
        .expect("the provider reaches its post-Started park")
        .unwrap();
    fixture.host.projection_settled().await;
    let expected = request_snapshots(&fixture.host.request_history())[0]
        .identity
        .attempt_id
        .clone();
    let before = fixture.host.checkpoint().unwrap();
    assert!(
        before.cursor > before_started.cursor,
        "Started publishes after the request-arrival baseline"
    );
    let subscription = attachment.subscribe_events(before.cursor).unwrap();
    let mut fault = super::source_recovery_tests::OccupancySourceFault::new(&fixture);
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    inner
        .lock_published_state()
        .projection
        .request_read_domains_for_test();
    inner
        .pending
        .push(ConversationObservation::JournalSourceReady);
    let delivery = subscription.next().await;
    assert!(
        matches!(delivery, EventDelivery::Unavailable { .. }),
        "{delivery:?}"
    );
    let preparation = HeldPreparation::new(inner.clone());
    assert_eq!(accepted(cancel(attachment, 61).await, 61), expected);
    let observation_unavailable = {
        let state = inner.lock_published_state();
        matches!(
            state.projection.snapshot_ref_checked(),
            Err(RuntimeClientError::ObservationUnavailable { .. })
        )
    };
    assert!(observation_unavailable);
    drop(preparation);
    let terminal = settle(&fixture.runtime, 0).await;
    assert_eq!(terminal.attempt_id, expected);
    assert!(matches!(terminal.outcome, NativeAttemptOutcome::Cancelled));
    assert_eq!(model.requests().len(), 1);
    assert_one_terminal(&fixture, &expected, true);
    fault.repair();
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_cancellation_does_not_join_or_cancel_the_blocked_physical_observation_read() {
    let (_release, release_rx) = model_release();
    let (model, fixture) = host_fixture(
        vec![parked_script(release_rx)],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let attachment = attach(&fixture);
    attachment
        .submit_inbound(submit_content("blocked derived read control"))
        .unwrap();
    await_adapter_request_count(&model, 1).await;
    fixture.host.projection_settled().await;
    let expected = request_snapshots(&fixture.host.request_history())[0]
        .identity
        .attempt_id
        .clone();
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (cancelled, mut cancelled_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let release_on_drop = ReleaseRead(release);
    fixture
        .host
        .gate_projection_read_for_test(entered, cancelled, release_rx);
    entered_rx.await.unwrap(); // The original physical read is actually blocked.
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let preparation = HeldPreparation::new(inner.clone());
    assert_eq!(accepted(cancel(attachment, 71).await, 71), expected);
    assert!(
        matches!(
            cancelled_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "control did not cancel an observer to make progress"
    );
    drop(preparation);
    let terminal = settle(&fixture.runtime, 0).await;
    assert_eq!(terminal.attempt_id, expected);
    assert!(matches!(terminal.outcome, NativeAttemptOutcome::Cancelled));
    assert_eq!(model.requests().len(), 1);
    assert_one_terminal(&fixture, &expected, true);
    inner.cancel_presentation_reads();
    cancelled_rx.await.unwrap();
    drop(release_on_drop);
    fixture.runtime.shutdown().await.unwrap();
    inner.drain_projection().await.unwrap();
}
