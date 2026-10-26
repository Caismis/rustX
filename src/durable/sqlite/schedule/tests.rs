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
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
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
            .admit_scheduled_target(&schedule.id, 1, &clock)
            .unwrap(),
        ScheduleAdmission::NotDue
    ));
    clock.add(60);
    let ScheduleAdmission::Admitted { snapshot, inbound } = store
        .admit_scheduled_target(&schedule.id, 1, &clock)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(snapshot.state, ScheduleState::Completed);
    assert_eq!(snapshot.revision, 2);
    assert_eq!(
        match &snapshot.last_trigger.as_ref().unwrap().target {
            crate::scheduler::ScheduledTargetReceipt::ConversationInbound { sequence, .. } =>
                *sequence,
            crate::scheduler::ScheduledTargetReceipt::Workflow { .. } => panic!("inbound receipt"),
        },
        inbound.sequence
    );
    assert_eq!(inbound.message.source, UserSource::Runtime);
    assert_eq!(inbound.message.kind, InboundKind::Message);
    assert!(store.load_canonical().unwrap().is_empty());
    assert_eq!(store.load_pending().unwrap().len(), 1);
    assert!(matches!(
        store
            .admit_scheduled_target(&schedule.id, 1, &clock)
            .unwrap(),
        ScheduleAdmission::AlreadyAdvanced { .. }
    ));
    assert!(matches!(
        store
            .admit_scheduled_target(&schedule.id, 2, &clock)
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
            .admit_scheduled_target(&schedule.id, 1, &clock)
            .is_err()
    );
    assert_eq!(
        store.schedule_get(&schedule.id).unwrap(),
        Some(schedule.clone())
    );
    assert!(store.load_pending().unwrap().is_empty());
    assert_eq!(store.schedule_summary().unwrap().generation, generation);
    let ScheduleAdmission::Admitted { inbound, .. } = store
        .admit_scheduled_target(&schedule.id, 1, &clock)
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
        .admit_scheduled_target(&schedule.id, 1, &clock)
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
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
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
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
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
        .admit_scheduled_target(&schedule.id, 1, &clock)
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
                .admit_scheduled_target(&schedule.id, 2, &clock)
                .unwrap(),
            ScheduleAdmission::WaitingForTarget
        ));
    }
    assert_eq!(store.schedule_summary().unwrap().generation, generation);
    assert_eq!(store.load_pending().unwrap()[0].message, inbound.message);
    store.adopt_pending_batch(inbound.sequence, None).unwrap();
    assert!(matches!(
        store
            .admit_scheduled_target(&schedule.id, 2, &clock)
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
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
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
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(snapshot.revision, 2);
    assert!(matches!(
        store
            .admit_scheduled_target(&schedule.id, 1, &clock)
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
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
        )
        .unwrap();
    store
        .admit_scheduled_target(&schedule.id, 3, &clock)
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
                &clock,
                &crate::scheduler::ScheduleCommandAuthority::RuntimeControl
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
                    .admit_scheduled_target(&id, 1, clock.as_ref())
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
        .admit_scheduled_target(&schedule.id, 1, &clock)
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
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
        )
        .unwrap()
    else {
        panic!()
    };
    clock.add(60);
    assert!(matches!(
        store
            .admit_scheduled_target(&schedule.id, snapshot.revision, &clock)
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
            .admit_scheduled_target(&schedule.id, 1, &clock)
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
                &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
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
            .admit_scheduled_target(&schedule.id, 1, &clock)
            .unwrap(),
        ScheduleAdmission::SchedulerCapacity
    ));
    let sequence = store.load_pending().unwrap()[0].sequence;
    store.adopt_pending_batch(sequence, None).unwrap();
    assert!(matches!(
        store
            .admit_scheduled_target(&schedule.id, 1, &clock)
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
                &clock,
                &crate::scheduler::ScheduleCommandAuthority::RuntimeControl
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

#[test]
fn due_headers_page_past_a_blocked_prefix_without_decoding_definitions() {
    let (store, binding, clock) = fixture();
    let mut blocked = Vec::new();
    for index in 0..64 {
        blocked.push(create(
            &store,
            &binding,
            &clock,
            &format!("interval-{index}"),
            ScheduleTiming::Interval { every_seconds: 60 },
        ));
    }
    clock.add(60);
    for snapshot in &blocked {
        assert!(matches!(
            store
                .admit_scheduled_target(&snapshot.id, 1, &clock)
                .unwrap(),
            ScheduleAdmission::Admitted { .. }
        ));
    }
    let later = create(
        &store,
        &binding,
        &clock,
        "later",
        ScheduleTiming::After { seconds: 180 },
    );
    clock.add(180);
    let first = store.schedule_due_page(clock.now(), None).unwrap();
    assert_eq!(first.candidates.len(), 64);
    for candidate in first.candidates {
        assert!(matches!(
            store
                .admit_scheduled_target(&candidate.id, candidate.revision, &clock)
                .unwrap(),
            ScheduleAdmission::WaitingForTarget
        ));
    }
    let second = store
        .schedule_due_page(clock.now(), first.next.as_ref())
        .unwrap();
    assert_eq!(second.candidates.len(), 1);
    assert_eq!(second.candidates[0].id, later.id);
    assert!(second.next.is_none());
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE schedules SET snapshot_json='invalid body' WHERE ordinal=?1",
            [later.id.ordinal],
        )
        .unwrap();
    assert_eq!(
        store
            .schedule_due_page(clock.now(), first.next.as_ref())
            .unwrap()
            .candidates[0]
            .id,
        later.id
    );
    assert!(store.schedule_get(&later.id).is_err());
}
#[test]
fn simultaneous_disabled_and_enabled_quota_is_independent_of_historical_creation() {
    let (store, binding, clock) = fixture();
    let mut first = None;
    for index in 0..256 {
        let snapshot = create(
            &store,
            &binding,
            &clock,
            &format!("create-{index}"),
            after(),
        );
        if index == 0 {
            first = Some(snapshot.clone());
        }
        if index % 2 == 0 {
            store
                .schedule_command(
                    &binding,
                    &command(
                        &format!("off-{index}"),
                        ScheduleMutation::SetEnabled {
                            id: snapshot.id,
                            expected_revision: 1,
                            enabled: false,
                        },
                    ),
                    &clock,
                    &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
                )
                .unwrap();
        }
    }
    let next = command(
        "overflow",
        ScheduleMutation::Create {
            title: "one".into(),
            text: "one".into(),
            timing: after(),
        },
    );
    assert_eq!(
        store
            .schedule_command(
                &binding,
                &next,
                &clock,
                &crate::scheduler::ScheduleCommandAuthority::RuntimeControl
            )
            .unwrap(),
        ScheduleCommandResult::Capacity
    );
    let first = first.unwrap();
    store
        .schedule_command(
            &binding,
            &command(
                "delete",
                ScheduleMutation::Delete {
                    id: first.id,
                    expected_revision: 2,
                },
            ),
            &clock,
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
        )
        .unwrap();
    assert!(
        matches!(store.schedule_command(&binding,&next,&clock, &crate::scheduler::ScheduleCommandAuthority::RuntimeControl).unwrap(),ScheduleCommandResult::Applied{receipt} if receipt.id.ordinal==257)
    );
}
#[test]
fn reconciliation_prunes_display_history_without_refunding_native_capacity_or_command_identity() {
    let (store, binding, clock) = fixture();
    let original = create(&store, &binding, &clock, "original", after());
    clock.add(60);
    store
        .admit_scheduled_target(&original.id, 1, &clock)
        .unwrap();
    for index in 0..200 {
        let snapshot = create(
            &store,
            &binding,
            &clock,
            &format!("create-{index}"),
            after(),
        );
        clock.add(60);
        store
            .admit_scheduled_target(&snapshot.id, 1, &clock)
            .unwrap();
    }
    assert!(store.schedule_get(&original.id).unwrap().is_none());
    let before = store.schedule_summary().unwrap();
    assert_eq!(before.pending.len(), 201);
    clock.add(31 * 86400);
    let (removed, generation) = store.prune_schedules(&clock).unwrap();
    assert_eq!(removed, 200);
    assert_eq!(generation, before.generation + 1);
    assert_eq!(store.prune_schedules(&clock).unwrap(), (0, generation));
    assert_eq!(store.schedule_summary().unwrap().pending.len(), 201);
    assert!(store.schedule_list(0, 100).unwrap().is_empty());
    let retry = store
        .schedule_command(
            &binding,
            &command(
                "original",
                ScheduleMutation::Create {
                    title: "Reminder".into(),
                    text: "check result".into(),
                    timing: after(),
                },
            ),
            &clock,
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
        )
        .unwrap();
    assert_eq!(
        retry,
        ScheduleCommandResult::Applied {
            receipt: (&original).into()
        }
    );
}

fn human_request(message: &str, attempt: &str) -> crate::scheduler::ScheduleCommandAuthority {
    crate::scheduler::ScheduleCommandAuthority::HumanRequest {
        message_id: crate::runtime::identity::MessageId::new(message),
        original_attempt_id: crate::runtime::identity::AttemptId::new(attempt),
    }
}
#[test]
fn human_request_budget_is_atomic_and_exact_retries_do_not_charge_twice() {
    let (store, binding, clock) = fixture();
    let authority = human_request("human-1", "attempt-1");
    let create = |key: &str| {
        command(
            key,
            ScheduleMutation::Create {
                title: "Reminder".into(),
                text: "check result".into(),
                timing: after(),
            },
        )
    };
    let original = store
        .schedule_command(&binding, &create("first"), &clock, &authority)
        .unwrap();
    for index in 1..7 {
        store
            .schedule_command(
                &binding,
                &create(&format!("create-{index}")),
                &clock,
                &authority,
            )
            .unwrap();
    }
    clock.add(120);
    assert_eq!(
        store
            .schedule_command(&binding, &create("first"), &clock, &authority)
            .unwrap(),
        original
    );
    let store = Arc::new(store);
    let clock = Arc::new(clock);
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let workers: Vec<_> = (0..2)
        .map(|index| {
            let store = store.clone();
            let clock = clock.clone();
            let binding = binding.clone();
            let authority = authority.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .schedule_command(
                        &binding,
                        &command(
                            &format!("final-{index}"),
                            ScheduleMutation::Create {
                                title: "Reminder".into(),
                                text: "check result".into(),
                                timing: after(),
                            },
                        ),
                        clock.as_ref(),
                        &authority,
                    )
                    .unwrap()
            })
        })
        .collect();
    barrier.wait();
    let outcomes: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(outcomes.iter().filter(|outcome|matches!(outcome,ScheduleCommandResult::Applied{receipt} if receipt.request_budget.as_ref().unwrap().mutations_used==8)).count(),1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ScheduleCommandResult::MutationBudgetExhausted))
            .count(),
        1
    );
    assert_eq!(store.schedule_list(0, 100).unwrap().len(), 8);
}
#[test]
#[allow(clippy::too_many_lines)] // One persisted request across no-op, conflict, deletion and reopen.
fn failed_cas_noop_delete_and_display_pruning_do_not_reset_request_usage() {
    let (store, binding, clock) = fixture();
    let authority = human_request("human-1", "attempt-1");
    let make = |key: &str| {
        command(
            key,
            ScheduleMutation::Create {
                title: "Reminder".into(),
                text: "check result".into(),
                timing: after(),
            },
        )
    };
    let ScheduleCommandResult::Applied { receipt } = store
        .schedule_command(&binding, &make("first"), &clock, &authority)
        .unwrap()
    else {
        panic!()
    };
    let noop = command(
        "noop",
        ScheduleMutation::SetEnabled {
            id: receipt.id.clone(),
            expected_revision: 1,
            enabled: true,
        },
    );
    let ScheduleCommandResult::Applied { receipt: unchanged } = store
        .schedule_command(&binding, &noop, &clock, &authority)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(unchanged.request_budget.unwrap().mutations_used, 1);
    assert!(matches!(
        store
            .schedule_command(
                &binding,
                &command(
                    "stale",
                    ScheduleMutation::Delete {
                        id: receipt.id.clone(),
                        expected_revision: 99
                    }
                ),
                &clock,
                &authority
            )
            .unwrap(),
        ScheduleCommandResult::Conflict { .. }
    ));
    for index in 1..8 {
        store
            .schedule_command(
                &binding,
                &make(&format!("create-{index}")),
                &clock,
                &authority,
            )
            .unwrap();
    }
    store
        .schedule_command(
            &binding,
            &command(
                "delete-control",
                ScheduleMutation::Delete {
                    id: receipt.id.clone(),
                    expected_revision: 1,
                },
            ),
            &clock,
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
        )
        .unwrap();
    assert_eq!(
        store
            .schedule_command(&binding, &make("ninth"), &clock, &authority)
            .unwrap(),
        ScheduleCommandResult::MutationBudgetExhausted
    );
    let retry = store
        .schedule_command(&binding, &make("first"), &clock, &authority)
        .unwrap();
    assert!(matches!(retry,ScheduleCommandResult::Applied{receipt:original} if original==receipt));
    let id = store.schedule_list(0, 1).unwrap()[0].id.clone();
    let result = store
        .schedule_command(
            &binding,
            &command(
                "noop-after-eight",
                ScheduleMutation::SetEnabled {
                    id,
                    expected_revision: 1,
                    enabled: true,
                },
            ),
            &clock,
            &authority,
        )
        .unwrap();
    assert!(
        matches!(result,ScheduleCommandResult::Applied{receipt} if receipt.request_budget.as_ref().unwrap().mutations_used==8)
    );
}
#[test]
fn request_binding_is_pinned_and_runtime_control_has_a_separate_command_scope() {
    let (store, binding, clock) = fixture();
    let make = command(
        "shared",
        ScheduleMutation::Create {
            title: "Reminder".into(),
            text: "check result".into(),
            timing: after(),
        },
    );
    let original = store
        .schedule_command(
            &binding,
            &make,
            &clock,
            &human_request("human-1", "attempt-1"),
        )
        .unwrap();
    assert!(matches!(
        store
            .schedule_command(
                &binding,
                &make,
                &clock,
                &human_request("human-1", "attempt-2")
            )
            .unwrap(),
        ScheduleCommandResult::Invalid { .. }
    ));
    assert!(matches!(
        store
            .schedule_command(
                &binding,
                &command("new-key", make.mutation.clone()),
                &clock,
                &human_request("human-1", "attempt-2")
            )
            .unwrap(),
        ScheduleCommandResult::Invalid { .. }
    ));
    let next = store
        .schedule_command(
            &binding,
            &make,
            &clock,
            &human_request("human-2", "attempt-2"),
        )
        .unwrap();
    assert_ne!(next, original);
    let control = store
        .schedule_command(
            &binding,
            &make,
            &clock,
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
        )
        .unwrap();
    assert!(
        matches!(control,ScheduleCommandResult::Applied{receipt} if receipt.request_budget.is_none())
    );
    assert_eq!(store.schedule_list(0, 100).unwrap().len(), 3);
}
#[test]
fn receipt_failure_rolls_back_mutation_ordinal_generation_and_request_charge() {
    let (store, binding, clock) = fixture();
    let authority = human_request("human-1", "attempt-1");
    let make = command(
        "stable",
        ScheduleMutation::Create {
            title: "Reminder".into(),
            text: "check result".into(),
            timing: after(),
        },
    );
    store.lock().unwrap().execute_batch("CREATE TRIGGER reject_schedule_receipt BEFORE INSERT ON schedule_commands BEGIN SELECT RAISE(ABORT,'receipt fault'); END;").unwrap();
    assert!(
        store
            .schedule_command(&binding, &make, &clock, &authority)
            .is_err()
    );
    assert!(store.schedule_list(0, 100).unwrap().is_empty());
    assert_eq!(store.schedule_summary().unwrap().generation, 0);
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_schedule_receipt")
        .unwrap();
    clock.add(60);
    let ScheduleCommandResult::Applied { receipt } = store
        .schedule_command(&binding, &make, &clock, &authority)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(receipt.id.ordinal, 1);
    assert_eq!(
        receipt.next_due_utc,
        Some(clock.now() + Duration::seconds(60))
    );
    assert_eq!(receipt.request_budget.unwrap().mutations_used, 1);
}

