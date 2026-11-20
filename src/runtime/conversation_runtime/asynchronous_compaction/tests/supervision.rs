use super::*;
use crate::durable::{CanonicalStore, ProjectionStore, SqliteConversationStore};

async fn fixture(
    directory: &tempfile::TempDir,
    script: Vec<FakeStep>,
) -> (
    ConversationRuntime,
    Arc<FakeModel>,
    Arc<SqliteConversationStore>,
) {
    let id = ConversationId::generate();
    let store = Arc::new(SqliteConversationStore::in_memory(id.clone()).unwrap());
    store.initialize(&[]).unwrap();
    for name in ["first", "second"] {
        let mut message = runtime_inbound_message(name);
        message.content = text_content(&name.repeat(2048));
        store
            .append_canonical(&MessageBlock::User(message))
            .unwrap();
    }
    let (runtime, model) =
        headless_runtime_over_store_with(directory, id.as_str(), store.clone(), vec![script], None)
            .await;
    // Drive the reaper explicitly so no idle installer races the assertions.
    assert!(runtime.inner.lifecycle.activate());
    (runtime, model, store)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aborted_worker_publishes_failure_wakes_waiter_and_releases_admission() {
    let directory = tempfile::tempdir().unwrap();
    let (_release, gate) = model_release();
    let mut script = summary_turn_script("retained fact");
    script.insert(1, FakeStep::ParkUntilReleased(gate));
    let (runtime, model, _) = fixture(&directory, script).await;
    let admission = start_background(&runtime).await;
    let work = runtime.inner.async_work(&admission).unwrap();
    within_liveness_guard("parked worker", model.parks().wait_for(|count| *count >= 1))
        .await
        .unwrap();
    let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(&runtime.inner));
    let attempt = AttemptId::new("mandatory-waiter");
    let signal = CancellationSignal::new();
    let mut waiting = coordinator.wait(&admission, &attempt, &signal);
    assert!(futures_util::poll!(&mut waiting).is_pending());
    assert_eq!(
        work.state
            .lock()
            .unwrap()
            .status
            .waiting_attempt_id
            .as_ref(),
        Some(&attempt)
    );
    work.join.lock().unwrap().as_ref().unwrap().abort();
    runtime.inner.reap_async_compaction(true).await;
    let cut = runtime.inner.store.compaction_resource_cut().unwrap();
    let status = runtime
        .inner
        .store
        .latest_compaction_operations(cut.journal.0)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(status.phase, CompactionOperationPhase::Failed);
    assert!(status.waiting_attempt_id.is_none());
    assert!(
        matches!(within_liveness_guard("failure delivery", waiting).await, Some(CompactionDelivery::Failed(error)) if error.kind == ContextErrorKind::AuxiliarySupervisionFailed)
    );
    runtime.inner.reap_async_compaction(false).await;
    assert!(runtime.inner.async_work(&admission).is_none());
    assert!(coordinator.admit(&attempt).unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_primary_staging_returns_the_ready_result_to_its_owner() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, _, store) = fixture(&directory, summary_turn_script("retained fact")).await;
    let admission = start_background(&runtime).await;
    runtime.inner.reap_async_compaction(true).await;
    let work = runtime.inner.async_work(&admission).unwrap();
    let observed = Arc::new(AtomicBool::new(false));
    let failed = observed.clone();
    *store.fail_projected_history_when.lock().unwrap() = Some(Box::new(move || {
        let installing = matches!(work.state.lock().unwrap().phase, Phase::Installing(_));
        if installing {
            failed.store(true, Ordering::Release);
        }
        installing
    }));
    runtime.submit_inbound(text_content("next turn")).unwrap();
    runtime.inner.admit_next_attempt();
    within_liveness_guard(
        "failed staging settlement",
        runtime.child_attempt_settlement(0),
    )
    .await;
    assert!(observed.load(Ordering::Acquire));
    assert!(runtime.cancel_compaction(&admission.operation_id).unwrap());
    runtime.inner.reap_async_compaction(false).await;
    assert!(runtime.inner.async_work(&admission).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worker_finishing_after_named_cancellation_never_publishes_ready() {
    let directory = tempfile::tempdir().unwrap();
    let (release, gate) = model_release();
    let mut script = summary_turn_script("retained fact");
    script.insert(1, FakeStep::ParkUntilReleased(gate));
    let (runtime, model, _) = fixture(&directory, script).await;
    let admission = start_background(&runtime).await;
    let work = runtime.inner.async_work(&admission).unwrap();
    within_liveness_guard("generating", model.parks().wait_for(|count| *count >= 1))
        .await
        .unwrap();
    let (generated, finish_worker) = cancellation::install_gate(&work.generation_settlement_gate);
    let (flagged, publish_token) = cancellation::install_gate(&work.cancellation_request_gate);
    release.send(true).unwrap();
    within_liveness_guard("generated result", generated)
        .await
        .unwrap();
    let owner = runtime.clone();
    let id = admission.operation_id.clone();
    let cancel = tokio::task::spawn_blocking(move || owner.cancel_compaction(&id));
    within_liveness_guard("named cancellation", flagged)
        .await
        .unwrap();
    assert!(!admission.cancellation.is_cancelled());
    finish_worker.send(()).unwrap();
    within_liveness_guard("worker publication before token cancellation", async {
        loop {
            let changed = work.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if work
                .state
                .lock()
                .unwrap()
                .published
                .as_ref()
                .is_some_and(|status| {
                    status.phase.is_terminal() || status.phase == CompactionOperationPhase::Ready
                })
            {
                break;
            }
            changed.await;
        }
    })
    .await;
    publish_token.send(()).unwrap();
    assert!(cancel.await.unwrap().unwrap());
    runtime.inner.reap_async_compaction(true).await;
    let state = work.state.lock().unwrap();
    assert_eq!(state.status.phase, CompactionOperationPhase::Cancelled);
    assert_eq!(state.published.as_ref(), Some(&state.status));
    assert!(runtime.inner.async_work(&admission).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_suffix_during_idle_install_preserves_session_admission() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, _, store) = fixture(&directory, summary_turn_script("retained fact")).await;
    let admission = start_background(&runtime).await;
    runtime.inner.reap_async_compaction(true).await;
    let work = runtime.inner.async_work(&admission).unwrap();
    assert!(matches!(work.state.lock().unwrap().phase, Phase::Ready(_)));
    let mut message = runtime_inbound_message("large-late-suffix");
    message.content = text_content(
        &"x".repeat(crate::context::resources::ResourceLimits::default().record_bytes + 1),
    );
    store
        .append_canonical(&MessageBlock::User(message))
        .unwrap();
    runtime.inner.install_ready_async_when_idle().await;
    assert!(store.projection_head().unwrap().is_none());
    assert!(!runtime.inner.durability_gate.is_failed());
    assert!(runtime.inner.lifecycle.is_running());
    let state = work.state.lock().unwrap();
    assert_eq!(state.published.as_ref(), Some(&state.status));
    assert!(state.status.diagnostic.is_some());
}
