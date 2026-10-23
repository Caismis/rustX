use super::*;
use crate::durable::{CanonicalStore, InboxStore};
use crate::runtime::identity::{ConversationId, SessionId, SessionNodeId};
use crate::runtime::types::RuntimeClock;
use std::sync::{Arc, Mutex};
#[derive(Debug)]
struct Clock(Mutex<DateTime<Utc>>);
impl Clock {
    fn new() -> Self {
        Self(Mutex::new(
            DateTime::parse_from_rfc3339("2026-10-23T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        ))
    }
    fn add(&self, seconds: i64) {
        let mut at = self.0.lock().unwrap();
        *at += Duration::seconds(seconds);
    }
}
impl RuntimeClock for Clock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}
fn fixture() -> (SqliteConversationStore, ScheduleBinding, Clock) {
    let id = ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18");
    (
        SqliteConversationStore::in_memory(id.clone()).unwrap(),
        ScheduleBinding {
            session_id: SessionId::new("ses_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            node_id: SessionNodeId::new("node_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            conversation_id: id,
        },
        Clock::new(),
    )
}
fn command(id: &str, mutation: ScheduleMutation) -> ScheduleCommand {
    ScheduleCommand {
        command_id: id.into(),
        mutation,
    }
}
fn create(
    store: &SqliteConversationStore,
    binding: &ScheduleBinding,
    clock: &Clock,
    key: &str,
    timing: ScheduleTiming,
) -> ScheduleSnapshot {
    match store
        .schedule_command(
            binding,
            &command(
                key,
                ScheduleMutation::Create {
                    title: "Reminder".into(),
                    text: "check result".into(),
                    timing,
                },
            ),
            clock,
        )
        .unwrap()
    {
        ScheduleCommandResult::Applied { receipt } => {
            store.schedule_get(&receipt.id).unwrap().unwrap()
        }
        other => panic!("{other:?}"),
    }
}
fn after() -> ScheduleTiming {
    ScheduleTiming::After { seconds: 60 }
}
#[test]
fn one_shot_commits_one_native_pending_and_no_canonical_message() {
    let (store, binding, clock) = fixture();
    let schedule = create(&store, &binding, &clock, "one", after());
    assert!(matches!(
        store
            .admit_scheduled_inbound(&schedule.id, 1, &clock)
            .unwrap(),
        ScheduleAdmission::NotDue
    ));
    clock.add(60);
    let ScheduleAdmission::Admitted { snapshot, inbound } = store
        .admit_scheduled_inbound(&schedule.id, 1, &clock)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(snapshot.state, ScheduleState::Completed);
    assert_eq!(snapshot.revision, 2);
    assert_eq!(
        snapshot.last_trigger.as_ref().unwrap().sequence,
        inbound.sequence
    );
    assert_eq!(inbound.message.source, UserSource::Runtime);
    assert_eq!(inbound.message.kind, InboundKind::Message);
    assert!(store.load_canonical().unwrap().is_empty());
    assert_eq!(store.load_pending().unwrap().len(), 1);
    assert!(matches!(
        store
            .admit_scheduled_inbound(&schedule.id, 1, &clock)
            .unwrap(),
        ScheduleAdmission::AlreadyAdvanced { .. }
    ));
    assert!(matches!(
        store
            .admit_scheduled_inbound(&schedule.id, 2, &clock)
            .unwrap(),
        ScheduleAdmission::NotDue
    ));
    assert_eq!(store.schedule_summary().unwrap().pending.len(), 1);
    store.adopt_pending_batch(inbound.sequence, None).unwrap();
    assert!(store.schedule_summary().unwrap().pending.is_empty());
    assert_eq!(store.load_canonical().unwrap().len(), 1);
}
#[test]
fn failure_rolls_back_target_schedule_generation_and_sequence() {
    let (store, binding, clock) = fixture();
    let schedule = create(&store, &binding, &clock, "one", after());
    clock.add(60);
    let generation = store.schedule_summary().unwrap().generation;
    store
        .fail_accept_remaining
        .store(1, std::sync::atomic::Ordering::SeqCst);
    assert!(
        store
            .admit_scheduled_inbound(&schedule.id, 1, &clock)
            .is_err()
    );
    assert_eq!(
        store.schedule_get(&schedule.id).unwrap(),
        Some(schedule.clone())
    );
    assert!(store.load_pending().unwrap().is_empty());
    assert_eq!(store.schedule_summary().unwrap().generation, generation);
    let ScheduleAdmission::Admitted { inbound, .. } = store
        .admit_scheduled_inbound(&schedule.id, 1, &clock)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(inbound.sequence.get(), 1);
}
#[test]
fn command_retry_resolves_relative_time_before_sampling_clock_or_current_state() {
    let (store, binding, clock) = fixture();
    let schedule = create(&store, &binding, &clock, "stable", after());
    clock.add(120);
    store
        .admit_scheduled_inbound(&schedule.id, 1, &clock)
        .unwrap();
    let retry = store
        .schedule_command(
            &binding,
            &command(
                "stable",
                ScheduleMutation::Create {
                    title: "Reminder".into(),
                    text: "check result".into(),
                    timing: after(),
                },
            ),
            &clock,
        )
        .unwrap();
    assert_eq!(
        retry,
        ScheduleCommandResult::Applied {
            receipt: (&schedule).into()
        }
    );
    let changed = store
        .schedule_command(
            &binding,
            &command(
                "stable",
                ScheduleMutation::Create {
                    title: "changed".into(),
                    text: "check result".into(),
                    timing: after(),
                },
            ),
            &clock,
        )
        .unwrap();
    assert!(matches!(changed, ScheduleCommandResult::Invalid { .. }));
    assert_eq!(store.schedule_list(0, 100).unwrap().len(), 1);
}
#[test]
fn intervals_collapse_fourteen_days_and_wait_for_native_adoption() {
    let (store, binding, clock) = fixture();
    let schedule = create(
        &store,
        &binding,
        &clock,
        "interval",
        ScheduleTiming::Interval { every_seconds: 60 },
    );
    clock.add(14 * 86400 + 37);
    let ScheduleAdmission::Admitted { snapshot, inbound } = store
        .admit_scheduled_inbound(&schedule.id, 1, &clock)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        snapshot
            .last_trigger
            .as_ref()
            .unwrap()
            .trigger_id
            .occurrence_utc,
        clock.now() - Duration::seconds(37)
    );
    assert_eq!(
        snapshot.next_due_utc,
        Some(clock.now() + Duration::seconds(23))
    );
    clock.add(14 * 86400);
    let generation = store.schedule_summary().unwrap().generation;
    for _ in 0..10 {
        assert!(matches!(
            store
                .admit_scheduled_inbound(&schedule.id, 2, &clock)
                .unwrap(),
            ScheduleAdmission::WaitingForTarget
        ));
    }
    assert_eq!(store.schedule_summary().unwrap().generation, generation);
    assert_eq!(store.load_pending().unwrap()[0].message, inbound.message);
    store.adopt_pending_batch(inbound.sequence, None).unwrap();
    assert!(matches!(
        store
            .admit_scheduled_inbound(&schedule.id, 2, &clock)
            .unwrap(),
        ScheduleAdmission::Admitted { .. }
    ));
    assert_eq!(store.load_pending().unwrap().len(), 1);
}
#[test]
fn semantic_noop_preserves_revision_generation_and_receipt_count() {
    let (store, binding, clock) = fixture();
    let schedule = create(&store, &binding, &clock, "one", after());
    let before = store.schedule_summary().unwrap();
    let result = store
        .schedule_command(
            &binding,
            &command(
                "noop",
                ScheduleMutation::SetEnabled {
                    id: schedule.id.clone(),
                    expected_revision: 1,
                    enabled: true,
                },
            ),
            &clock,
        )
        .unwrap();
    assert_eq!(
        result,
        ScheduleCommandResult::Applied {
            receipt: (&schedule).into()
        }
    );
    assert_eq!(store.schedule_summary().unwrap(), before);
    let receipts: u64 = store
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM schedule_commands", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(receipts, 1);
}
#[test]
fn cas_mutation_and_due_admission_have_one_winner() {
    let (store, binding, clock) = fixture();
    let schedule = create(&store, &binding, &clock, "one", after());
    clock.add(60);
    let ScheduleCommandResult::Applied { receipt: snapshot } = store
        .schedule_command(
            &binding,
            &command(
                "off",
                ScheduleMutation::SetEnabled {
                    id: schedule.id.clone(),
                    expected_revision: 1,
                    enabled: false,
                },
            ),
            &clock,
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(snapshot.revision, 2);
    assert!(matches!(
        store
            .admit_scheduled_inbound(&schedule.id, 1, &clock)
            .unwrap(),
        ScheduleAdmission::AlreadyAdvanced { .. }
    ));
    assert!(store.load_pending().unwrap().is_empty());
    store
        .schedule_command(
            &binding,
            &command(
                "on",
                ScheduleMutation::SetEnabled {
                    id: schedule.id.clone(),
                    expected_revision: 2,
                    enabled: true,
                },
            ),
            &clock,
        )
        .unwrap();
    store
        .admit_scheduled_inbound(&schedule.id, 3, &clock)
        .unwrap();
    assert!(matches!(
        store
            .schedule_command(
                &binding,
                &command(
                    "stale",
                    ScheduleMutation::Delete {
                        id: schedule.id,
                        expected_revision: 3
                    }
                ),
                &clock
            )
            .unwrap(),
        ScheduleCommandResult::Conflict { .. }
    ));
    assert_eq!(store.load_pending().unwrap().len(), 1);
}
#[test]
fn concurrent_wakes_share_one_committed_revision() {
    let (store, binding, clock) = fixture();
    let schedule = create(&store, &binding, &clock, "one", after());
    clock.add(60);
    let store = Arc::new(store);
    let clock = Arc::new(clock);
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let store = store.clone();
            let clock = clock.clone();
            let barrier = barrier.clone();
            let id = schedule.id.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .admit_scheduled_inbound(&id, 1, clock.as_ref())
                    .unwrap()
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, ScheduleAdmission::Admitted { .. }))
            .count(),
        1
    );
    assert_eq!(store.load_pending().unwrap().len(), 1);
}
#[test]
fn edits_do_not_refund_outstanding_target_capacity() {
    let (store, binding, clock) = fixture();
    let schedule = create(
        &store,
        &binding,
        &clock,
        "interval",
        ScheduleTiming::Interval { every_seconds: 60 },
    );
    clock.add(60);
    store
        .admit_scheduled_inbound(&schedule.id, 1, &clock)
        .unwrap();
    let ScheduleCommandResult::Applied { receipt: snapshot } = store
        .schedule_command(
            &binding,
            &command(
                "edit",
                ScheduleMutation::Update {
                    id: schedule.id.clone(),
                    expected_revision: 2,
                    title: "new".into(),
                    text: "new payload".into(),
                    timing: Some(after()),
                },
            ),
            &clock,
        )
        .unwrap()
    else {
        panic!()
    };
    clock.add(60);
    assert!(matches!(
        store
            .admit_scheduled_inbound(&schedule.id, snapshot.revision, &clock)
            .unwrap(),
        ScheduleAdmission::WaitingForTarget
    ));
    assert!(format!("{:?}", store.load_pending().unwrap()[0].message).contains("check result"));
}
#[test]
fn deleted_and_pruned_definitions_cannot_bypass_native_aggregate_capacity() {
    let (store, binding, clock) = fixture();
    for index in 0..256 {
        let schedule = create(
            &store,
            &binding,
            &clock,
            &format!("create-{index}"),
            after(),
        );
        clock.add(60);
        let ScheduleAdmission::Admitted { snapshot, .. } = store
            .admit_scheduled_inbound(&schedule.id, 1, &clock)
            .unwrap()
        else {
            panic!()
        };
        store
            .schedule_command(
                &binding,
                &command(
                    &format!("delete-{index}"),
                    ScheduleMutation::Delete {
                        id: snapshot.id,
                        expected_revision: 2,
                    },
                ),
                &clock,
            )
            .unwrap();
    }
    assert!(store.schedule_list(0, 100).unwrap().is_empty());
    assert_eq!(store.schedule_summary().unwrap().pending.len(), 256);
    let schedule = create(&store, &binding, &clock, "next", after());
    assert_eq!(schedule.id.ordinal, 257);
    clock.add(60);
    assert!(matches!(
        store
            .admit_scheduled_inbound(&schedule.id, 1, &clock)
            .unwrap(),
        ScheduleAdmission::SchedulerCapacity
    ));
    let sequence = store.load_pending().unwrap()[0].sequence;
    store.adopt_pending_batch(sequence, None).unwrap();
    assert!(matches!(
        store
            .admit_scheduled_inbound(&schedule.id, 1, &clock)
            .unwrap(),
        ScheduleAdmission::Admitted { .. }
    ));
}
#[test]
fn reserved_native_provenance_cannot_be_spoofed_by_ordinary_producers() {
    let (store, binding, clock) = fixture();
    let schedule = create(&store, &binding, &clock, "one", after());
    clock.add(60);
    let trigger = ScheduleTriggerId {
        schedule_id: schedule.id,
        admission_revision: 1,
        occurrence_utc: clock.now(),
    };
    let mut draft = crate::durable::sqlite::test_support::draft("fake");
    draft.correlation = Some(trigger.correlation());
    assert!(store.accept_inbound(draft).is_err());
    let mut draft = crate::durable::sqlite::test_support::draft("fake");
    draft.message_id = Some(trigger.message_id());
    assert!(store.accept_inbound(draft).is_err());
    assert!(store.schedule_summary().unwrap().pending.is_empty());
}
#[test]
fn read_bounds_and_foreign_binding_are_enforced() {
    let (store, mut binding, clock) = fixture();
    assert!(store.schedule_list(0, 0).is_err());
    assert!(store.schedule_list(0, 101).is_err());
    binding.conversation_id = ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e19");
    assert!(matches!(
        store
            .schedule_command(
                &binding,
                &command(
                    "one",
                    ScheduleMutation::Create {
                        title: "title".into(),
                        text: "text".into(),
                        timing: after()
                    }
                ),
                &clock
            )
            .unwrap(),
        ScheduleCommandResult::Invalid { .. }
    ));
}
#[test]
fn missing_required_scheduler_index_refuses_current_schema() {
    let (store, _, _) = fixture();
    store
        .lock()
        .unwrap()
        .execute_batch("DROP INDEX schedules_due")
        .unwrap();
    assert!(matches!(
        verify_schema(&store.lock().unwrap()),
        Err(ConversationStoreError::IncompatibleSchema(_))
    ));
}
