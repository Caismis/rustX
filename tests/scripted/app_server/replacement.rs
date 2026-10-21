//! Unload and replacement ordering: unload and load wait for each other,
//! native admission winners, replacement waits for active attempts and
//! revokes the old client and composition.
#![allow(clippy::too_many_lines)]
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unload_waits_for_load_then_load_waits_for_unload() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        probe.before_compose.arm();
        let first = f.load(0);
        probe.before_compose.entered().await;
        let unload = unload_task(&f, id.clone());
        probe
            .unloads_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        // Park unload after its residency claim and before native shutdown.
        probe.before_shutdown.arm();
        probe.before_compose.release();
        let first = first.await.unwrap().unwrap();
        let old = first.inspect_runtime().unwrap();
        probe.before_shutdown.entered().await;
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloading);
        let reload = f.load(0);
        probe.joined(2).await;
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(!reload.is_finished());
        // Dropping the unload caller does not abandon its claimed transition.
        unload.abort();
        let _ = unload.await;
        probe.before_shutdown.release();
        let second = reload.await.unwrap().unwrap();
        assert_ne!(first.incarnation_id(), second.incarnation_id());
        assert!(old.submit_inbound(input("stale")).is_err());
        assert!(first.inspect_runtime().is_none());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unload_and_inbound_have_both_native_admission_winners() {
    bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        let f = Fixture::with_session_count(None, 1).await;
        let managed = f.load(0).await.unwrap().unwrap();
        let live = managed.inspect_runtime().unwrap();
        let submit_gate = Arc::new(Gate::default());
        let release = submit_gate.arm_scoped();
        live.install_residency_probe(Some(submit_gate.clone()), None);
        let arrival = Arc::new(tokio::sync::Notify::new());
        let drained = Arc::new(tokio::sync::Notify::new());
        live.install_drain_signals(arrival.clone(), drained.clone());
        let inbound = live.clone();
        let submit =
            tokio::task::spawn_blocking(move || inbound.submit_inbound(input("request-A")));
        gate_entered(&submit_gate).await;
        let unload = unload_task(&f, managed.conversation_id().clone());
        arrival.notified().await; // Shutdown has arrived, submit holds coordinator.
        assert!(!unload.is_finished());
        drop(release);
        assert_eq!(submit.await.unwrap().unwrap().inbound_sequence.get(), 1);
        unload.await.unwrap().unwrap();
        assert!(live.submit_inbound(input("late-A")).is_err());
        // Opposite ordering: native drain CAS signals before inbound is released.
        let managed = f.load(0).await.unwrap().unwrap();
        let live = managed.inspect_runtime().unwrap();
        let drained = Arc::new(tokio::sync::Notify::new());
        live.install_drain_signals(Arc::new(tokio::sync::Notify::new()), drained.clone());
        let unload = unload_task(&f, managed.conversation_id().clone());
        drained.notified().await;
        assert!(live.submit_inbound(input("late-B")).is_err());
        unload.await.unwrap().unwrap();
        assert_eq!(
            f.manager.residency(managed.conversation_id()),
            ResidencyState::Unloaded
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacement_waits_for_active_attempt_task_and_changes_only_incarnation_a() {
    bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        use crate::runtime::types::ConversationLifecycleState;
        let f = Fixture::new().await;
        let a = f.load(0).await.unwrap().unwrap();
        let old = a.inspect_runtime().unwrap();
        let b = f.load(1).await.unwrap().unwrap();
        let gate = Arc::new(Gate::default());
        let release = gate.arm_scoped();
        old.install_residency_probe(None, Some(gate.clone()));
        let drain = Arc::new(tokio::sync::Notify::new());
        old.install_drain_signals(Arc::new(tokio::sync::Notify::new()), drain.clone());
        old.submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        let replacement = replace_task(&f, 0);
        drain.notified().await;
        gate_entered(&gate).await;
        assert_eq!(old.lifecycle_state(), ConversationLifecycleState::Draining);
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloading
        );
        assert!(!replacement.is_finished());
        assert_eq!(
            f.manager
                .probe(a.conversation_id())
                .compositions
                .load(Ordering::SeqCst),
            1
        );
        assert!(
            !f.manager
                .is_current(a.conversation_id(), a.incarnation_id())
        );
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        drop(release);
        let new = replacement.await.unwrap().unwrap();
        assert_eq!(new.conversation_id(), a.conversation_id());
        assert_ne!(new.incarnation_id(), a.incarnation_id());
        assert!(
            f.manager
                .is_current(new.conversation_id(), new.incarnation_id())
        );
        assert!(old.submit_inbound(input("stale")).is_err());
        assert_eq!(
            a.client().checkpoint().unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        assert!(Arc::ptr_eq(&b, &f.load(1).await.unwrap().unwrap()));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacement_failure_is_unloaded_and_retryable_but_shutdown_failure_retains_owner() {
    bounded(async {
        let f = Fixture::new().await;
        let a = f.load(0).await.unwrap().unwrap();
        let old = a.inspect_runtime().unwrap();
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        live_b.submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        f.manager
            .probe(a.conversation_id())
            .fail_compose_once
            .store(true, Ordering::SeqCst);
        assert!(replace_task(&f, 0).await.unwrap().is_err());
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloaded
        );
        assert!(old.submit_inbound(input("old")).is_err());
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        let a = f.load(0).await.unwrap().unwrap();
        a.inspect_runtime().unwrap().fail_residency_settlement();
        let failure = f.manager.unload(a.conversation_id()).await.unwrap_err();
        assert!(
            failure
                .to_string()
                .contains("residency test settlement failure")
        );
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloading
        );
        assert!(a.inspect_runtime().is_some());
        assert_eq!(f.load(0).await.unwrap().unwrap_err(), failure);
        assert_eq!(replace_task(&f, 0).await.unwrap().unwrap_err(), failure);
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        let done = live_b.settlement_signal().notified();
        f.gates[1].release();
        done.await;
        f.manager.unload(b.conversation_id()).await.unwrap();
        // Native unproven settlement deliberately remains retained, not a fake
        // successful unload. Process teardown is the terminal failure policy.
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unload_joining_replacement_drains_the_new_incarnation_before_success() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let a = f.load(0).await.unwrap().unwrap();
        let old = a.inspect_runtime().unwrap();
        let probe = f.manager.probe(a.conversation_id());
        probe.before_shutdown.arm();
        let replacement = replace_task(&f, 0);
        probe.before_shutdown.entered().await;
        let unload = unload_task(&f, a.conversation_id().clone());
        probe
            .unloads_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        assert!(!unload.is_finished());
        probe.before_shutdown.release();
        let new = replacement.await.unwrap().unwrap();
        unload.await.unwrap().unwrap();
        assert_ne!(a.incarnation_id(), new.incarnation_id());
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Unloaded
        );
        assert!(new.inspect_runtime().is_none());
        assert!(old.submit_inbound(input("stale")).is_err());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replacement_revokes_old_client_and_releases_old_composition() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let old = f.load(0).await.unwrap().unwrap();
        let client = old.client();
        let weak = old.inspect_runtime().unwrap().weak_inner();
        let new = f.manager.replace(&f.sessions[0].id, None).await.unwrap();
        assert_eq!(old.conversation_id(), new.conversation_id());
        assert_ne!(old.incarnation_id(), new.incarnation_id());
        assert!(weak.upgrade().is_none());
        assert_eq!(
            client.submit_inbound(input("stale")).unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        assert_eq!(
            client.checkpoint().unwrap_err(),
            RuntimeManagerError::StaleIncarnation
        );
        new.client().checkpoint().unwrap();
        assert!(
            f.manager
                .is_current(new.conversation_id(), new.incarnation_id())
        );
        f.close().await;
    })
    .await;
}