#[test]
fn cron_uses_latest_only_admission_and_native_backpressure_without_replaying_ticks() {
    let (store, binding, clock) = fixture();
    let snapshot = create(
        &store,
        &binding,
        &clock,
        "cron",
        ScheduleTiming::Cron {
            expression: "*/10 * * * *".into(),
            time_zone: "UTC".into(),
        },
    );
    assert_eq!(
        snapshot.next_due_utc,
        Some(clock.now() + Duration::minutes(10))
    );
    clock.add(37 * 60);
    let ScheduleAdmission::Admitted {
        snapshot: admitted,
        inbound,
    } = store
        .admit_scheduled_target(&snapshot.id, 1, &clock)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        admitted.last_trigger.unwrap().trigger_id.occurrence_utc,
        clock.now() - Duration::minutes(7)
    );
    assert_eq!(
        admitted.next_due_utc,
        Some(clock.now() + Duration::minutes(3))
    );
    clock.add(14 * 86400);
    assert!(matches!(
        store
            .admit_scheduled_target(&snapshot.id, 2, &clock)
            .unwrap(),
        ScheduleAdmission::WaitingForTarget
    ));
    store.adopt_pending_batch(inbound.sequence, None).unwrap();
    assert!(matches!(
        store
            .admit_scheduled_target(&snapshot.id, 2, &clock)
            .unwrap(),
        ScheduleAdmission::Admitted { .. }
    ));
    assert_eq!(store.load_pending().unwrap().len(), 1);
}
#[test]
fn cron_exhaustion_completes_temporal_state_and_invalid_saved_rule_disables_without_target() {
    let (store, binding, clock) = fixture();
    *clock.0.lock().unwrap() = DateTime::parse_from_rfc3339("4998-12-31T23:58:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let snapshot = create(
        &store,
        &binding,
        &clock,
        "last",
        ScheduleTiming::Cron {
            expression: "* * * * *".into(),
            time_zone: "Pacific/Kiritimati".into(),
        },
    );
    clock.add(60);
    let ScheduleAdmission::Admitted {
        snapshot: completed,
        ..
    } = store
        .admit_scheduled_target(&snapshot.id, 1, &clock)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(completed.state, ScheduleState::Completed);
    assert_eq!(
        completed.completion_reason,
        Some(crate::scheduler::ScheduleCompletionReason::CalendarExhausted)
    );
    let (store, binding, clock) = fixture();
    let mut snapshot = create(
        &store,
        &binding,
        &clock,
        "invalid-after-upgrade",
        ScheduleTiming::Cron {
            expression: "* * * * *".into(),
            time_zone: "UTC".into(),
        },
    );
    snapshot.spec = crate::scheduler::ScheduleSpec::Cron {
        expression: "* * * * *".into(),
        time_zone: "removed-zone".into(),
    };
    {
        let mut connection = store.lock().unwrap();
        let tx = connection.transaction().unwrap();
        save(&tx, &snapshot).unwrap();
        tx.commit().unwrap();
    }
    clock.add(60);
    let ScheduleAdmission::Disabled { current } = store
        .admit_scheduled_target(&snapshot.id, 1, &clock)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(current.revision, 2);
    assert_eq!(current.state, ScheduleState::Disabled);
    assert!(current.last_failure.is_some());
    assert!(store.load_pending().unwrap().is_empty());
}
