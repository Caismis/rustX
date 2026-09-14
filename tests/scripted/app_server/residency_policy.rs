use super::*;
use crate::runtime::monotonic::ManualMonotonicClock;

fn policy(f: &mut Fixture, limit: usize) -> Arc<ManualMonotonicClock> {
    let clock = Arc::new(ManualMonotonicClock::new());
    f.manager.clock = clock.clone();
    let mut state = f.manager.registry.0.lock().unwrap();
    state.policy.max_resident_runtimes = limit;
    state.policy.idle_grace_ms = 100;
    clock
}

#[tokio::test]
async fn quota_counts_loading_loaded_unloading_and_single_flight() {
    let mut f = Fixture::new().await;
    policy(&mut f, 1);
    let id = f.id(0).await;
    let probe = f.manager.probe(&id);
    probe.before_compose.arm();
    let first = f.load(0);
    probe.before_compose.entered().await;
    let same = f.load(0);
    probe.joined(2).await;
    assert_eq!(f.manager.diagnostics().loading, 1);
    assert_eq!(
        f.load(1).await.unwrap().unwrap_err(),
        RuntimeManagerError::ResidencyCapacity
    );
    probe.before_compose.release();
    let first = first.await.unwrap().unwrap();
    assert!(Arc::ptr_eq(&first, &same.await.unwrap().unwrap()));
    assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.load(1).await.unwrap().unwrap_err(),
        RuntimeManagerError::ResidencyCapacity
    );
    probe.before_shutdown.arm();
    let manager = f.manager.clone();
    let unload_id = id.clone();
    let unload = tokio::spawn(async move { manager.unload(&unload_id).await });
    probe.before_shutdown.entered().await;
    assert_eq!(f.manager.diagnostics().unloading, 1);
    assert_eq!(
        f.load(1).await.unwrap().unwrap_err(),
        RuntimeManagerError::ResidencyCapacity
    );
    probe.before_shutdown.release();
    unload.await.unwrap().unwrap();
    f.load(1).await.unwrap().unwrap();
    f.close().await;
}

#[tokio::test]
async fn failed_composition_releases_reservation() {
    let mut f = Fixture::new().await;
    policy(&mut f, 1);
    f.manager
        .probe(&f.id(0).await)
        .panic_once
        .store(true, Ordering::SeqCst);
    assert!(f.load(0).await.unwrap().is_err());
    assert_eq!(f.manager.diagnostics().loading, 0);
    f.load(1).await.unwrap().unwrap();
    f.close().await;
}

#[tokio::test]
async fn idle_grace_detach_eviction_cold_resume_and_session_independence() {
    let mut f = Fixture::new().await;
    let clock = policy(&mut f, 2);
    let a = f.load(0).await.unwrap().unwrap();
    let b = f.load(1).await.unwrap().unwrap();
    let (_attachment, external) = a.client().attach().unwrap();
    let (_b_attachment, _b_external) = b.client().attach().unwrap();
    f.manager.reap_idle();
    clock.advance(101);
    f.manager.reap_idle();
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Loaded
    );
    external.release();
    f.manager.reap_idle();
    clock.advance(99);
    f.manager.reap_idle();
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Loaded
    );
    clock.advance(1);
    f.manager.reap_idle();
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Unloading
    );
    assert_eq!(
        a.client().submit_inbound(input("late")).unwrap_err(),
        RuntimeManagerError::StaleIncarnation
    );
    f.manager.unload(a.conversation_id()).await.unwrap();
    assert_eq!(
        f.manager.residency(b.conversation_id()),
        ResidencyState::Loaded
    );
    let recovered = f.load(0).await.unwrap().unwrap();
    assert_ne!(a.incarnation_id(), recovered.incarnation_id());
    f.close().await;
}

