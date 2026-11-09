//! Thread status reads and instance notifications observe native owners only.
use super::*;
use crate::app_server::thread_status::ThreadStatus;
use crate::local_runtime::session_runtime_manager::ResidencyState;

pub(super) async fn read(
    connection: &AppServerConnection,
    id: &crate::runtime::identity::ConversationId,
) -> crate::app_server::thread_status::Thread {
    loop {
        let response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(700),
                call: Method::ThreadRead {
                    thread_id: id.clone(),
                },
            })
            .await;
        match response {
            Response::Success(response) => {
                let MethodResult::Thread { thread } = response.result else {
                    panic!("Thread read");
                };
                return thread;
            }
            Response::Failure(Failure {
                error:
                    RpcError {
                        data: Some(ErrorData::ObservationUnavailable { .. }),
                        ..
                    },
                ..
            }) => {
                // A transient observer/source lock is a valid public refusal.
                // Native recovery publishes a fresh proof even at the same level.
                loop {
                    if let NotificationMethod::ThreadStatusChanged(entry) =
                        connection.next_notification().await.notification
                        && &entry.thread_id == id
                    {
                        break;
                    }
                }
            }
            other @ Response::Failure(_) => panic!("Thread read: {other:?}"),
        }
    }
}
pub(super) async fn next_status(
    connection: &AppServerConnection,
    id: &crate::runtime::identity::ConversationId,
    status: &ThreadStatus,
) -> crate::app_server::thread_status::ThreadStatusEntry {
    loop {
        let notification = connection.next_notification().await;
        if let NotificationMethod::ThreadStatusChanged(entry) = notification.notification
            && &entry.thread_id == id
            && &entry.status == status
        {
            return entry;
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thread_status_cold_reads_running_settlement_and_unload_need_no_attachment() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let id = f.id(0).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        assert_eq!(read(&connection, &id).await.status, ThreadStatus::NotLoaded);
        let MethodResult::Threads { data, .. } = call(
            &connection,
            701,
            Method::ThreadList {
                offset: 0,
                limit: 64,
                archived: false,
            },
        )
        .await
        else {
            panic!("Thread page")
        };
        assert_eq!(data[0].status, ThreadStatus::NotLoaded);
        let MethodResult::LoadedThreads { data, .. } = call(
            &connection,
            702,
            Method::ThreadLoadedList {
                offset: 0,
                limit: 64,
            },
        )
        .await
        else {
            panic!("resident page")
        };
        assert!(data.is_empty());
        assert!(f.manager.is_empty());
        assert!(f.provider.request_bodies().is_empty());
        assert_eq!(
            f.manager
                .probe(&id)
                .compositions
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let managed = f.load(0).await.unwrap().unwrap();
        let idle = next_status(&connection, &id, &ThreadStatus::Idle).await;
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(read(&connection, &id).await.status, ThreadStatus::Idle);
        let runtime = managed.inspect_runtime().unwrap();
        runtime.submit_inbound(input("status work")).unwrap();
        f.gates[0].wait_entered().await;
        let active = ThreadStatus::Active {
            active_flags: vec![],
        };
        let running = next_status(&connection, &id, &active).await;
        assert!(running.revision > idle.revision);
        assert_eq!(read(&connection, &id).await.status, active);
        let mut changes = f.manager.status_notifications();
        for _ in 0..20 {
            assert_eq!(read(&connection, &id).await.status, active);
        }
        assert!(
            matches!(
                changes.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ),
            "identical reads and provider deltas do not publish status changes"
        );
        let done = runtime.settlement_signal().notified();
        f.gates[0].release();
        done.await;
        let settled = next_status(&connection, &id, &ThreadStatus::Idle).await;
        assert!(settled.revision > running.revision);
        // Losing the only instance observer has no execution authority.
        drop(connection);
        drop(runtime);
        drop(managed);
        f.manager.unload(&id).await.unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let cold = read(&connection, &id).await;
        assert_eq!(cold.status, ThreadStatus::NotLoaded);
        assert!(cold.status_revision > settled.revision);
        assert_eq!(connection.attachment_counts(), (0, 0));
        let restored = f.load(0).await.unwrap().unwrap();
        let restored_status = read(&connection, &id).await;
        assert_eq!(restored_status.status, ThreadStatus::Idle);
        assert_ne!(
            restored_status.runtime_incarnation,
            settled.runtime_incarnation
        );
        assert!(restored_status.status_revision > cold.status_revision);
        drop(restored);
        f.manager.unload(&id).await.unwrap();
        assert!(
            f.manager.is_empty(),
            "lightweight observation does not pin residency"
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thread_status_native_fault_is_absorbing_and_cannot_be_cleared_by_reads() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let id = f.id(0).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let managed = f.load(0).await.unwrap().unwrap();
        let runtime = managed.inspect_runtime().unwrap();
        assert_eq!(read(&connection, &id).await.status, ThreadStatus::Idle);
        runtime.force_durability_failure_for_test(
            crate::runtime::types::DurableOperation::BackgroundTerminalPublication,
            "settlement storage failed",
        );
        next_status(&connection, &id, &ThreadStatus::SystemError).await;
        for _ in 0..3 {
            assert_eq!(
                read(&connection, &id).await.status,
                ThreadStatus::SystemError
            );
        }
        assert!(
            f.manager.unload(&id).await.is_err(),
            "the semantic fault is reported even when physical retirement succeeds"
        );
        assert_eq!(
            read(&connection, &id).await.status,
            ThreadStatus::NotLoaded,
            "proven physical retirement releases residency despite the failed Turn/runtime outcome"
        );
        let restored = f.load(0).await.unwrap().unwrap();
        assert_eq!(
            read(&connection, &id).await.status,
            ThreadStatus::Idle,
            "native recomposition owns a fresh healthy frontier"
        );
        drop(restored);
        f.close().await;
        drop(runtime);
        drop(managed);
        drop(connection);
    })
    .await;
}

/// A metadata-only lock holder emits no native activity watch notification.
/// The failed public capture must arrange recovery without polling or blocking
/// the Coordinator, and metadata pages must remain independently readable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thread_status_busy_metadata_lock_recovers_without_a_semantic_wake() {
    for source in 0..4 {
        bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        use crate::local_runtime::session_runtime_manager::status::ThreadStatusEvent;
        let f = Fixture::with_session_count(None, 1).await;
        let id = f.id(0).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let managed = f.load(0).await.unwrap().unwrap();
        let before = read(&connection, &id).await;
        assert_eq!(before.status, ThreadStatus::Idle);
        let mut changes = f.manager.status_notifications();
        let gate = std::sync::Arc::new(Gate::default());
        let release = gate.arm_scoped();
        let holding = gate.clone();
        let native = managed.inspect_runtime().unwrap();
        let resident = {
            let registry=f.manager.registry.0.lock().unwrap();
            let Some(crate::local_runtime::session_runtime_manager::Entry::Loaded(resident))=registry.entries.get(&id) else {panic!("resident");};
            resident.clone()
        };
        let held = tokio::task::spawn_blocking(move || match source {
            0=>native.tool_runtime().background().with_activity_lock_for_test(|| holding.enter()),
            1=>native.with_activity_coordinator_lock_for_test(|| holding.enter()),
            2=>{let guard=resident.status_read.lock().unwrap();holding.enter();drop(guard);},
            3=>{let guard=resident.composition.lock().unwrap();holding.enter();drop(guard);},
            _=>unreachable!(),
        });
        super::super::gate_entered(&gate).await;
        let unavailable=connection.handle_request(Request {jsonrpc:JsonRpcVersion::V2,id:RequestId::Integer(704),call:Method::ThreadRead {thread_id:id.clone()}}).await;
        assert!(matches!(unavailable,Response::Failure(Failure {error:RpcError {data:Some(ErrorData::ObservationUnavailable {..}),..},..})));
        loop {
            if matches!(changes.recv().await.unwrap(),ThreadStatusEvent::ResyncRequired) {break;}
        }
        let MethodResult::Sessions {sessions,..} = call(&connection,703,Method::SessionList {query:None,offset:0,limit:32}).await else {panic!("metadata page");};
        assert_eq!(sessions.len(),1);
        assert_eq!(sessions[0].thread_id,id);
        assert!(sessions[0].thread_status.is_none());
        assert_eq!(connection.attachment_counts(),(0,0));
        assert!(f.provider.request_bodies().is_empty());
        drop(release);
        held.await.unwrap();
        loop {
            if let ThreadStatusEvent::Changed(status)=changes.recv().await.unwrap()
                && status.thread_id==id
                && status.status==crate::local_runtime::session_runtime_manager::status::ThreadRuntimeStatus::Idle {
                assert!(status.revision>before.status_revision);
                break;
            }
        }
        assert_eq!(read(&connection,&id).await.status,ThreadStatus::Idle);
        drop(managed);
        f.close().await;
    }).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn archive_idle_probe_never_holds_registry_while_another_thread_is_observed() {
    bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        let f = Fixture::with_session_count(None, 2).await;
        let a_id = f.id(0).await;
        let b_id = f.id(1).await;
        let probe = f.manager.probe(&a_id);
        // Arm before observer startup so even an initial recovery cannot pass
        // the gate and become a later unaccounted Coordinator lock contender.
        probe.status_before_recovery.arm();
        let a = f.load(0).await.unwrap().unwrap();
        let b = f.load(1).await.unwrap().unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        assert_eq!(read(&connection, &a_id).await.status, ThreadStatus::Idle);
        assert_eq!(read(&connection, &b_id).await.status, ThreadStatus::Idle);

        let native = a.inspect_runtime().unwrap();
        let admission = std::sync::Arc::new(Gate::default());
        native.install_admission_gate(admission.clone());
        let release_admission = admission.arm_scoped();
        native.wake_admission_for_test();
        super::super::gate_entered(&admission).await;
        probe.before_shutdown.arm();
        let gate = std::sync::Arc::new(Gate::default());
        let release_native = gate.arm_scoped();
        let holding = gate.clone();
        let held = tokio::task::spawn_blocking(move || {
            native.with_activity_coordinator_lock_for_test(|| holding.enter());
        });
        super::super::gate_entered(&gate).await;
        let release_archive = probe.archive_before_idle.arm_scoped();
        let manager = f.manager.clone();
        let id = a_id.clone();
        let archiving = tokio::spawn(async move { manager.archive_thread(id, true).await });
        super::super::gate_entered(&probe.archive_before_idle).await;

        // Both A's Coordinator and the actual archive probe boundary remain
        // parked until these unrelated strong and metadata reads complete.
        assert_eq!(read(&connection, &b_id).await.status, ThreadStatus::Idle);
        let MethodResult::Sessions { sessions, .. } = call(
            &connection,
            705,
            Method::SessionList {
                query: None,
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("Session metadata page");
        };
        assert_eq!(sessions.len(), 2);
        assert!(
            sessions
                .iter()
                .find(|row| row.thread_id == a_id)
                .unwrap()
                .thread_status
                .is_none()
        );
        assert_eq!(
            sessions
                .iter()
                .find(|row| row.thread_id == b_id)
                .unwrap()
                .thread_status
                .as_ref()
                .unwrap()
                .status,
            ThreadStatus::Idle
        );
        assert!(!archiving.is_finished());
        assert!(f.provider.request_bodies().is_empty());
        // The deliberate Unavailable observation wakes a recovery reader.
        // Keep it and the empty admission pass outside Coordinator while the
        // first archive claim runs; contention may legitimately refuse a claim.
        probe.status_before_recovery.entered().await;
        drop(release_native);
        held.await.unwrap();
        drop(release_archive);
        probe.before_shutdown.entered().await;
        drop(release_admission);
        probe.status_before_recovery.release();
        probe.before_shutdown.release();
        assert!(archiving.await.unwrap().unwrap().archived);
        assert_eq!(read(&connection, &b_id).await.status, ThreadStatus::Idle);
        drop((a, b));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn archive_rechecks_the_exact_resident_and_native_admission_epoch() {
    for competitor in 0..3 {
        bounded(async {
            let f = Fixture::with_session_count(None, 1).await;
            let id = f.id(0).await;
            let a = f.load(0).await.unwrap().unwrap();
            let probe = f.manager.probe(&id);
            let release = probe.archive_before_claim.arm_scoped();
            let manager = f.manager.clone();
            let archiving_id = id.clone();
            let archiving =
                tokio::spawn(async move { manager.archive_thread(archiving_id, true).await });
            super::super::gate_entered(&probe.archive_before_claim).await;
            let replacement = if competitor == 2 {
                f.manager.unload(&id).await.unwrap();
                Some(f.load(0).await.unwrap().unwrap())
            } else if competitor == 0 {
                a.client()
                    .submit_inbound(input("archive epoch competitor"))
                    .unwrap();
                f.gates[0].wait_entered().await;
                None
            } else {
                let native = a.inspect_runtime().unwrap();
                let lifecycle = native.lifecycle_for_test();
                let previous = lifecycle.idle_epoch().unwrap();
                drop(lifecycle.try_enter_running().unwrap());
                lifecycle.wait_for_no_active_operations().await;
                assert!(lifecycle.idle_epoch().unwrap() > previous);
                assert!(!native.has_current_attempt());
                None
            };
            drop(release);
            let failure = archiving.await.unwrap().unwrap_err().to_string();
            assert!(
                failure.contains(if competitor == 2 {
                    "residency changed"
                } else {
                    "admission changed"
                }),
                "{failure}"
            );
            assert!(!f.manager.sessions.thread(&id).await.unwrap().archived);
            assert_eq!(f.manager.residency(&id), ResidencyState::Loaded);
            f.gates[0].release();
            drop((a, replacement));
            f.close().await;
        })
        .await;
    }
}
