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
    within_liveness_guard("failed staging settlement", runtime.attempt_settlement(0)).await;
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

fn journal_statuses(
    runtime: &ConversationRuntime,
    admission: &CompactionAdmission,
) -> Vec<CompactionOperationStatus> {
    runtime
        .inner
        .store
        .read_events(None, 256)
        .unwrap()
        .events
        .into_iter()
        .filter_map(|event| match event.event {
            RuntimeEvent::CompactionOperationUpdated { status }
                if status.operation_id == admission.operation_id =>
            {
                Some(status)
            }
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_waits_for_named_cancellation_of_a_joined_ready_worker() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, _, store) = fixture(&directory, summary_turn_script("retained fact")).await;
    let admission = start_background(&runtime).await;
    runtime.inner.reap_async_compaction(true).await;
    let work = runtime.inner.async_work(&admission).unwrap();
    assert!(runtime.inner.lifecycle.idle_epoch().is_some());
    assert_eq!(
        journal_statuses(&runtime, &admission).last().unwrap().phase,
        CompactionOperationPhase::Ready
    );

    let (publishing, release) =
        cancellation::install_gate(&work.named_cancellation_publication_gate);
    let owner = runtime.clone();
    let id = admission.operation_id.clone();
    let cancel = tokio::task::spawn_blocking(move || owner.cancel_compaction(&id));
    within_liveness_guard("named cancellation publication", publishing)
        .await
        .unwrap();
    let drain = runtime.inner.begin_drain().unwrap();
    let admissions = runtime.inner.lifecycle.wait_for_no_admissions();
    tokio::pin!(admissions);
    assert!(futures_util::poll!(&mut admissions).is_pending());
    assert!(!drain.completed.load(Ordering::Acquire));
    assert_eq!(
        journal_statuses(&runtime, &admission).last().unwrap().phase,
        CompactionOperationPhase::Ready
    );

    release.send(()).unwrap();
    assert!(
        within_liveness_guard("named cancellation settlement", cancel)
            .await
            .unwrap()
            .unwrap()
    );
    within_liveness_guard("shutdown after cancellation", runtime.shutdown())
        .await
        .unwrap();
    assert_eq!(
        runtime.inner.lifecycle.state(),
        crate::runtime::types::ConversationLifecycleState::Quiescent
    );
    assert!(runtime.inner.async_work(&admission).is_none());
    let terminal: Vec<_> = journal_statuses(&runtime, &admission)
        .into_iter()
        .filter(|status| status.phase.is_terminal())
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].phase, CompactionOperationPhase::Cancelled);
    let settled_cut = store.compaction_resource_cut().unwrap().journal;
    assert!(!runtime.cancel_compaction(&admission.operation_id).unwrap());
    assert_eq!(
        store.compaction_resource_cut().unwrap().journal,
        settled_cut
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn named_cancellation_after_idle_drain_admission_closes_leaves_publication_to_drain() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, _, store) = fixture(&directory, summary_turn_script("retained fact")).await;
    let admission = start_background(&runtime).await;
    runtime.inner.reap_async_compaction(true).await;
    let lifecycle = &runtime.inner.lifecycle;
    assert!(lifecycle.begin_idle_drain(lifecycle.idle_epoch().unwrap()));
    let before = store.compaction_resource_cut().unwrap().journal;
    assert!(!runtime.cancel_compaction(&admission.operation_id).unwrap());
    assert_eq!(store.compaction_resource_cut().unwrap().journal, before);
    within_liveness_guard("drain owns ready cancellation", runtime.shutdown())
        .await
        .unwrap();
    assert_eq!(
        journal_statuses(&runtime, &admission).last().unwrap().phase,
        CompactionOperationPhase::Cancelled
    );
    assert!(runtime.inner.async_work(&admission).is_none());
}