#[tokio::test]
async fn operation_wins_idle_and_drain_retains_owned_lease_after_waiter_drop() {
    let mut f = Fixture::new().await;
    let clock = policy(&mut f, 2);
    let a = f.load(0).await.unwrap().unwrap();
    f.manager.reap_idle();
    clock.advance(100);
    let probe = f.manager.probe(a.conversation_id());
    probe.before_operation.arm();
    let receiver = a.client().start_operation(|| async {}).unwrap();
    probe.before_operation.entered().await;
    drop(receiver);
    f.manager.reap_idle();
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Loaded
    );
    f.manager.begin_drain();
    assert_eq!(
        a.client().start_operation(|| async {}).unwrap_err(),
        RuntimeManagerError::ServerDraining
    );
    assert_eq!(
        f.load(1).await.unwrap().unwrap_err(),
        RuntimeManagerError::ServerDraining
    );
    let manager = f.manager.clone();
    let drain = tokio::spawn(async move { manager.drain().await });
    probe
        .draining_operations
        .subscribe()
        .wait_for(|value| *value)
        .await
        .unwrap();
    assert!(!drain.is_finished());
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Unloading
    );
    probe.before_operation.release();
    assert!(drain.await.unwrap().is_empty());
    f.manager.finish_drain().unwrap();
    assert_eq!(
        f.manager.diagnostics().lifecycle,
        ServerLifecycle::Terminated
    );
    f.close().await;
}

