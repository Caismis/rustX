//! Scheduler commands exercise the real native manager, writer and retirement gates.
#![allow(clippy::too_many_lines, clippy::large_futures)]
use super::*;
use crate::durable::{InboxStore, ScheduleStore};
use crate::runtime::types::RuntimeClock;
use crate::scheduler::*;
use chrono::{DateTime, Duration, Utc};
#[derive(Debug)]
struct Clock(Mutex<DateTime<Utc>>);
impl Clock {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(
            DateTime::parse_from_rfc3339("2026-10-23T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        )))
    }
    fn add(&self, seconds: i64) {
        *self.0.lock().unwrap() += Duration::seconds(seconds);
    }
}
impl RuntimeClock for Clock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}
fn create(key: &str) -> ScheduleCommand {
    ScheduleCommand {
        command_id: key.into(),
        mutation: ScheduleMutation::Create {
            title: "Check".into(),
            text: "check result".into(),
            timing: ScheduleTiming::After { seconds: 60 },
        },
    }
}
fn start(
    f: &Fixture,
    index: usize,
    key: &str,
) -> tokio::task::JoinHandle<Result<ScheduleCommandResult, RuntimeManagerError>> {
    let manager = f.manager.clone();
    let session = f.sessions[index].clone();
    let command = create(key);
    tokio::spawn(async move {
        manager
            .schedule_command(&session.id, &session.active_node, command)
            .await
    })
}
fn receipt(result: ScheduleCommandResult) -> ScheduleCommandReceipt {
    let ScheduleCommandResult::Applied { receipt } = result else {
        panic!("{result:?}")
    };
    receipt
}
async fn pending(f: &Fixture, session: &SessionSnapshot) -> usize {
    let access = f
        .manager
        .sessions
        .acquire_session(&session.id, Some(&session.active_node))
        .await
        .unwrap();
    crate::durable::SqliteConversationStore::open_existing(
        access.node.conversation_id.clone(),
        &access.database_path,
    )
    .unwrap()
    .load_pending()
    .unwrap()
    .len()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_one_shot_uses_one_durable_inbox_without_composing_an_agent() {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 1).await;
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();
        let created = receipt(start(&f, 0, "one").await.unwrap().unwrap());
        let id = f.id(0).await;
        assert_eq!(f.manager.probe(&id).compositions.load(Ordering::SeqCst), 0);
        clock.add(60);
        let session = &f.sessions[0];
        let result = f
            .manager
            .schedule_operation(
                &session.id,
                &session.active_node,
                ScheduleOperation::Admit {
                    id: created.id.clone(),
                    expected_revision: 1,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            result,
            ScheduleOperationResult::Admission(ScheduleAdmission::Admitted { .. })
        ));
        assert_eq!(pending(&f, session).await, 1);
        assert!(f.manager.is_empty());
        let retry = f
            .manager
            .schedule_operation(
                &session.id,
                &session.active_node,
                ScheduleOperation::Admit {
                    id: created.id,
                    expected_revision: 1,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            retry,
            ScheduleOperationResult::Admission(ScheduleAdmission::AlreadyAdvanced { .. })
        ));
        assert_eq!(pending(&f, session).await, 1);
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_registration_wins_load_and_caller_cancellation_keeps_the_flight_owned() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        probe.storage_before_close.arm();
        let command = start(&f, 0, "stable");
        probe.storage_before_close.entered().await;
        assert_eq!(f.manager.diagnostics().storage_only, 1);
        command.abort();
        assert!(command.await.unwrap_err().is_cancelled());
        let load = f.load(0);
        probe.joined(1).await;
        assert!(!load.is_finished());
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 0);
        probe.storage_before_close.release();
        let loaded = load.await.unwrap().unwrap();
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        let result = receipt(start(&f, 0, "stable").await.unwrap().unwrap());
        assert_eq!(result.id.ordinal, 1);
        assert_eq!(
            f.manager
                .schedule_list(&f.sessions[0].id, &f.sessions[0].active_node, 0, 100)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(loaded.client().validate().is_ok());
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn load_registration_wins_and_storage_reselects_the_resident_owner() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        probe.before_compose.arm();
        let load = f.load(0);
        probe.before_compose.entered().await;
        let command = start(&f, 0, "one");
        probe
            .storage_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        assert!(!command.is_finished());
        probe.before_compose.release();
        let loaded = load.await.unwrap().unwrap();
        receipt(command.await.unwrap().unwrap());
        assert_eq!(f.manager.diagnostics().storage_only, 0);
        assert!(loaded.client().validate().is_ok());
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn storage_commands_join_release_without_reusing_another_commands_result() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let probe = f.manager.probe(&f.id(0).await);
        probe.storage_before_close.arm();
        let first = start(&f, 0, "first");
        probe.storage_before_close.entered().await;
        let second = start(&f, 0, "second");
        probe
            .storage_joined
            .subscribe()
            .wait_for(|n| *n == 1)
            .await
            .unwrap();
        probe.storage_before_close.release();
        let a = receipt(first.await.unwrap().unwrap());
        let b = receipt(second.await.unwrap().unwrap());
        assert_eq!(a.id.ordinal, 1);
        assert_eq!(b.id.ordinal, 2);
        assert!(f.manager.is_empty());
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_nodes_residency_does_not_refuse_cold_admission_or_change_selection() {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 1).await;
        let branch = super::session_claim::second_node(&f).await;
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();
        {
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .policy
                .max_resident_runtimes = 1;
        }
        let b = f
            .manager
            .load(&branch.id, Some(&branch.active_node))
            .await
            .unwrap();
        let original = f.sessions[0].clone();
        let created = receipt(start(&f, 0, "one").await.unwrap().unwrap());
        clock.add(60);
        let result = f
            .manager
            .schedule_operation(
                &original.id,
                &original.active_node,
                ScheduleOperation::Admit {
                    id: created.id,
                    expected_revision: 1,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            result,
            ScheduleOperationResult::Admission(ScheduleAdmission::Admitted { .. })
        ));
        assert_eq!(pending(&f, &original).await, 1);
        assert!(b.client().validate().is_ok());
        assert!(matches!(
            f.manager
                .load(&original.id, Some(&original.active_node))
                .await,
            Err(RuntimeManagerError::SessionAlreadyResident { .. })
        ));
        assert_eq!(
            f.manager
                .sessions
                .read_session(&original.id)
                .await
                .unwrap()
                .active_node,
            original.active_node
        );
        f.manager.unload(b.conversation_id()).await.unwrap();
        let a = f
            .manager
            .load(&original.id, Some(&original.active_node))
            .await
            .unwrap();
        f.gates[0].wait_entered().await;
        assert_eq!(pending(&f, &original).await, 0);
        assert!(a.client().validate().is_ok());
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retirement_before_final_commit_refuses_mutation_and_joins_cold_allocation() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::with_session_count(None, 1).await;
        let revision = super::deletion::deletion_revision(&f, 0).await;
        let probe = f.manager.probe(&f.id(0).await);
        probe.storage_before_commit.arm();
        let command = start(&f, 0, "one");
        probe.storage_before_commit.entered().await;
        probe.after_delete_fence.arm();
        let delete = super::deletion::delete_task(&f, 0, revision);
        probe.after_delete_fence.entered().await;
        probe.storage_before_commit.release();
        assert!(command.await.unwrap().is_err());
        probe.after_delete_fence.release();
        assert!(matches!(
            delete.await.unwrap().unwrap(),
            SessionDeleteResult::Deleted { .. }
        ));
        assert!(f.manager.is_empty());
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_enabled_schedule_blocks_deletion_without_unloading_another_owner() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let revision = super::deletion::deletion_revision(&f, 0).await;
        let b = f.load(1).await.unwrap().unwrap();
        let probe = f.manager.probe(&f.id(0).await);
        probe.storage_after_commit.arm();
        let command = start(&f, 0, "one");
        probe.storage_after_commit.entered().await;
        assert!(matches!(
            f.manager
                .delete_session(&f.sessions[0].id, &revision)
                .await
                .unwrap(),
            SessionDeleteResult::Blocked { .. }
        ));
        assert!(b.client().validate().is_ok());
        probe.storage_after_commit.release();
        receipt(command.await.unwrap().unwrap());
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_drain_fences_cold_commit_and_waits_for_physical_release() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let probe = f.manager.probe(&f.id(0).await);
        probe.storage_before_commit.arm();
        let command = start(&f, 0, "one");
        probe.storage_before_commit.entered().await;
        f.host.begin_drain();
        let host = f.host.clone();
        let drain = tokio::spawn(async move { host.drain().await });
        assert!(!drain.is_finished());
        probe.storage_before_commit.release();
        assert!(command.await.unwrap().is_err());
        assert!(drain.await.unwrap().is_empty());
        assert!(f.manager.is_empty());
        assert_eq!(pending(&f, &f.sessions[0]).await, 0);
        assert_eq!(f.manager.diagnostics().schedule_commits, 0);
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_close_keeps_the_writer_reserved_but_preserves_the_committed_receipt() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        probe.fail_storage_close_once.store(true, Ordering::SeqCst);
        let created = receipt(start(&f, 0, "one").await.unwrap().unwrap());
        assert_eq!(created.id.ordinal, 1);
        assert_eq!(f.manager.diagnostics().storage_only, 1);
        assert!(f.load(0).await.unwrap().is_err());
        assert!(f.manager.unload(&id).await.is_err());
        let access = f
            .manager
            .sessions
            .acquire_session(&f.sessions[0].id, None)
            .await
            .unwrap();
        let store =
            crate::durable::SqliteConversationStore::open_existing(id, &access.database_path)
                .unwrap();
        assert_eq!(
            store.schedule_get(&created.id).unwrap().unwrap().revision,
            1
        );
        assert!(!f.manager.is_empty());
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn panicked_storage_operation_rolls_back_and_proves_writer_release() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let probe = f.manager.probe(&f.id(0).await);
        probe.panic_storage_once.store(true, Ordering::SeqCst);
        assert!(start(&f, 0, "one").await.unwrap().is_err());
        assert!(f.manager.is_empty());
        assert_eq!(
            receipt(start(&f, 0, "one").await.unwrap().unwrap())
                .id
                .ordinal,
            1
        );
        f.close().await;
    })
    .await;
}