async fn capture_failure_keeps_cancellation(named: bool) {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, _, _) = fixture(&directory, Vec::new()).await;
    let protected = [runtime_inbound_message("first").id];
    let constraints = if named {
        CompactionConstraints {
            protected_context_ids: &protected,
            ..CompactionConstraints::default()
        }
    } else {
        CompactionConstraints {
            estimate_correction: crate::context::engine::EstimateCorrection::new(1, u64::MAX),
            ..CompactionConstraints::default()
        }
    };
    let (admission, operation, captured) = capture_background(&runtime, &constraints).await;
    let Err(error) = captured else {
        panic!("expected planning failure")
    };
    let expected = if named {
        ContextErrorKind::NoEligibleHistory
    } else {
        ContextErrorKind::CannotFit
    };
    assert!(
        matches!(&error, CompactionExecutionError::Context(error) if error.kind == expected),
        "{error:?}"
    );
    operation.close().await;
    let work = runtime.inner.async_work(&admission).unwrap();
    let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(&runtime.inner));
    if named {
        let (flagged, token) = cancellation::install_gate(&work.cancellation_request_gate);
        let owner = runtime.clone();
        let id = admission.operation_id.clone();
        let cancel = tokio::task::spawn_blocking(move || owner.cancel_compaction(&id));
        within_liveness_guard("accepted named cancellation before its token", flagged)
            .await
            .unwrap();
        assert!(!admission.cancellation.is_cancelled());
        coordinator.fail_admission(&admission, &error).unwrap();
        token.send(()).unwrap();
        assert!(
            within_liveness_guard("named capture cancellation", cancel)
                .await
                .unwrap()
                .unwrap()
        );
    } else {
        // Represent the foreground owner holding capture until settlement.
        hold_idle_installation(&mut runtime.inner.lock_state());
        runtime.inner.begin_drain().unwrap();
        assert!(admission.cancellation.is_cancelled());
        coordinator.fail_admission(&admission, &error).unwrap();
        release_idle_installation(&mut runtime.inner.lock_state());
        runtime.inner.settlement.notify_waiters();
    }
    assert!(work.lifecycle.lock().unwrap().is_none());
    within_liveness_guard(
        "capture admission release",
        runtime.inner.lifecycle.wait_for_no_admissions(),
    )
    .await;
    within_liveness_guard(
        "capture retirement",
        runtime.inner.reap_async_compaction(true),
    )
    .await;
    assert!(runtime.inner.async_work(&admission).is_none());
    let statuses = journal_statuses(&runtime, &admission);
    let status = statuses.last().unwrap();
    assert_eq!(status.phase, CompactionOperationPhase::Cancelled);
    assert!(status.diagnostic.is_none());
    assert!(status.waiting_attempt_id.is_none());
    within_liveness_guard("capture shutdown", runtime.shutdown())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_named_cancellation_beats_no_eligible_history_before_token() {
    capture_failure_keeps_cancellation(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drain_cancellation_beats_cannot_fit() {
    capture_failure_keeps_cancellation(false).await;
}

#[tokio::test]
async fn cancelled_capture_still_latches_its_durability_failure() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, _, _) = fixture(&directory, Vec::new()).await;
    let coordinator = RuntimeCompactionCoordinator(Arc::downgrade(&runtime.inner));
    let admission = coordinator
        .admit(&AttemptId::new("capture"))
        .unwrap()
        .unwrap();
    assert!(runtime.cancel_compaction(&admission.operation_id).unwrap());
    let error = CompactionExecutionError::Durable("failed to read capture source".into());
    coordinator.fail_admission(&admission, &error).unwrap();
    let failure = runtime.inner.durability_gate.failure().unwrap();
    assert_eq!(failure.operation, DurableOperation::CanonicalCommit);
    assert_eq!(failure.diagnostic, "failed to read capture source");
    let statuses = journal_statuses(&runtime, &admission);
    let status = statuses.last().unwrap();
    assert_eq!(status.phase, CompactionOperationPhase::Cancelled);
    assert!(status.diagnostic.is_none());
    let _ = within_liveness_guard("failed capture drain", runtime.shutdown()).await;
}

#[tokio::test]
async fn manual_compaction_hydration_failure_fences_subsequent_admission() {
    use crate::runtime::conversation_runtime::compaction::ManualCompactionError;

    let directory = tempfile::tempdir().unwrap();
    let (runtime, _, store) = fixture(&directory, Vec::new()).await;
    assert!(runtime.inner.lock_state().foreground.slot.owned().is_none());
    *store.fail_projected_history_when.lock().unwrap() = Some(Box::new(|| true));
    let Err(ManualCompactionError::Durable { message }) = runtime.admit_compaction(None, None)
    else {
        panic!("first manual admission should fail hydration");
    };
    let failure = runtime.inner.durability_gate.failure().unwrap();
    assert_eq!(failure.operation, DurableOperation::HydrateConversation);
    assert_eq!(failure.diagnostic, message);
    *store.fail_projected_history_when.lock().unwrap() = None;
    assert!(
        matches!(runtime.admit_compaction(None, None), Err(ManualCompactionError::DurabilityFailed { message }) if message == failure.diagnostic)
    );
}