#[tokio::test]
async fn external_attachment_capacity_is_independent_and_released_on_detach() {
    let mut f = Fixture::new().await;
    policy(&mut f, 2);
    f.manager
        .registry
        .0
        .lock()
        .unwrap()
        .policy
        .max_external_attachments = 1;
    let a = f.load(0).await.unwrap().unwrap();
    let b = f.load(1).await.unwrap().unwrap();
    let (_a, external) = a.client().attach().unwrap();
    assert!(matches!(
        b.client().attach(),
        Err(RuntimeManagerError::AttachmentCapacity)
    ));
    external.release();
    let _b = b.client().attach().unwrap();
    assert_eq!(f.manager.diagnostics().external_attachments, 1);
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_claim_has_both_attach_and_inbound_winners_at_publication() {
    bounded(async {
        for attachment_wins in [false, true] {
            let mut f = Fixture::new().await;
            let clock = policy(&mut f, 2);
            let a = f.load(0).await.unwrap().unwrap();
            let native = a.inspect_runtime().unwrap();
            while native.idle_epoch().is_err() {
                tokio::task::yield_now().await;
            }
            f.manager.reap_idle();
            clock.advance(100);
            let probe = f.manager.probe(a.conversation_id());
            let guard = probe.idle_before_claim.arm_scoped();
            let manager = f.manager.clone();
            let scan = tokio::task::spawn_blocking(move || manager.reap_idle());
            gate_entered(&probe.idle_before_claim).await;
            let attachment = if attachment_wins {
                Some(a.client().attach().unwrap())
            } else {
                a.client().submit_inbound(input("request-A")).unwrap();
                None
            };
            drop(guard);
            scan.await.unwrap();
            assert_eq!(
                f.manager.residency(a.conversation_id()),
                ResidencyState::Loaded
            );
            drop(attachment);
            f.close().await;
        }
        let mut f = Fixture::new().await;
        let clock = policy(&mut f, 2);
        let a = f.load(0).await.unwrap().unwrap();
        let native = a.inspect_runtime().unwrap();
        while native.idle_epoch().is_err() {
            tokio::task::yield_now().await;
        }
        f.manager.reap_idle();
        clock.advance(100);
        let probe = f.manager.probe(a.conversation_id());
        let guard = probe.idle_after_claim.arm_scoped();
        let manager = f.manager.clone();
        let scan = tokio::task::spawn_blocking(move || manager.reap_idle());
        gate_entered(&probe.idle_after_claim).await;
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloading
        );
        assert!(matches!(
            a.client().attach(),
            Err(RuntimeManagerError::StaleIncarnation)
        ));
        assert_eq!(
            a.client().submit_inbound(input("late")).unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        let reloading = f.load(0);
        probe.joined(2).await;
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        drop(guard);
        scan.await.unwrap();
        let new = reloading.await.unwrap().unwrap();
        assert_ne!(a.incarnation_id(), new.incarnation_id());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn active_turn_after_detach_starts_grace_only_after_settlement() {
    bounded(async {
        let mut f = Fixture::new().await;
        let clock = policy(&mut f, 2);
        let a = f.load(0).await.unwrap().unwrap();
        let native = a.inspect_runtime().unwrap();
        let (attached, external) = a.client().attach().unwrap();
        a.client().submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        attached.attachment.detach();
        external.release();
        clock.advance(1000);
        f.manager.reap_idle();
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        assert!(f.manager.diagnostics().sessions[0].idle_for_ms.is_none());
        f.gates[0].release();
        while native.idle_epoch().is_err() {
            tokio::task::yield_now().await;
        }
        f.manager.reap_idle();
        clock.advance(99);
        f.manager.reap_idle();
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        clock.advance(1);
        f.manager.reap_idle();
        f.manager.unload(a.conversation_id()).await.unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn drain_reports_failure_and_still_settles_sibling_without_releasing_failed_slot() {
    let mut f = Fixture::new().await;
    policy(&mut f, 2);
    let a = f.load(0).await.unwrap().unwrap();
    let b = f.load(1).await.unwrap().unwrap();
    a.inspect_runtime().unwrap().fail_residency_settlement();
    assert!(f.manager.unload(a.conversation_id()).await.is_err());
    let c = f
        .manager
        .sessions
        .create_session(SessionPersistentState::from_input(
            &SessionConfigInput::new(f.workspaces[0].clone()),
        ))
        .await
        .unwrap()
        .session;
    assert_eq!(
        f.manager.load(&c.id, None).await.unwrap_err(),
        RuntimeManagerError::ResidencyCapacity
    );
    let failures = f.manager.drain().await;
    assert_eq!(failures.len(), 1);
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Unloading
    );
    assert_eq!(
        f.manager.residency(b.conversation_id()),
        ResidencyState::Unloaded
    );
    assert_eq!(f.manager.diagnostics().lifecycle, ServerLifecycle::Draining);
    assert_eq!(f.manager.diagnostics().shutdown_failures, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_inbound_before_attempt_admission_prevents_idle_claim() {
    bounded(async {
        let mut f = Fixture::new().await;
        let clock = policy(&mut f, 2);
        let a = f.load(0).await.unwrap().unwrap();
        let native = a.inspect_runtime().unwrap();
        while native.idle_epoch().is_err() {
            tokio::task::yield_now().await;
        }
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        native.install_admission_gate(gate.clone());
        let guard = gate.arm_scoped();
        a.client().submit_inbound(input("request-A")).unwrap();
        gate_entered(&gate).await;
        assert!(!native.has_current_attempt());
        assert!(native.idle_epoch().is_err());
        clock.advance(1000);
        f.manager.reap_idle();
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        drop(guard);
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn goal_extension_is_conservatively_retained_and_policy_is_user_only() {
    let mut f = Fixture::new().await;
    let clock = policy(&mut f, 2);
    std::fs::write(
        f.workspaces[0].join("rustx.toml"),
        "[agent.extensions.goal]\nenabled = true\n",
    )
    .unwrap();
    let a = f.load(0).await.unwrap().unwrap();
    f.manager.reap_idle();
    clock.advance(1000);
    f.manager.reap_idle();
    assert_eq!(
        f.manager.residency(a.conversation_id()),
        ResidencyState::Loaded
    );
    std::fs::write(
        f.workspaces[1].join("rustx.toml"),
        "[app_server]\nmax_resident_runtimes = 2\n",
    )
    .unwrap();
    assert!(
        f.load(1)
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("forbidden")
    );
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_distinct_loads_cannot_oversubscribe_one_slot() {
    let mut f = Fixture::new().await;
    policy(&mut f, 1);
    let (a, b) = tokio::join!(f.load(0), f.load(1));
    let results = [a.unwrap(), b.unwrap()];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(RuntimeManagerError::ResidencyCapacity)))
            .count(),
        1
    );
    assert_eq!(f.manager.diagnostics().loaded, 1);
    f.close().await;
}

#[tokio::test]
async fn one_reaper_timer_wakes_from_manual_clock_and_unloads_automatically() {
    bounded(async {
        let mut f = Fixture::new().await;
        let clock = policy(&mut f, 2);
        let a = f.load(0).await.unwrap().unwrap();
        let native = a.inspect_runtime().unwrap();
        while native.idle_epoch().is_err() {
            tokio::task::yield_now().await;
        }
        let (waiting, mut observed) = watch::channel(0);
        f.manager.registry.0.lock().unwrap().reaper_waiting = Some(waiting);
        let stop = tokio_util::sync::CancellationToken::new();
        let worker_stop = stop.clone();
        let manager = f.manager.clone();
        let worker = tokio::spawn(async move { manager.run_idle_reaper(worker_stop).await });
        observed
            .wait_for(|deadline| *deadline == 1000)
            .await
            .unwrap();
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        clock.advance(1000);
        observed
            .wait_for(|deadline| *deadline == 2000)
            .await
            .unwrap();
        assert_ne!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        f.manager.unload(a.conversation_id()).await.unwrap();
        stop.cancel();
        worker.await.unwrap();
        f.close().await;
    })
    .await;
}
