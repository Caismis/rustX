//! Defer cannot return a cancelled installer's result to Ready.
use super::*;

struct SelectedWork {
    _directory: tempfile::TempDir,
    runtime: ConversationRuntime,
    admission: CompactionAdmission,
    work: Arc<Work>,
}

async fn selected_work() -> SelectedWork {
    let directory = tempfile::tempdir().unwrap();
    let mut old = runtime_inbound_message("cancel-defer-old");
    old.content = text_content(&"old context ".repeat(512));
    let mut retained = runtime_inbound_message("cancel-defer-retained");
    retained.content = text_content(&"retained context ".repeat(512));
    let (runtime, _) = headless_runtime_with_options(
        &directory,
        vec![summary_turn_script("historical fact")],
        None,
        None,
        HeadlessRuntimeOptions {
            initial_messages: vec![MessageBlock::User(old), MessageBlock::User(retained)],
            ..HeadlessRuntimeOptions::default()
        },
    )
    .await;
    runtime.activate();
    // Keep idle installation out of the selection handoff owned by this test.
    hold_idle_installation(&mut runtime.inner.lock_state());
    let admission = start_background(&runtime).await;
    let work = runtime.inner.async_work(&admission).unwrap();
    within_liveness_guard("defer fixture Ready publication", async {
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = work.state.lock().unwrap();
                if matches!(state.phase, Phase::Ready(_))
                    && state.published.as_ref() == Some(&state.status)
                {
                    break;
                }
                assert!(!matches!(state.phase, Phase::Failed(_) | Phase::Terminal));
            }
            changed.await;
        }
    })
    .await;
    let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(&runtime.inner));
    assert!(matches!(
        coordinator.take_ready(),
        Some(CompactionDelivery::Ready(_))
    ));
    within_liveness_guard(
        "selected Summary worker join",
        runtime.inner.reap_async_compaction(true),
    )
    .await;
    assert!(matches!(
        work.state.lock().unwrap().phase,
        Phase::Installing(_)
    ));
    SelectedWork {
        _directory: directory,
        runtime,
        admission,
        work,
    }
}

pub(super) fn install_gate(
    slot: &Mutex<Option<ArbitrationTestGate>>,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (entered, arrival) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    *slot.lock().unwrap() = Some(ArbitrationTestGate {
        entered,
        release: released,
    });
    (arrival, release)
}

async fn assert_cancelled_and_retired(selected: &SelectedWork) {
    {
        let state = selected.work.state.lock().unwrap();
        assert!(matches!(state.phase, Phase::Terminal));
        assert_eq!(state.status.phase, CompactionOperationPhase::Cancelled);
        assert_eq!(state.published.as_ref(), Some(&state.status));
        assert!(state.status.waiting_attempt_id.is_none());
        assert!(state.status.diagnostic.is_none());
    }
    assert!(
        selected
            .runtime
            .inner
            .store
            .projection_head()
            .unwrap()
            .is_none()
    );
    // Return the test installer's foreground ownership only after its status
    // settles, just as a real installer returns the checked-out conversation.
    release_idle_installation(&mut selected.runtime.inner.lock_state());
    selected.runtime.inner.settlement.notify_waiters();
    within_liveness_guard(
        "cancelled installation shutdown",
        selected.runtime.shutdown(),
    )
    .await
    .unwrap();
    assert!(
        selected
            .runtime
            .inner
            .async_work(&selected.admission)
            .is_none()
    );
    let events = selected
        .runtime
        .inner
        .store
        .read_events(None, 256)
        .unwrap()
        .events;
    let statuses = events
        .iter()
        .filter_map(|event| match &event.event {
            RuntimeEvent::CompactionOperationUpdated { status }
                if status.operation_id == selected.admission.operation_id =>
            {
                Some(status.phase)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(statuses.last(), Some(&CompactionOperationPhase::Cancelled));
    assert!(
        !statuses
            .windows(2)
            .any(|pair| pair[0] == CompactionOperationPhase::Cancelled
                && pair[1] == CompactionOperationPhase::Ready)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_cancellation_wins_before_defer_state_transition() {
    let selected = selected_work().await;
    let (arrived, release) = install_gate(&selected.work.defer_settlement_gate);
    let owner = Arc::downgrade(&selected.runtime.inner);
    let admission = selected.admission.clone();
    let settlement = tokio::task::spawn_blocking(move || {
        RuntimeCompactionCoordinator(owner).settle(
            &admission,
            CompactionDisposition::Defer(Some("installer stopped".into())),
        )
    });
    within_liveness_guard("defer before state arbitration", arrived)
        .await
        .unwrap();
    selected.runtime.inner.begin_drain().unwrap();
    assert!(selected.admission.cancellation.is_cancelled());
    assert!(matches!(
        selected.work.state.lock().unwrap().phase,
        Phase::Installing(_)
    ));
    release.send(()).unwrap();
    within_liveness_guard("defer after drain", settlement)
        .await
        .unwrap()
        .unwrap();
    assert_cancelled_and_retired(&selected).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn named_cancellation_flag_wins_before_token_and_defer() {
    let selected = selected_work().await;
    let (arrived, release) = install_gate(&selected.work.cancellation_request_gate);
    let owner = selected.runtime.inner.clone();
    let operation_id = selected.admission.operation_id.clone();
    let cancellation =
        tokio::task::spawn_blocking(move || owner.cancel_named_async_compaction(&operation_id));
    within_liveness_guard("named cancellation flag before token", arrived)
        .await
        .unwrap();
    assert!(selected.work.state.lock().unwrap().cancel_requested);
    assert!(!selected.admission.cancellation.is_cancelled());
    let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(&selected.runtime.inner));
    coordinator
        .settle(&selected.admission, CompactionDisposition::Defer(None))
        .unwrap();
    assert!(matches!(
        selected.work.state.lock().unwrap().phase,
        Phase::Terminal
    ));
    release.send(()).unwrap();
    assert!(
        within_liveness_guard("named cancellation token publication", cancellation)
            .await
            .unwrap()
            .unwrap()
    );
    assert!(selected.admission.cancellation.is_cancelled());
    within_liveness_guard(
        "named cancellation ordinary reaping",
        selected.runtime.inner.reap_async_compaction(false),
    )
    .await;
    assert!(
        selected
            .runtime
            .inner
            .async_work(&selected.admission)
            .is_none()
    );
    assert_cancelled_and_retired(&selected).await;
}
