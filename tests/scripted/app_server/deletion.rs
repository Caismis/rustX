//! Deletion fences against loading, attachment and replacement, physical
//! cleanup, writer transfer, and deletion recovery under durability
//! uncertainty.
#![allow(clippy::too_many_lines)]
use super::*;

pub(super) async fn deletion_revision(f: &Fixture, index: usize) -> String {
    let crate::local_runtime::session::deletion::SessionDeleteResult::Preview { preview } = f
        .manager
        .sessions
        .delete_preview(&f.sessions[index].id)
        .await
    else {
        panic!("preview")
    };
    preview.target_revision
}

pub(super) fn delete_task(
    f: &Fixture,
    index: usize,
    revision: String,
) -> tokio::task::JoinHandle<
    Result<crate::local_runtime::session::deletion::SessionDeleteResult, RuntimeManagerError>,
> {
    let manager = f.manager.clone();
    let id = f.sessions[index].id.clone();
    tokio::spawn(async move { manager.delete_session(&id, &revision).await })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_fence_prevents_loading_publication_and_isolates_other_sessions() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        let revision = deletion_revision(&f, 0).await;
        probe.before_compose.arm();
        let load = f.load(0);
        probe.before_compose.entered().await;
        // Joining callers hold no redundant allocation across retirement.
        let joined_load = f.load(0);
        probe.joined(2).await;
        // Preview remains available while a load owns its allocation.
        assert_eq!(deletion_revision(&f, 0).await, revision);
        probe.after_delete_fence.arm();
        let delete = delete_task(&f, 0, revision);
        probe.after_delete_fence.entered().await;
        assert!(f.load(0).await.unwrap().is_err());
        let other = f.load(1).await.unwrap().unwrap();
        assert!(other.client().validate().is_ok());
        probe.before_compose.release();
        probe.after_delete_fence.release();
        assert!(load.await.unwrap().is_err());
        assert!(joined_load.await.unwrap().is_err());
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(other.client().validate().is_ok());
        assert!(f.load(0).await.unwrap().is_err());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_fence_rejects_late_operation_attach_and_replacement() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::with_session_count(None, 1).await;
        let runtime = f.load(0).await.unwrap().unwrap();
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(runtime.conversation_id());
        probe.after_delete_fence.arm();
        let delete = delete_task(&f, 0, revision);
        probe.after_delete_fence.entered().await;
        assert!(
            runtime
                .client()
                .submit_inbound(input("never admitted"))
                .is_err()
        );
        assert!(runtime.client().attach().is_err());
        assert!(replace_task(&f, 0).await.unwrap().is_err());
        assert!(f.load(0).await.unwrap().is_err());
        probe.after_delete_fence.release();
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert!(runtime.inspect_runtime().is_none());
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_joins_replacement_without_publishing_its_candidate() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::with_session_count(None, 1).await;
        let old = f.load(0).await.unwrap().unwrap();
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(old.conversation_id());
        probe.before_shutdown.arm();
        let replace = replace_task(&f, 0);
        probe.before_shutdown.entered().await;
        probe.after_delete_fence.arm();
        let delete = delete_task(&f, 0, revision);
        probe.after_delete_fence.entered().await;
        probe.before_shutdown.release();
        probe.after_delete_fence.release();
        assert!(replace.await.unwrap().is_err());
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert!(old.inspect_runtime().is_none());
        assert!(f.load(0).await.unwrap().is_err());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_retirement_failure_retains_writer_slot_and_session_fence() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let runtime = f.load(0).await.unwrap().unwrap();
        let revision = deletion_revision(&f, 0).await;
        runtime
            .inspect_runtime()
            .unwrap()
            .fail_residency_settlement();
        let result = f.manager.delete_session(&f.sessions[0].id, &revision).await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("residency test settlement failure")
        );
        let flight = {
            let registry = f.manager.registry.0.lock().unwrap();
            assert!(registry.retiring_sessions.contains(&f.sessions[0].id));
            let Some(Entry::Unloading { flight, .. }) =
                registry.entries.get(runtime.conversation_id())
            else {
                panic!("unproven writer slot")
            };
            flight.clone()
        };
        assert!(matches!(
            flight.wait().await,
            Outcome::RetirementUnproven(_)
        ));
        assert!(
            f.manager
                .sessions
                .read_session(&f.sessions[0].id)
                .await
                .is_ok()
        );
        assert_eq!(
            f.manager.residency(runtime.conversation_id()),
            ResidencyState::Unloading
        );
        assert!(runtime.inspect_runtime().is_some());
        assert!(f.load(0).await.unwrap().is_err());
        assert!(replace_task(&f, 0).await.unwrap().is_err());
        assert!(matches!(
            f.manager
                .recover_session_deletion(&f.sessions[0].id)
                .await
                .unwrap(),
            crate::local_runtime::session::deletion::SessionDeleteResult::Preview { .. }
        ));
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(&f.sessions[0].id)
        );
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(f.manager.diagnostics().unloading, 1);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_settles_an_active_native_attempt() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::with_session_count(None, 1).await;
        let runtime = f.load(0).await.unwrap().unwrap();
        runtime.client().submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        let revision = deletion_revision(&f, 0).await;
        let live = runtime.inspect_runtime().unwrap();
        let weak = live.weak_inner();
        let arrival = Arc::new(tokio::sync::Notify::new());
        let drained = Arc::new(tokio::sync::Notify::new());
        live.install_drain_signals(arrival.clone(), drained.clone());
        drop(live);
        let result = f
            .manager
            .delete_session(&f.sessions[0].id, &revision)
            .await
            .unwrap();
        // Native coordinator Running -> Draining, not a catalog-only shortcut.
        arrival.notified().await;
        drained.notified().await;
        assert!(matches!(result, SessionDeleteResult::Deleted { .. }));
        assert!(weak.upgrade().is_none());
        assert!(runtime.inspect_runtime().is_none());
        f.gates[0].release();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_delete_rejects_load_before_physical_cleanup_finishes() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        use crate::runtime::conversation_runtime::Gate;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        let SessionDeleteResult::Preview { preview } =
            f.manager.sessions.delete_preview(&f.sessions[0].id).await
        else {
            panic!("unloaded")
        };
        let gate = Arc::new(Gate::default());
        let release = gate.arm_scoped();
        f.manager.sessions.install_delete_cleanup_gate(gate.clone());
        let sessions = f.manager.sessions.clone();
        let session = f.sessions[0].id.clone();
        let deletion = tokio::spawn(async move {
            sessions
                .delete_session(&session, &preview.target_revision)
                .await
        });
        gate_entered(&gate).await; // Durable delete committed; filesystem cleanup parked.
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 0);
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        let b = f.load(1).await.unwrap().unwrap();
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        drop(release);
        assert!(matches!(
            deletion.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_fence_before_load_prevents_allocation_acquisition() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(&id);
        probe.after_delete_fence.arm();
        let delete = delete_task(&f, 0, revision);
        probe.after_delete_fence.entered().await;
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 0);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 0);
        assert!(f.load(1).await.unwrap().is_ok());
        probe.after_delete_fence.release();
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn load_claim_is_visible_to_delete_before_allocation_acquisition() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(&id);
        probe.before_allocation.arm();
        let load = f.load(0);
        probe.before_allocation.entered().await;
        assert_eq!(f.manager.residency(&id), ResidencyState::Loading);
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 0);
        let joined = f.load(0);
        probe.joined(2).await;
        let delete = delete_task(&f, 0, revision);
        // This watch fires only when delete sees and joins the registered flight.
        probe
            .unloads_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        assert!(!delete.is_finished());
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(&f.sessions[0].id)
        );
        assert!(
            f.load(1)
                .await
                .unwrap()
                .unwrap()
                .client()
                .validate()
                .is_ok()
        );
        probe.before_allocation.release();
        assert!(load.await.unwrap().is_err());
        assert!(joined.await.unwrap().is_err());
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 1);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(f.provider.request_bodies().is_empty());
        assert!(
            f.manager
                .sessions
                .read_session(&f.sessions[0].id)
                .await
                .is_err()
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_joining_failed_loading_receives_proven_writer_absence() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::with_session_count(None, 1).await;
        let id = f.id(0).await;
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(&id);
        probe.before_compose.arm();
        probe.fail_compose_once.store(true, Ordering::SeqCst);
        let load = f.load(0);
        probe.before_compose.entered().await;
        let flight = {
            let registry = f.manager.registry.0.lock().unwrap();
            let Some(Entry::Loading(flight)) = registry.entries.get(&id) else {
                panic!("Loading")
            };
            flight.clone()
        };
        let delete = delete_task(&f, 0, revision);
        probe
            .unloads_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        assert!(!delete.is_finished());
        probe.before_compose.release();
        assert!(load.await.unwrap().is_err());
        assert!(matches!(flight.wait().await, Outcome::WriterAbsent(Err(_))));
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        assert!(
            !f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(&f.sessions[0].id)
        );
        assert!(
            f.manager
                .sessions
                .read_session(&f.sessions[0].id)
                .await
                .is_err()
        );
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 1);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_joins_replacement_failure_after_proven_writer_transfer() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::with_session_count(None, 1).await;
        let old = f.load(0).await.unwrap().unwrap();
        let id = old.conversation_id();
        let weak = old.inspect_runtime().unwrap().weak_inner();
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(id);
        probe.after_writer_transfer.arm();
        probe.fail_compose_once.store(true, Ordering::SeqCst);
        let replace = replace_task(&f, 0);
        probe.after_writer_transfer.entered().await;
        assert!(
            weak.upgrade().is_none(),
            "old native writer released before replacement acquisition"
        );
        assert!(old.inspect_runtime().is_none());
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 1);
        let flight = {
            let registry = f.manager.registry.0.lock().unwrap();
            let Some(Entry::Loading(flight)) = registry.entries.get(id) else {
                panic!("replacement Loading")
            };
            flight.clone()
        };
        let delete = delete_task(&f, 0, revision);
        probe
            .unloads_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        assert!(!delete.is_finished());
        probe.after_writer_transfer.release();
        assert!(replace.await.unwrap().is_err());
        assert!(matches!(flight.wait().await, Outcome::WriterAbsent(Err(_))));
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert_eq!(f.manager.residency(id), ResidencyState::Unloaded);
        assert!(
            !f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(&f.sessions[0].id)
        );
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 2);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
        assert!(f.provider.request_bodies().is_empty());
        assert!(
            f.manager
                .sessions
                .read_session(&f.sessions[0].id)
                .await
                .is_err()
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_observes_live_session_without_releasing_active_delete_fence() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::with_session_count(None, 1).await;
        let id = f.id(0).await;
        let session = &f.sessions[0].id;
        let revision = deletion_revision(&f, 0).await;
        let probe = f.manager.probe(&id);
        probe.after_delete_fence.arm();
        let delete = delete_task(&f, 0, revision.clone());
        probe.after_delete_fence.entered().await;
        let SessionDeleteResult::Preview { preview } =
            f.manager.recover_session_deletion(session).await.unwrap()
        else {
            panic!("still-live Session must return Preview")
        };
        assert_eq!(preview.target_revision, revision);
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(session)
        );
        assert!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .pending_deletion_ids()
                .is_empty()
        );
        assert!(!delete.is_finished());
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 0);
        probe.after_delete_fence.release();
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_settles_durability_uncertainty_without_runtime_reconstruction() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let old = f.load(0).await.unwrap().unwrap();
        let session = &f.sessions[0].id;
        let probe = f.manager.probe(old.conversation_id());
        let revision = deletion_revision(&f, 0).await;
        f.manager
            .sessions
            .catalog
            .lock()
            .await
            .arm_write_fault_after_rename();
        assert!(matches!(
            f.manager.delete_session(session, &revision).await.unwrap(),
            SessionDeleteResult::CommittedDurabilityUncertain { .. }
        ));
        assert!(old.inspect_runtime().is_none());
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(session)
        );
        assert!(f.load(0).await.unwrap().is_err());
        assert!(
            f.load(1)
                .await
                .unwrap()
                .unwrap()
                .client()
                .validate()
                .is_ok()
        );
        assert!(matches!(
            f.manager.recover_session_deletion(session).await.unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert!(
            !f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(session)
        );
        assert!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .pending_deletion_ids()
                .is_empty()
        );
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        // Repeated reconciliation confirms absence; it never recreates authority.
        assert!(matches!(
            f.manager.recover_session_deletion(session).await.unwrap(),
            SessionDeleteResult::NotFound { .. }
        ));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_with_unproven_catalog_durability_retains_delete_fence() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::with_session_count(None, 1).await;
        let session = &f.sessions[0].id;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        let revision = deletion_revision(&f, 0).await;
        f.manager
            .sessions
            .catalog
            .lock()
            .await
            .arm_write_fault_after_rename();
        assert!(matches!(
            f.manager.delete_session(session, &revision).await.unwrap(),
            SessionDeleteResult::CommittedDurabilityUncertain { .. }
        ));
        f.manager
            .sessions
            .catalog
            .lock()
            .await
            .arm_write_fault_before_rename();
        assert!(matches!(
            f.manager.recover_session_deletion(session).await.unwrap(),
            SessionDeleteResult::CommittedDurabilityUncertain { .. }
        ));
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(session)
        );
        assert_eq!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .pending_deletion_ids(),
            vec![session.clone()]
        );
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 0);
        assert_eq!(probe.acquisitions.load(Ordering::SeqCst), 0);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_retires_abandoned_unpublished_activation_without_inventing_an_agent() {
    use crate::local_runtime::session::deletion::SessionDeleteResult;
    use crate::runtime::subagent::physical_recovery::{AllocationBoundary, ParentPhysicalLease};
    for boundary in [
        AllocationBoundary::Created,
        AllocationBoundary::LeaseCreated,
        AllocationBoundary::Initialized,
    ] {
        let f = Fixture::with_session_count(None, 1).await;
        let session = &f.sessions[0];
        let product =
            crate::runtime::local_storage::ProductRoot::existing(&f.archive_root).unwrap();
        let activation = crate::runtime::identity::SubagentId::for_conversation(
            &session.active_conversation_id,
            1,
        );
        assert!(
            ParentPhysicalLease::reserve_at_boundaries(
                &product,
                &session.id,
                &ConversationId::generate(),
                &activation,
                |at| if at == boundary {
                    Err(std::io::Error::other("abandoned initialization"))
                } else {
                    Ok(())
                }
            )
            .is_err()
        );
        let managed = f.load(0).await.unwrap().unwrap();
        let runtime = managed.inspect_runtime().unwrap();
        let registry = runtime.subagent_registry().unwrap();
        registry.wait_recovery_reconciliation().await;
        assert!(registry.all_snapshots().is_empty());
        assert!(registry.unproven_settlements().is_empty());
        drop(runtime); // release the inspection lease before actual deletion cleanup
        let revision = deletion_revision(&f, 0).await;
        assert!(matches!(
            f.manager
                .delete_session(&session.id, &revision)
                .await
                .unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert!(managed.inspect_runtime().is_none());
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deletion_keeps_live_published_activation_fenced() {
    use crate::runtime::subagent::physical_recovery::ParentPhysicalLease;
    let f = Fixture::with_session_count(None, 1).await;
    let session = &f.sessions[0];
    let product = crate::runtime::local_storage::ProductRoot::existing(&f.archive_root).unwrap();
    let activation =
        crate::runtime::identity::SubagentId::for_conversation(&session.active_conversation_id, 1);
    let owner = ParentPhysicalLease::reserve(
        &product,
        &session.id,
        &ConversationId::generate(),
        &activation,
    )
    .unwrap();
    let managed = f.load(0).await.unwrap().unwrap();
    let runtime = managed.inspect_runtime().unwrap();
    assert_eq!(
        runtime.subagent_registry().unwrap().unproven_settlements(),
        vec![activation]
    );
    let revision = deletion_revision(&f, 0).await;
    assert!(
        f.manager
            .delete_session(&session.id, &revision)
            .await
            .is_err()
    );
    assert!(!runtime.is_quiescent());
    assert!(managed.inspect_runtime().is_some());
    assert!(f.manager.sessions.read_session(&session.id).await.is_ok());
    assert!(
        f.manager
            .registry
            .0
            .lock()
            .unwrap()
            .retiring_sessions
            .contains(&session.id)
    );
    drop(owner);
}