fn driver(f: &Fixture) -> tokio::task::JoinHandle<Result<(), RuntimeManagerError>> {
    let manager = f.manager.clone();
    tokio::spawn(async move { manager.run_schedule_driver().await })
}
fn manual_driver(
    f: &mut Fixture,
) -> (
    Arc<Clock>,
    Arc<crate::runtime::monotonic::ManualMonotonicClock>,
    watch::Receiver<u64>,
) {
    let utc = Clock::new();
    let monotonic = Arc::new(crate::runtime::monotonic::ManualMonotonicClock::new());
    f.manager.schedule_clock = utc.clone();
    f.manager.clock = monotonic.clone();
    let (waiting, receiver) = watch::channel(u64::MAX);
    f.manager.registry.0.lock().unwrap().schedule_waiting = Some(waiting);
    (utc, monotonic, receiver)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn driver_scans_empty_inventory_without_activating_ordinary_runtime_or_human_pending() {
    bounded(async {
        let mut f = Fixture::new().await;
        let (_utc, _monotonic, mut waiting) = manual_driver(&mut f);
        for (index, source) in [
            (0, crate::message::types::UserSource::Human),
            (1, crate::message::types::UserSource::Runtime),
        ] {
            let session = &f.sessions[index];
            let access = f
                .manager
                .sessions
                .acquire_session(&session.id, None)
                .await
                .unwrap();
            let store = crate::durable::SqliteConversationStore::open(
                access.node.conversation_id.clone(),
                &access.database_path,
            )
            .unwrap();
            store
                .accept_inbound(crate::durable::InboundDraft {
                    message_id: None,
                    source,
                    kind: crate::message::types::InboundKind::Message,
                    content: input("scheduler: spoofed text"),
                    timestamp: Utc::now(),
                    correlation: None,
                })
                .unwrap();
        }
        let task = driver(&f);
        waiting
            .wait_for(|deadline| *deadline == 30_000)
            .await
            .unwrap();
        assert_eq!(f.manager.schedule_driver_diagnostics().stores_visited, 2);
        for index in 0..2 {
            assert_eq!(
                f.manager
                    .probe(&f.id(index).await)
                    .compositions
                    .load(Ordering::SeqCst),
                0
            );
            assert_eq!(pending(&f, &f.sessions[index]).await, 1);
        }
        assert!(f.manager.run_schedule_driver().await.is_err());
        f.host.begin_drain();
        assert!(task.await.unwrap().is_ok());
        assert!(f.manager.run_schedule_driver().await.is_ok());
        assert!(f.host.drain().await.is_empty());
        assert!(f.manager.is_empty());
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_creation_hint_is_reconciled_from_an_empty_index_with_controlled_clocks() {
    bounded(async {
        use futures_util::FutureExt;
        let mut f = Fixture::with_session_count(None, 1).await;
        let (utc, monotonic, mut waiting) = manual_driver(&mut f);
        let scan_done = f
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .schedule_scan_done
            .clone();
        scan_done.arm();
        let task = driver(&f);
        scan_done.entered().await;
        let created = receipt(start(&f, 0, "lost").await.unwrap().unwrap());
        assert!(
            f.manager
                .schedule_changed
                .notified()
                .now_or_never()
                .is_some()
        );
        f.manager.schedule_dirty.lock().unwrap().clear();
        scan_done.release();
        waiting
            .wait_for(|deadline| *deadline == 30_000)
            .await
            .unwrap();
        utc.add(120);
        monotonic.advance(30_000);
        f.gates[0].wait_entered().await;
        let schedule = f
            .manager
            .schedule_get(&f.sessions[0].id, &f.sessions[0].active_node, created.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(schedule.state, ScheduleState::Completed);
        assert_eq!(schedule.revision, 2);
        assert!(f.manager.schedule_driver_diagnostics().passes_completed >= 2);
        f.host.begin_drain();
        f.gates[0].release();
        assert!(task.await.unwrap().is_ok());
        assert!(f.host.drain().await.is_empty());
        assert!(f.manager.is_empty());
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activation_rechecks_adopted_native_target_before_composition() {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 1).await;
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();
        let created = receipt(start(&f, 0, "one").await.unwrap().unwrap());
        clock.add(60);
        let session = &f.sessions[0];
        let ScheduleOperationResult::Admission(ScheduleAdmission::Admitted { snapshot, inbound }) =
            f.manager
                .schedule_operation(
                    &session.id,
                    &session.active_node,
                    ScheduleOperation::Admit {
                        id: created.id,
                        expected_revision: 1,
                    },
                )
                .await
                .unwrap()
        else {
            panic!()
        };
        let access = f
            .manager
            .sessions
            .acquire_session(&session.id, None)
            .await
            .unwrap();
        let store = crate::durable::SqliteConversationStore::open(
            access.node.conversation_id.clone(),
            &access.database_path,
        )
        .unwrap();
        store.adopt_pending_batch(inbound.sequence, None).unwrap();
        drop(store);
        drop(access);
        f.manager
            .activate_scheduled_target(
                &snapshot.binding,
                &snapshot.last_trigger.unwrap().trigger_id,
            )
            .await
            .unwrap();
        assert!(f.manager.is_empty());
        assert_eq!(
            f.manager
                .probe(&session.active_conversation_id)
                .compositions
                .load(Ordering::SeqCst),
            0
        );
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fork_does_not_copy_schedules_command_receipts_or_temporal_identity() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let original = receipt(start(&f, 0, "stable").await.unwrap().unwrap());
        let source = &f.sessions[0];
        let cloned = f
            .manager
            .sessions
            .clone_session(
                &source.id,
                Some(&source.active_node),
                crate::conversation::SurfaceRevision::new(0),
            )
            .await
            .unwrap()
            .session;
        assert!(
            f.manager
                .schedule_list(&cloned.id, &cloned.active_node, 0, 100)
                .await
                .unwrap()
                .is_empty()
        );
        let result = receipt(
            f.manager
                .schedule_command(&cloned.id, &cloned.active_node, create("stable"))
                .await
                .unwrap(),
        );
        assert_eq!(result.id.ordinal, 1);
        assert_ne!(result.id, original.id);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn required_write_failure_fences_retries_until_explicit_native_recovery() {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 1).await;
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();
        let created = receipt(start(&f, 0, "one").await.unwrap().unwrap());
        clock.add(60);
        let session = &f.sessions[0];
        let operation = ScheduleOperation::Admit {
            id: created.id,
            expected_revision: 1,
        };
        f.manager
            .probe(&session.active_conversation_id)
            .fail_storage_accept_once
            .store(true, Ordering::SeqCst);
        assert!(matches!(
            f.manager
                .schedule_operation(&session.id, &session.active_node, operation.clone())
                .await,
            Err(RuntimeManagerError::ScheduleStore(_))
        ));
        assert_eq!(pending(&f, session).await, 0);
        assert_eq!(
            f.manager.diagnostics().schedule_write_fenced,
            vec![session.active_conversation_id.clone()]
        );
        assert!(
            f.manager
                .schedule_operation(&session.id, &session.active_node, operation.clone())
                .await
                .unwrap_err()
                .to_string()
                .contains("recovery")
        );
        let summary = f
            .manager
            .recover_schedule_storage(&session.id, &session.active_node)
            .await
            .unwrap();
        assert!(summary.pending.is_empty());
        assert!(f.manager.diagnostics().schedule_write_fenced.is_empty());
        assert!(matches!(
            f.manager
                .schedule_operation(&session.id, &session.active_node, operation)
                .await
                .unwrap(),
            ScheduleOperationResult::Admission(ScheduleAdmission::Admitted { .. })
        ));
        assert_eq!(pending(&f, session).await, 1);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn utc_rollback_during_cold_acquisition_cannot_admit_an_early_occurrence() {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 1).await;
        let clock = Clock::new();
        f.manager.schedule_clock = clock.clone();
        let created = receipt(start(&f, 0, "one").await.unwrap().unwrap());
        clock.add(60);
        let session = f.sessions[0].clone();
        let probe = f.manager.probe(&session.active_conversation_id);
        probe.storage_before_commit.arm();
        let operation = ScheduleOperation::Admit {
            id: created.id.clone(),
            expected_revision: 1,
        };
        let manager = f.manager.clone();
        let task_operation = operation.clone();
        let task_session = session.clone();
        let task = tokio::spawn(async move {
            manager
                .schedule_operation(&task_session.id, &task_session.active_node, task_operation)
                .await
        });
        probe.storage_before_commit.entered().await;
        clock.add(-120);
        probe.storage_before_commit.release();
        assert!(matches!(
            task.await.unwrap().unwrap(),
            ScheduleOperationResult::Admission(ScheduleAdmission::NotDue)
        ));
        assert_eq!(pending(&f, &session).await, 0);
        assert_eq!(
            f.manager
                .schedule_get(&session.id, &session.active_node, created.id)
                .await
                .unwrap()
                .unwrap()
                .revision,
            1
        );
        clock.add(120);
        assert!(matches!(
            f.manager
                .schedule_operation(&session.id, &session.active_node, operation)
                .await
                .unwrap(),
            ScheduleOperationResult::Admission(ScheduleAdmission::Admitted { .. })
        ));
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hot_schedule_operation_lease_is_joined_before_runtime_writer_retirement() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let loaded = f.load(0).await.unwrap().unwrap();
        let id = loaded.conversation_id().clone();
        let probe = f.manager.probe(&id);
        probe.before_operation.arm();
        let command = start(&f, 0, "one");
        probe.before_operation.entered().await;
        let unload = unload_task(&f, id.clone());
        probe
            .draining_operations
            .subscribe()
            .wait_for(|draining| *draining)
            .await
            .unwrap();
        assert!(!unload.is_finished());
        probe.before_operation.release();
        let created = receipt(command.await.unwrap().unwrap());
        unload.await.unwrap().unwrap();
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        assert_eq!(
            f.manager
                .schedule_get(&f.sessions[0].id, &f.sessions[0].active_node, created.id)
                .await
                .unwrap()
                .unwrap()
                .revision,
            1
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_scan_skips_a_stalled_reader_and_drain_retains_its_physical_tail() {
    bounded(async {
        let mut f = Fixture::new().await;
        let (utc, monotonic, _waiting) = manual_driver(&mut f);
        let a = f.sessions[0].active_conversation_id.clone();
        let probe = f.manager.probe(&a);
        probe.schedule_native_read.arm();
        receipt(start(&f, 1, "due-b").await.unwrap().unwrap());
        utc.add(60);
        let task = driver(&f);
        probe.schedule_native_read.entered().await;
        monotonic.advance(super::super::schedule_driver::READ_MS);
        f.gates[0].wait_entered().await;
        let health = f.manager.schedule_driver_diagnostics();
        assert!(!health.sources[&a].available);
        assert!(
            health.sources[&a]
                .error
                .as_ref()
                .unwrap()
                .contains("budget")
        );
        assert_eq!(health.scanned_in_pass, 2);
        assert_eq!(health.pass_overruns, 0);
        assert_eq!(health.discovery_bound_ms, health.pass_bound_ms * 2 + 30_000);
        assert!(health.last_pass_ms <= health.pass_bound_ms);
        f.host.begin_drain();
        assert!(task.await.unwrap().is_ok());
        assert_eq!(f.manager.diagnostics().schedule_reads, 1);
        assert!(!f.manager.is_empty());
        probe.schedule_native_read.release();
        f.gates[0].release();
        assert!(f.host.drain().await.is_empty());
        assert!(f.manager.is_empty());
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unavailable_source_keeps_last_success_and_rejoins_after_repair() {
    bounded(async {
        let mut f = Fixture::new().await;
        let (utc, monotonic, mut waiting) = manual_driver(&mut f);
        let a = f.sessions[0].active_conversation_id.clone();
        receipt(start(&f, 0, "one").await.unwrap().unwrap());
        let task = driver(&f);
        waiting.wait_for(|d| *d == 30_000).await.unwrap();
        let before = f.manager.schedule_driver_diagnostics().sources[&a].clone();
        assert_eq!(before.last_generation, Some(1));
        let probe = f.manager.probe(&a);
        probe.schedule_native_read.arm();
        monotonic.advance(30_000);
        probe.schedule_native_read.entered().await;
        monotonic.advance(super::super::schedule_driver::READ_MS);
        waiting.wait_for(|d| *d == 61_000).await.unwrap();
        let failed = f.manager.schedule_driver_diagnostics();
        assert_eq!(failed.indexed_conversations, 2);
        assert!(!failed.sources[&a].available);
        assert_eq!(failed.sources[&a].last_success_utc, before.last_success_utc);
        assert_eq!(failed.sources[&a].last_generation, Some(1));
        probe.schedule_native_read.release();
        let mut readers = {
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .schedule_reads
                .subscribe()
        };
        readers.wait_for(|n| *n == 0).await.unwrap();
        utc.add(1);
        monotonic.advance(30_000);
        waiting.wait_for(|d| *d == 91_000).await.unwrap();
        let repaired = f.manager.schedule_driver_diagnostics();
        assert!(repaired.sources[&a].available);
        assert!(repaired.sources[&a].error.is_none());
        assert_eq!(repaired.sources[&a].last_success_utc, Some(utc.now()));
        f.host.begin_drain();
        task.await.unwrap().unwrap();
        assert!(f.host.drain().await.is_empty());
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abandoned_old_scan_cannot_overwrite_a_newer_dirty_observation() {
    bounded(async {
        let mut f = Fixture::new().await;
        let (_utc, monotonic, mut waiting) = manual_driver(&mut f);
        let created = receipt(start(&f, 0, "one").await.unwrap().unwrap());
        let a = f.sessions[0].active_conversation_id.clone();
        let probe = f.manager.probe(&a);
        probe.schedule_read_return.arm();
        probe
            .pause_schedule_read_return
            .store(true, Ordering::SeqCst);
        let task = driver(&f);
        probe.schedule_read_return.entered().await;
        monotonic.advance(super::super::schedule_driver::READ_MS);
        waiting.wait_for(|d| *d == 31_000).await.unwrap();
        let command = ScheduleCommand {
            command_id: "disable".into(),
            mutation: ScheduleMutation::SetEnabled {
                id: created.id,
                expected_revision: 1,
                enabled: false,
            },
        };
        f.manager
            .schedule_command(&f.sessions[0].id, &f.sessions[0].active_node, command)
            .await
            .unwrap();
        waiting
            .wait_for(|_| {
                f.manager.schedule_driver_diagnostics().sources[&a].last_generation == Some(2)
            })
            .await
            .unwrap();
        probe.schedule_read_return.release();
        let mut readers = {
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .schedule_reads
                .subscribe()
        };
        readers.wait_for(|n| *n == 0).await.unwrap();
        let health = f.manager.schedule_driver_diagnostics();
        assert_eq!(health.sources[&a].last_generation, Some(2));
        assert!(health.sources[&a].next_due_utc.is_none());
        f.host.begin_drain();
        task.await.unwrap().unwrap();
        assert!(f.host.drain().await.is_empty());
        f.close().await;
    })
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dirty_hint_refreshes_only_its_conversation_without_restarting_the_full_pass() {
    bounded(async {
        let mut f = Fixture::new().await;
        let (_utc, _monotonic, mut waiting) = manual_driver(&mut f);
        let task = driver(&f);
        waiting.wait_for(|d| *d == 30_000).await.unwrap();
        let before = f.manager.schedule_driver_diagnostics();
        let a = f.sessions[0].active_conversation_id.clone();
        receipt(start(&f, 0, "one").await.unwrap().unwrap());
        waiting
            .wait_for(|_| {
                f.manager.schedule_driver_diagnostics().sources[&a].last_generation == Some(1)
            })
            .await
            .unwrap();
        let after = f.manager.schedule_driver_diagnostics();
        assert_eq!(after.passes_completed, before.passes_completed);
        assert_eq!(after.stores_visited, before.stores_visited + 1);
        f.host.begin_drain();
        task.await.unwrap().unwrap();
        assert!(f.host.drain().await.is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prepared_live_read_rechecks_tombstoned_residue_after_allocation_lock() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::new().await;
        let session = &f.sessions[0];
        let plan = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .plan_schedule_read(
                &session.id,
                &session.active_node,
                f.manager.sessions.catalog.clone(),
            )
            .unwrap();
        let database = plan.database_path.clone();
        let revision = super::deletion::deletion_revision(&f, 0).await;
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        f.manager.sessions.install_delete_cleanup_gate(gate.clone());
        let sessions = f.manager.sessions.clone();
        let id = session.id.clone();
        let deletion = tokio::spawn(async move { sessions.delete_session(&id, &revision).await });
        gate_entered(&gate).await;
        assert!(database.is_file());
        assert!(
            tokio::task::spawn_blocking(move || plan.open())
                .await
                .unwrap()
                .is_err()
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
async fn live_read_retention_excludes_destructive_allocation_and_allows_metadata_edits() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let session = &f.sessions[0];
        let plan = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .plan_schedule_read(
                &session.id,
                &session.active_node,
                f.manager.sessions.catalog.clone(),
            )
            .unwrap();
        let allocation = plan.database_path.parent().unwrap().to_owned();
        f.manager
            .sessions
            .rename_session(&session.id, "New display name")
            .await
            .unwrap();
        let access = tokio::task::spawn_blocking(move || plan.open())
            .await
            .unwrap()
            .unwrap();
        assert!(
            crate::runtime::local_storage::ConversationExclusion::acquire(
                &access.allocation,
                &allocation
            )
            .is_err()
        );
        drop(access);
        let product =
            crate::runtime::local_storage::ProductRoot::existing(&f.archive_root).unwrap();
        assert!(
            crate::runtime::local_storage::ConversationExclusion::acquire(&product, &allocation)
                .is_ok()
        );
        f.close().await;
    })
    .await;
}

mod workflow;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_review_stalled_activations_cannot_starve_a_healthy_due_source() {
    bounded(async {
        let mut f = Fixture::with_session_count(None, 17).await;
        f.manager
            .registry
            .0
            .lock()
            .unwrap()
            .policy
            .max_resident_runtimes = 32;
        let (utc, monotonic, _waiting) = manual_driver(&mut f);
        let mut created = Vec::new();
        for i in 0..17 {
            created.push(receipt(start(&f, i, "fairness").await.unwrap().unwrap()));
        }
        utc.add(60);
        let mut loads = Vec::new();
        let mut probes = Vec::new();
        for (i, created) in created.iter().take(16).enumerate() {
            f.manager
                .schedule_operation(
                    &f.sessions[i].id,
                    &f.sessions[i].active_node,
                    ScheduleOperation::Admit {
                        id: created.id.clone(),
                        expected_revision: created.revision,
                    },
                )
                .await
                .unwrap();
            let probe = f.manager.probe(&f.sessions[i].active_conversation_id);
            probe.before_compose.arm();
            loads.push(f.load(i));
            probe.before_compose.entered().await;
            probes.push(probe);
        }
        let mut activations = f
            .manager
            .registry
            .0
            .lock()
            .unwrap()
            .schedule_activation_started
            .subscribe();
        let driver = driver(&f);
        let healthy = &f.sessions[16];
        for _ in 0..17 {
            activations.changed().await.unwrap();
            drop(activations.borrow_and_update());
            let current = f
                .manager
                .schedule_get(&healthy.id, &healthy.active_node, created[16].id.clone())
                .await
                .unwrap()
                .unwrap();
            if current.state == ScheduleState::Completed {
                break;
            }
            monotonic.advance(2000);
        }
        let healthy_schedule = f
            .manager
            .schedule_get(&healthy.id, &healthy.active_node, created[16].id.clone())
            .await
            .unwrap()
            .unwrap();
        f.manager.schedule_stop.cancel();
        driver.await.unwrap().unwrap();
        for probe in probes {
            probe.before_compose.release();
        }
        for load in loads {
            load.await.unwrap().unwrap();
        }
        f.close().await;
        assert_eq!(healthy_schedule.state, ScheduleState::Completed);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler85_review_busy_hot_writer_does_not_block_activation_or_other_sessions() {
    bounded(async {
        let mut f = Fixture::new().await;
        let utc = Clock::new();
        f.manager.schedule_clock = utc.clone();
        let resident = f.load(0).await.unwrap().unwrap();
        resident
            .inspect_runtime()
            .unwrap()
            .submit_inbound(input("request-A"))
            .unwrap();
        f.gates[0].wait_entered().await;
        let created = receipt(start(&f, 0, "pending-target").await.unwrap().unwrap());
        utc.add(60);
        let ScheduleOperationResult::Admission(ScheduleAdmission::Admitted { snapshot, .. }) = f
            .manager
            .schedule_operation(
                &f.sessions[0].id,
                &f.sessions[0].active_node,
                ScheduleOperation::Admit {
                    id: created.id,
                    expected_revision: created.revision,
                },
            )
            .await
            .unwrap()
        else {
            panic!("pending native target")
        };
        assert_eq!(pending(&f, &f.sessions[0]).await, 1);
        let access = f
            .manager
            .sessions
            .acquire_session(&f.sessions[0].id, None)
            .await
            .unwrap();
        let competitor = rusqlite::Connection::open(&access.database_path).unwrap();
        competitor.execute_batch("BEGIN IMMEDIATE").unwrap();
        let probe = f.manager.probe(resident.conversation_id());
        let mut busy = probe.schedule_writer_busy.subscribe();
        let command = start(&f, 0, "busy-hot-command");
        busy.wait_for(|v| *v).await.unwrap();
        let binding = ScheduleBinding {
            session_id: f.sessions[0].id.clone(),
            node_id: f.sessions[0].active_node.clone(),
            conversation_id: resident.conversation_id().clone(),
        };
        let trigger = snapshot.last_trigger.unwrap().trigger_id;
        let manager = f.manager.clone();
        let mut activation =
            tokio::spawn(
                async move { manager.activate_scheduled_target(&binding, &trigger).await },
            );
        let mut other = f.load(1);
        let progress = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            (&mut activation).await.unwrap().unwrap();
            let other = (&mut other).await.unwrap().unwrap();
            f.manager.unload(other.conversation_id()).await.unwrap();
        })
        .await;
        competitor.execute_batch("ROLLBACK").unwrap();
        receipt(command.await.unwrap().unwrap());
        if !activation.is_finished() {
            activation.await.unwrap().unwrap();
        }
        if !other.is_finished() {
            other.await.unwrap().unwrap();
        }
        drop(access);
        f.close().await;
        assert!(
            progress.is_ok(),
            "a busy hot writer stalled native activation or another Session's admission"
        );
    })
    .await;
}
